//! TCP-Listener mit Längenpräfix nach RFC 1035 §4.2.2.
//!
//! Zwei Zeitlimits und zwei Obergrenzen halten den Listener auch dann
//! beherrschbar, wenn ein Gerät im LAN nicht mehr mitspielt. Ohne sie hält eine
//! Handvoll stiller Verbindungen den Server offen, und eine Verbindung, die ihr
//! Längenpräfix schickt und dann schweigt, hält einen Task, 64 KB und einen
//! Deskriptor unbegrenzt — im LAN reicht dafür ein einziges kompromittiertes
//! Gerät. Die Drosselung pro Client hilft dagegen nicht: sie wird je *Anfrage*
//! befragt, und eine Slowloris-Verbindung stellt nie eine.

use std::collections::HashMap;
use std::net::IpAddr;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio::sync::Semaphore;
use tokio_util::sync::CancellationToken;
use tokio_util::task::TaskTracker;

use crate::logging::QueryLog;
use crate::ratelimit::RateLimiter;
use crate::resolve::ResolveBackend;

/// Wie lange eine offene Verbindung ohne neue Anfrage warten darf, bevor sie
/// geschlossen wird (RFC 7766 §6.2.3 empfiehlt einige Sekunden). Ohne dieses
/// Limit hält eine Handvoll stiller Verbindungen den Server offen.
const IDLE_TIMEOUT: Duration = Duration::from_secs(10);

/// Wie lange der Body nach seinem Längenpräfix auf sich warten lassen darf.
///
/// Ohne dieses Limit hält eine Verbindung, die zwei Bytes `0xFF 0xFF` schickt
/// und dann schweigt, einen Task, einen 64-KB-Puffer und einen Deskriptor —
/// unbegrenzt, bis jemand den Prozess beendet. Fünf Sekunden für höchstens
/// 64 KB aus dem LAN sind großzügig.
const BODY_TIMEOUT: Duration = Duration::from_secs(5);

/// Wie viele Verbindungen gleichzeitig offen sein dürfen.
///
/// Ohne Obergrenze wäre sie das Deskriptor-Limit des Prozesses. Das Permit
/// wird **vor** dem `accept` geholt: ist keines frei, kommt die Verbindung gar
/// nicht erst zustande, und das Backlog des Kernels drosselt von selbst —
/// statt anzunehmen und sofort wieder zu schließen. Nebenbei ist deshalb immer
/// eines reserviert, während auf die nächste Verbindung gewartet wird;
/// gleichzeitig bedient werden also höchstens `MAX_CONNECTIONS - 1`.
///
/// Vierundsechzig ist für ein Haushalts-LAN viel. Die Zahl ist bewusst so
/// klein, dass die Grenze im Test mit dem echten Wert geprüft werden kann,
/// ohne den Deskriptor-Haushalt des Testprozesses zu sprengen.
const MAX_CONNECTIONS: usize = 64;

/// Wie viele Verbindungen eine einzelne Quell-IP gleichzeitig halten darf.
///
/// Ohne diese Grenze belegt ein einzelnes Gerät alle Plätze, und die
/// Obergrenze wird selbst zur Waffe.
const MAX_PER_CLIENT: u32 = 8;

/// Was der TCP-Listener zu zählen hat.
///
/// Ohne diese Zähler wäre die Obergrenze unsichtbar: ein Gerät, dessen
/// Verbindungen abgewiesen werden, bekommt einfach keine Antworten mehr, und
/// im Log steht nichts. Atomar und ohne Schloss, wie die Zähler in
/// [`crate::privacy`] — sie liegen im Verbindungsaufbau, nicht im Anfragepfad.
#[derive(Debug, Default)]
pub struct Stats {
    /// Verbindungen, die eine Quell-IP über [`MAX_PER_CLIENT`] hinaus aufmachen
    /// wollte.
    rejected_per_client: AtomicU64,
    /// Annahmen, die warten mussten, weil alle Plätze belegt waren. Kein
    /// Fehler, sondern das Zeichen, dass die Obergrenze trägt.
    at_capacity: AtomicU64,
    /// Verbindungen, die ihr Längenpräfix geschickt und dann geschwiegen haben.
    body_timeouts: AtomicU64,
}

/// Momentaufnahme der Zähler, für Metriken und API.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Counters {
    pub rejected_per_client: u64,
    pub at_capacity: u64,
    pub body_timeouts: u64,
}

impl Stats {
    /// Der aktuelle Stand.
    pub fn counters(&self) -> Counters {
        Counters {
            rejected_per_client: self.rejected_per_client.load(Ordering::Relaxed),
            at_capacity: self.at_capacity.load(Ordering::Relaxed),
            body_timeouts: self.body_timeouts.load(Ordering::Relaxed),
        }
    }
}

/// Der Platz einer Quell-IP, solange ihre Verbindung läuft.
///
/// Ein Wächter: der Platz fällt mit dem Task, ohne dass ihn jemand
/// zurückgeben muss — auch dann, wenn die Verbindung mit einem Fehler endet.
struct ClientSlot {
    ip: IpAddr,
    open: Arc<Mutex<HashMap<IpAddr, u32>>>,
}

impl ClientSlot {
    /// `None`, wenn diese Adresse schon [`MAX_PER_CLIENT`] Verbindungen offen
    /// hat.
    fn take(open: Arc<Mutex<HashMap<IpAddr, u32>>>, ip: IpAddr, stats: &Stats) -> Option<Self> {
        let mut counts = open.lock().unwrap_or_else(PoisonError::into_inner);
        let held = counts.entry(ip).or_insert(0);
        if *held >= MAX_PER_CLIENT {
            stats.rejected_per_client.fetch_add(1, Ordering::Relaxed);
            return None;
        }
        *held += 1;
        drop(counts);
        Some(Self { ip, open })
    }
}

impl Drop for ClientSlot {
    fn drop(&mut self) {
        let mut counts = self.open.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(held) = counts.get_mut(&self.ip) {
            // Saturierend, obwohl es nicht unter null fallen kann: ein Panic
            // im Drop bräche den Prozess ab, wenn gerade ein anderer abgewickelt
            // wird (B.1 Regel 1).
            *held = held.saturating_sub(1);
            if *held == 0 {
                counts.remove(&self.ip);
            }
        }
    }
}

pub(crate) async fn serve<B: ResolveBackend>(
    listener: TcpListener,
    backend: Arc<B>,
    log: Arc<QueryLog>,
    limiter: Option<Arc<RateLimiter>>,
    stats: Arc<Stats>,
    shutdown: CancellationToken,
    tracker: TaskTracker,
) {
    let permits = Arc::new(Semaphore::new(MAX_CONNECTIONS));
    let open: Arc<Mutex<HashMap<IpAddr, u32>>> = Arc::new(Mutex::new(HashMap::new()));

    loop {
        // Erst der Platz, dann die Verbindung (siehe MAX_CONNECTIONS). Gezählt
        // wird, bevor gewartet wird, nicht danach: hinterher gezählt käme die
        // Zahl erst, wenn der Platz frei wird — also genau dann, wenn die
        // Grenze längst nicht mehr greift.
        if permits.available_permits() == 0 {
            stats.at_capacity.fetch_add(1, Ordering::Relaxed);
        }
        let permit = tokio::select! {
            () = shutdown.cancelled() => break,
            permit = Arc::clone(&permits).acquire_owned() => match permit {
                Ok(permit) => permit,
                // Geschlossen wird der Semaphor nirgends.
                Err(_) => break,
            },
        };

        let (stream, peer) = tokio::select! {
            () = shutdown.cancelled() => break,
            result = listener.accept() => match result {
                Ok(accepted) => accepted,
                Err(error) => {
                    tracing::warn!(%error, "accept fehlgeschlagen");
                    continue;
                }
            },
        };

        // Das Kontingent der Quell-IP wird erst hier geprüft: vor dem `accept`
        // ist sie nicht bekannt. Die Verbindung anzunehmen und gleich wieder zu
        // schließen ist der richtige Preis — der Client hat sein Kontingent
        // aufgebraucht, und ein Platz für ein anderes Gerät ist mehr wert.
        let Some(slot) = ClientSlot::take(Arc::clone(&open), peer.ip(), &stats) else {
            tracing::debug!(%peer, "zu viele offene Verbindungen von dieser Adresse");
            drop(permit);
            continue;
        };

        let backend = Arc::clone(&backend);
        let log = Arc::clone(&log);
        let limiter = limiter.clone();
        let shutdown = shutdown.clone();
        let stats = Arc::clone(&stats);
        tracker.spawn(async move {
            // Beide Wächter leben, solange der Task lebt.
            let _permit = permit;
            let _slot = slot;
            if let Err(error) = handle_connection(
                stream,
                peer,
                backend.as_ref(),
                log.as_ref(),
                limiter.as_deref(),
                stats.as_ref(),
                &shutdown,
            )
            .await
            {
                tracing::debug!(%error, "TCP-Verbindung beendet");
            }
        });
    }
}

/// Beantwortet Anfragen auf einer Verbindung, bis der Client sie schließt.
///
/// Über den Stream generisch und nicht auf [`tokio::net::TcpStream`] festgelegt:
/// die Zeitlimits lassen sich damit an einem Paar aus [`tokio::io::duplex`]
/// prüfen, ohne fünf Sekunden echte Zeit zu verbrauchen (B.3 Regel 4).
async fn handle_connection<B, S>(
    mut stream: S,
    peer: std::net::SocketAddr,
    backend: &B,
    log: &QueryLog,
    limiter: Option<&RateLimiter>,
    stats: &Stats,
    shutdown: &CancellationToken,
) -> std::io::Result<()>
where
    B: ResolveBackend,
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin,
{
    loop {
        let mut len_buf = [0_u8; 2];
        let read = tokio::select! {
            () = shutdown.cancelled() => return Ok(()),
            result = tokio::time::timeout(IDLE_TIMEOUT, stream.read_exact(&mut len_buf)) => result,
        };
        match read {
            // Zeitüberschreitung im Leerlauf ist der Normalfall, kein Fehler.
            Err(_elapsed) => return Ok(()),
            // Sauberes Verbindungsende zwischen zwei Anfragen.
            Ok(Err(error)) if error.kind() == std::io::ErrorKind::UnexpectedEof => return Ok(()),
            Ok(Err(error)) => return Err(error),
            Ok(Ok(_)) => {}
        }

        let mut packet = vec![0_u8; usize::from(u16::from_be_bytes(len_buf))];
        let read = tokio::select! {
            () = shutdown.cancelled() => return Ok(()),
            result = tokio::time::timeout(BODY_TIMEOUT, stream.read_exact(&mut packet)) => result,
        };
        match read {
            // Das Präfix war da, der Rest nicht: kein Client, sondern ein
            // Slowloris. Der `select!` auf `shutdown` steht hier genauso wie
            // oben — sonst verzögert so eine Verbindung das Herunterfahren bis
            // zum harten Limit.
            Err(_elapsed) => {
                stats.body_timeouts.fetch_add(1, Ordering::Relaxed);
                return Ok(());
            }
            Ok(Err(error)) => return Err(error),
            Ok(Ok(_)) => {}
        }

        let Some(response) =
            crate::server::handle_request(backend, &packet, peer, log, limiter).await
        else {
            continue;
        };
        let bytes = match response.to_vec() {
            Ok(bytes) => bytes,
            Err(error) => {
                tracing::warn!(%error, "Antwort nicht kodierbar");
                continue;
            }
        };
        // Über TCP wird nicht gekürzt; die Längenangabe ist 16 Bit, größer geht
        // eine DNS-Nachricht ohnehin nicht.
        let Ok(len) = u16::try_from(bytes.len()) else {
            tracing::warn!(len = bytes.len(), "Antwort passt nicht in das Längenpräfix");
            continue;
        };
        stream.write_all(&len.to_be_bytes()).await?;
        stream.write_all(&bytes).await?;
        stream.flush().await?;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::resolve::ResolveError;
    use crate::trace::Ctx;
    use hickory_proto::op::{Message, MessageType, OpCode, Query};
    use hickory_proto::rr::{Name, RecordType};
    use tokio::net::TcpStream as Client;

    /// Großzügig, damit der Listener im Test zuerst greift und nicht der Test.
    const OUTER: Duration = Duration::from_secs(30);

    /// Antwortet auf jede Frage mit einer leeren Antwort.
    #[derive(Debug)]
    struct Answering;

    impl ResolveBackend for Answering {
        fn resolve(
            &self,
            request: &Message,
            _ctx: &mut Ctx,
        ) -> impl std::future::Future<Output = Result<Message, ResolveError>> + Send {
            let id = request.metadata.id;
            let queries = request.queries.clone();
            async move {
                let mut response = Message::response(id, OpCode::Query);
                response.add_queries(queries);
                Ok(response)
            }
        }
    }

    struct Harness {
        addr: std::net::SocketAddr,
        stats: Arc<Stats>,
    }

    async fn start() -> Harness {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("Listener");
        let addr = listener.local_addr().expect("Adresse");
        let log = Arc::new(QueryLog::new(&crate::config::LoggingConfig::default()).expect("Log"));
        let stats = Arc::new(Stats::default());
        tokio::spawn(serve(
            listener,
            Arc::new(Answering),
            log,
            None,
            Arc::clone(&stats),
            CancellationToken::new(),
            TaskTracker::new(),
        ));
        Harness { addr, stats }
    }

    fn query_packet(id: u16) -> Vec<u8> {
        let mut message = Message::new(id, MessageType::Query, OpCode::Query);
        message.metadata.recursion_desired = true;
        message.add_query(Query::query(
            Name::from_ascii("example.com.").expect("Name"),
            RecordType::A,
        ));
        message.to_vec().expect("kodierbar")
    }

    /// Ein Client von einer bestimmten Quelladresse. `127.0.0.0/8` ist komplett
    /// lokal, damit lässt sich ein zweites Gerät vortäuschen.
    async fn client_from(addr: std::net::SocketAddr, from: [u8; 4]) -> Client {
        let socket = tokio::net::TcpSocket::new_v4().expect("Socket");
        socket
            .bind(std::net::SocketAddr::from((from, 0)))
            .expect("Quelladresse");
        socket.connect(addr).await.expect("verbunden")
    }

    /// Schickt eine Anfrage und liest die Antwort.
    async fn ask(client: &mut Client, id: u16) -> Message {
        let packet = query_packet(id);
        client
            .write_all(&(packet.len() as u16).to_be_bytes())
            .await
            .expect("Längenpräfix");
        client.write_all(&packet).await.expect("Anfrage");

        let mut len = [0_u8; 2];
        tokio::time::timeout(OUTER, client.read_exact(&mut len))
            .await
            .expect("Antwort kommt")
            .expect("Antwortlänge");
        let mut body = vec![0_u8; usize::from(u16::from_be_bytes(len))];
        tokio::time::timeout(OUTER, client.read_exact(&mut body))
            .await
            .expect("Antwort kommt")
            .expect("Antwort");
        Message::from_vec(&body).expect("parsbar")
    }

    #[tokio::test(start_paused = true)]
    async fn a_prefix_without_a_body_does_not_hold_the_connection() {
        // TODOS Nr. 1, Schritt 1. Am Paar aus `duplex` und nicht am echten
        // Socket: die fünf Sekunden stehen hier nur auf der Uhr der Runtime,
        // und es gibt keine epoll-Meldung, die sich mit ihr überholen könnte.
        let (mut client, server) = tokio::io::duplex(1024);
        let log = QueryLog::new(&crate::config::LoggingConfig::default()).expect("Log");
        let stats = Arc::new(Stats::default());
        let shutdown = CancellationToken::new();
        let backend = Answering;
        tokio::spawn({
            let stats = Arc::clone(&stats);
            async move {
                let _ = handle_connection(
                    server,
                    "127.0.0.1:5353".parse().expect("Adresse"),
                    &backend,
                    &log,
                    None,
                    stats.as_ref(),
                    &shutdown,
                )
                .await;
            }
        });

        // Fünf Bytes angekündigt, null geliefert.
        client.write_all(&[0x00, 0x05]).await.expect("Präfix");

        // Ohne das Zeitlimit hinge das hier, bis jemand den Prozess beendet.
        let mut buf = [0_u8; 1];
        let read = client.read(&mut buf).await.expect("kein Fehler");
        assert_eq!(read, 0, "der Server hat die Verbindung nicht geschlossen");
        assert_eq!(stats.counters().body_timeouts, 1);
    }

    #[tokio::test]
    async fn a_stalled_connection_does_not_block_the_others() {
        let harness = start().await;

        // Ein Gerät kündigt fünf Bytes an und schweigt.
        let mut stall = Client::connect(harness.addr).await.expect("verbunden");
        stall.write_all(&[0x00, 0x05]).await.expect("Präfix");

        // Ein zweites Gerät wird davon nicht berührt.
        let mut other = client_from(harness.addr, [127, 0, 0, 2]).await;
        assert_eq!(ask(&mut other, 0x4d2).await.metadata.id, 0x4d2);

        // Und die schweigende Verbindung ist noch da: ihr Limit ist der
        // Body-Timeout, nicht der Leerlauf. Dass er greift, steht im Test
        // darüber — fünf Sekunden echte Zeit wären hier niemandem etwas wert.
        assert_eq!(harness.stats.counters().body_timeouts, 0);
    }

    #[tokio::test]
    async fn one_address_cannot_hold_every_connection() {
        // TODOS Nr. 1, Schritt 3. Ohne diese Grenze belegt ein Gerät alle
        // Plätze, und die Obergrenze wird selbst zur Waffe.
        let harness = start().await;
        let mut held = Vec::new();
        for _ in 0..MAX_PER_CLIENT {
            held.push(Client::connect(harness.addr).await.expect("verbunden"));
        }

        let mut extra = Client::connect(harness.addr).await.expect("verbunden");
        let mut buf = [0_u8; 1];
        let read = tokio::time::timeout(OUTER, extra.read(&mut buf))
            .await
            .expect("Verbindung endet")
            .expect("kein Fehler");
        assert_eq!(read, 0, "die Verbindung wurde nicht abgewiesen");
        assert_eq!(harness.stats.counters().rejected_per_client, 1);

        // Ein zweites Gerät ist davon nicht betroffen.
        let mut other = client_from(harness.addr, [127, 0, 0, 2]).await;
        assert_eq!(ask(&mut other, 7).await.metadata.id, 7);

        // Und ein freigegebener Platz steht wieder zur Verfügung.
        held.pop();
        let mut next = Client::connect(harness.addr).await.expect("verbunden");
        assert_eq!(ask(&mut next, 8).await.metadata.id, 8);
    }

    #[tokio::test]
    async fn the_connection_limit_waits_instead_of_dropping() {
        // TODOS Nr. 1, Schritt 2. Jede Verbindung von einer eigenen
        // Quelladresse: sonst greift vorher die Grenze je Adresse und der Test
        // prüft das Falsche.
        let harness = start().await;
        let mut held = Vec::new();
        for i in 0..MAX_CONNECTIONS {
            held.push(client_from(harness.addr, [127, 0, 0, i as u8 + 1]).await);
        }

        // Darauf warten, dass die Annahme vor einem vollen Haus steht, statt es
        // zu unterstellen: der Kernel nimmt die Verbindungen schon an, während
        // der Listener noch mitten in der Schleife steckt.
        for _ in 0..10_000 {
            if harness.stats.counters().at_capacity > 0 {
                break;
            }
            tokio::time::sleep(Duration::from_millis(1)).await;
        }
        assert!(
            harness.stats.counters().at_capacity > 0,
            "die Obergrenze hat nicht gegriffen"
        );

        // Alle Plätze belegt: die nächste Verbindung wird nicht abgewiesen,
        // sondern wartet, bis eine frühere fällt.
        let mut waiting = client_from(harness.addr, [127, 0, 0, 200]).await;
        held.pop();
        assert_eq!(ask(&mut waiting, 9).await.metadata.id, 9);
    }
}
