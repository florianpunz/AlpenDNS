//! Listener und Request-Pipeline.
//!
//! In Phase 1 ist die Pipeline kurz: parsen, weiterleiten, antworten. Cache,
//! Policy und Filter hängen sich in späteren Phasen zwischen `handle_request`
//! und das [`ResolveBackend`] (ARCHITECTURE.md §1).

mod tcp;
mod udp;

use std::net::SocketAddr;
use std::sync::Arc;

use hickory_proto::op::{Message, OpCode, ResponseCode};
use tokio::net::{TcpListener, UdpSocket};
use tokio_util::sync::CancellationToken;
use tokio_util::task::TaskTracker;

use crate::config::ServerConfig;
use crate::dns;
use crate::logging::{BlockReason, QueryEvent, QueryLog};
use crate::ratelimit::RateLimiter;
use crate::resolve::ResolveBackend;
use crate::trace::{Ctx, Step};

/// Gemeinsamer Zustand aller Listener.
#[derive(Debug)]
pub struct Server<B> {
    backend: Arc<B>,
    /// Ab dieser Antwortgröße wird über UDP gekürzt und TC gesetzt.
    udp_payload_size: usize,
    /// Die einzige Stelle, an der Query-Namen den Prozess überleben dürfen.
    log: Arc<QueryLog>,
    /// Fehlt, wenn die Drosselung abgeschaltet ist.
    limiter: Option<Arc<RateLimiter>>,
}

impl<B: ResolveBackend> Server<B> {
    pub fn new(backend: B, udp_payload_size: u16, log: Arc<QueryLog>) -> Self {
        Self {
            backend: Arc::new(backend),
            udp_payload_size: usize::from(udp_payload_size),
            log,
            limiter: None,
        }
    }

    /// Hängt die Drosselung pro Client davor.
    ///
    /// Als eigener Schritt und nicht als Argument von [`Server::new`], weil sie
    /// abschaltbar ist und die Tests, die etwas anderes prüfen, sie nicht
    /// erwähnen sollen.
    #[must_use]
    pub fn with_rate_limit(mut self, limiter: Option<Arc<RateLimiter>>) -> Self {
        self.limiter = limiter;
        self
    }

    /// Öffnet alle konfigurierten Listener.
    ///
    /// Getrennt von [`Bound::run`], damit die tatsächlich vergebenen Ports vorher
    /// abfragbar sind — Tests binden auf Port 0 und brauchen die echte Adresse.
    pub async fn bind(self, config: &ServerConfig) -> std::io::Result<Bound<B>> {
        let mut udp = Vec::with_capacity(config.listen_udp.len());
        for addr in &config.listen_udp {
            udp.push(Arc::new(UdpSocket::bind(addr).await?));
        }
        let mut tcp = Vec::with_capacity(config.listen_tcp.len());
        for addr in &config.listen_tcp {
            tcp.push(TcpListener::bind(addr).await?);
        }
        Ok(Bound {
            server: self,
            udp,
            tcp,
        })
    }
}

/// Ein Server mit offenen Sockets, aber noch ohne laufende Schleifen.
#[derive(Debug)]
pub struct Bound<B> {
    server: Server<B>,
    udp: Vec<Arc<UdpSocket>>,
    tcp: Vec<TcpListener>,
}

impl<B: ResolveBackend> Bound<B> {
    /// Tatsächlich gebundene UDP-Adressen.
    pub fn udp_addrs(&self) -> Vec<SocketAddr> {
        self.udp
            .iter()
            .filter_map(|s| s.local_addr().ok())
            .collect()
    }

    /// Tatsächlich gebundene TCP-Adressen.
    pub fn tcp_addrs(&self) -> Vec<SocketAddr> {
        self.tcp
            .iter()
            .filter_map(|l| l.local_addr().ok())
            .collect()
    }

    /// Läuft, bis `shutdown` ausgelöst wird.
    ///
    /// Danach werden keine neuen Anfragen mehr angenommen, aber alle bereits
    /// laufenden zu Ende beantwortet, bevor die Funktion zurückkehrt.
    pub async fn run(self, shutdown: CancellationToken) {
        let tracker = TaskTracker::new();

        for socket in self.udp {
            tracker.spawn(udp::serve(
                socket,
                Arc::clone(&self.server.backend),
                Arc::clone(&self.server.log),
                self.server.limiter.clone(),
                self.server.udp_payload_size,
                shutdown.clone(),
                tracker.clone(),
            ));
        }
        for listener in self.tcp {
            tracker.spawn(tcp::serve(
                listener,
                Arc::clone(&self.server.backend),
                Arc::clone(&self.server.log),
                self.server.limiter.clone(),
                shutdown.clone(),
                tracker.clone(),
            ));
        }

        tracker.close();
        tracker.wait().await;
    }
}

/// Beantwortet ein einzelnes Anfragepaket.
///
/// Gibt `None` zurück, wenn es nichts gibt, worauf man antworten könnte — dann
/// wird das Paket kommentarlos verworfen.
///
/// Geloggt wird hier bewusst ohne Query-Namen (CLAUDE.md B.1, Regel 3). Die
/// Log-Schicht, die Namen abhängig vom konfigurierten Modus behandeln darf,
/// kommt in Phase 6.
pub(crate) async fn handle_request<B: ResolveBackend>(
    backend: &B,
    raw: &[u8],
    peer: SocketAddr,
    log: &QueryLog,
    limiter: Option<&RateLimiter>,
) -> Option<Message> {
    // Vor allem anderen: was hier verworfen wird, kostet weder Parsen noch
    // Upstream. Verworfen und nicht abgelehnt — eine Antwort an eine
    // womöglich gefälschte Absenderadresse ist genau die Reflexion, gegen die
    // gedrosselt wird (ADR-0020).
    if let Some(limiter) = limiter
        && !limiter.allow(peer.ip())
    {
        // Die Adresse steht nur auf `debug` und damit per Default nirgends:
        // im Betrieb genügt der Zähler, bei der Fehlersuche braucht man das
        // Gerät. Ein Query-Name taucht hier auch dann nicht auf — die Anfrage
        // ist zu diesem Zeitpunkt noch nicht einmal geparst.
        tracing::debug!(%peer, "Anfrage wegen Überschreitung des Limits verworfen");
        return None;
    }

    let request = match Message::from_vec(raw) {
        Ok(request) => request,
        Err(_) => {
            tracing::debug!(bytes = raw.len(), "Anfrage nicht dekodierbar");
            return dns::format_error(raw);
        }
    };

    if request.queries.is_empty() {
        return Some(dns::error_response(&request, ResponseCode::FormErr));
    }
    if request.metadata.op_code != OpCode::Query {
        return Some(dns::error_response(&request, ResponseCode::NotImp));
    }

    let mut ctx = Ctx::new(peer);
    let response = match backend.resolve(&request, &mut ctx).await {
        Ok(mut response) => {
            response.metadata.recursion_available = true;
            // Erst hier, hinter dem Cache: was von der Signaturkette an *diesen*
            // Client geht, hängt an seiner Anfrage. Weiter unten gestrippt
            // landete die gestutzte Fassung im Cache und alle bekämen sie.
            crate::dnssec::for_client(&request, &mut response);
            response
        }
        Err(error) => {
            tracing::warn!(%error, "Auflösung fehlgeschlagen");
            dns::error_response(&request, ResponseCode::ServFail)
        }
    };

    // Der einzige Ort, an dem der Trace die Pipeline verlässt. Was davon
    // gespeichert wird, entscheidet der konfigurierte Modus (ADR-0004).
    log.record(&build_event(&request, &response, &ctx));
    Some(response)
}

/// Setzt aus Anfrage, Antwort und Trace zusammen, was die Logging-Schicht sieht.
fn build_event(request: &Message, response: &Message, ctx: &Ctx) -> QueryEvent {
    let steps = ctx.steps();
    let client = steps
        .iter()
        .find_map(|step| match step {
            Step::ClientMatched { client, .. } => Some(Arc::clone(client)),
            _ => None,
        })
        .unwrap_or_else(|| Arc::from("unbekannt"));
    let query = request.queries.first();

    let blocked = steps
        .iter()
        .any(|step| matches!(step, Step::Synthesized { .. }));

    QueryEvent {
        name: query.map_or_else(String::new, |q| {
            q.name().to_ascii().trim_end_matches('.').to_owned()
        }),
        query_type: query.map_or_else(String::new, |q| q.query_type().to_string()),
        client,
        blocked,
        // Der Grund kommt aus dem Trace, nicht aus einer zweiten Buchführung.
        reason: blocked.then(|| BlockReason::from_steps(steps)),
        rcode: response.metadata.response_code,
        why: steps.iter().map(ToString::to_string).collect(),
        elapsed: ctx.elapsed(),
        // Nur was sichtbar sein soll: `log` zählt mit und erscheint sonst
        // nirgends. Dass die Namen darin den leisen Log-Modi nicht entkommen,
        // regelt weiterhin allein die Logging-Schicht — hier wird nur
        // zusammengetragen, was passiert ist.
        findings: crate::logging::Flagged::from_steps(steps),
    }
}
