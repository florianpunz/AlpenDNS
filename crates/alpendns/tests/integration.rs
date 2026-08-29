//! Integrationstests gegen einen echten Serverprozess auf einem Loopback-Port.
//!
//! Der Upstream ist immer ein Fake im selben Prozess. Tests gehen nie ins Netz
//! (docs/TESTING.md §4).

// Testcode darf laut B.1 panicken. `clippy.toml` erlaubt das nur in
// #[cfg(test)]-Modulen — ein Integrationstest ist ein eigenes Crate und fällt
// nicht darunter, deshalb hier ausdrücklich.
#![allow(clippy::expect_used, clippy::indexing_slicing, clippy::panic)]

use std::net::{Ipv4Addr, SocketAddr};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::Duration;

use alpendns::caching::CachingBackend;
use alpendns::clock::TestClock;
use alpendns::config::{CacheConfig, Config, LoggingConfig};
use alpendns::logging::QueryLog;
use alpendns::privacy;
use alpendns::server::Server;
use alpendns::upstream::ForwardBackend;
use hickory_proto::op::{Message, MessageType, OpCode, Query, ResponseCode};
use hickory_proto::rr::rdata::{A, TXT};
use hickory_proto::rr::{Name, RData, Record, RecordType};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpStream, UdpSocket};
use tokio_util::sync::CancellationToken;

fn name(s: &str) -> Name {
    Name::from_ascii(s).expect("gültiger Name")
}

fn query_message(qname: &str, qtype: RecordType) -> Message {
    let mut msg = Message::new(0x4d2, MessageType::Query, OpCode::Query);
    msg.metadata.recursion_desired = true;
    msg.add_query(Query::query(name(qname), qtype));
    msg
}

/// Standardantwort des Fakes: die Frage gespiegelt plus ein A-Record.
fn answer_with_a(request: &Message) -> Message {
    let mut response = Message::response(request.metadata.id, OpCode::Query);
    response.add_queries(request.queries.iter().cloned());
    response.add_answer(Record::from_rdata(
        name("example.com."),
        60,
        RData::A(A(Ipv4Addr::new(93, 184, 216, 34))),
    ));
    response
}

struct Fake {
    addr: SocketAddr,
    hits: Arc<AtomicUsize>,
    /// Umlegbar, um einen ausgefallenen Upstream zu simulieren.
    silent: Arc<AtomicBool>,
}

/// Startet einen Fake-Upstream, der jede Anfrage mit `respond` beantwortet.
/// `delay` simuliert Latenz; ein Responder, der `None` liefert, schweigt.
async fn fake_upstream<F>(delay: Duration, respond: F) -> Fake
where
    F: Fn(&Message) -> Option<Message> + Send + Sync + 'static,
{
    let socket = UdpSocket::bind("127.0.0.1:0").await.expect("bind");
    let addr = socket.local_addr().expect("local_addr");
    let hits = Arc::new(AtomicUsize::new(0));
    let silent = Arc::new(AtomicBool::new(false));
    let counter = Arc::clone(&hits);
    let muted = Arc::clone(&silent);

    tokio::spawn(async move {
        let mut buf = vec![0_u8; 4096];
        loop {
            let Ok((len, peer)) = socket.recv_from(&mut buf).await else {
                return;
            };
            counter.fetch_add(1, Ordering::SeqCst);
            let Ok(request) = Message::from_vec(&buf[..len]) else {
                continue;
            };
            if muted.load(Ordering::SeqCst) {
                continue;
            }
            if !delay.is_zero() {
                tokio::time::sleep(delay).await;
            }
            if let Some(response) = respond(&request)
                && let Ok(bytes) = response.to_vec()
            {
                let _ = socket.send_to(&bytes, peer).await;
            }
        }
    });

    Fake { addr, hits, silent }
}

struct Running {
    udp: SocketAddr,
    tcp: SocketAddr,
    /// Die Uhr des Caches. Steht still, bis der Test sie vorstellt.
    clock: Arc<TestClock>,
    shutdown: CancellationToken,
    handle: tokio::task::JoinHandle<()>,
}

async fn start_server(upstream: SocketAddr, timeout: Duration) -> Running {
    start_server_with(upstream, timeout, CacheConfig::default()).await
}

/// Startet einen Server, dessen Cache an einer stellbaren Uhr hängt.
async fn start_server_with(
    upstream: SocketAddr,
    timeout: Duration,
    cache_config: CacheConfig,
) -> Running {
    let config: Config = toml::from_str(
        r#"
[server]
listen_udp = ["127.0.0.1:0"]
listen_tcp = ["127.0.0.1:0"]

[[upstream_pool]]
name = "test"
"#,
    )
    .expect("Testkonfiguration");

    let clock = Arc::new(TestClock::new());
    let backend = CachingBackend::new(
        ForwardBackend::new(upstream, timeout, privacy::Settings::default()),
        &cache_config,
        Arc::clone(&clock),
    );
    let bound = Server::new(
        backend,
        config.server.edns.udp_payload_size,
        Arc::new(QueryLog::new(&LoggingConfig::default()).expect("QueryLog")),
    )
    .bind(&config.server)
    .await
    .expect("bind");
    let udp = *bound.udp_addrs().first().expect("ein UDP-Listener");
    let tcp = *bound.tcp_addrs().first().expect("ein TCP-Listener");

    let shutdown = CancellationToken::new();
    let handle = tokio::spawn(bound.run(shutdown.clone()));
    Running {
        udp,
        tcp,
        clock,
        shutdown,
        handle,
    }
}

/// Wartet, bis `condition` zutrifft, höchstens zwei Sekunden.
///
/// Für Hintergrundaufgaben (Prefetch, serve-stale-Auffrischung): gewartet wird
/// auf ein Ereignis, nicht auf den Ablauf einer Zeitspanne.
async fn wait_until(label: &str, condition: impl Fn() -> bool) {
    let deadline = std::time::Instant::now() + Duration::from_secs(2);
    while std::time::Instant::now() < deadline {
        if condition() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    panic!("Bedingung '{label}' trat nicht innerhalb von 2 s ein");
}

/// Schickt Bytes per UDP und wartet auf die Antwort.
async fn ask_udp(server: SocketAddr, packet: &[u8]) -> Vec<u8> {
    let socket = UdpSocket::bind("127.0.0.1:0").await.expect("bind");
    socket.connect(server).await.expect("connect");
    socket.send(packet).await.expect("send");
    let mut buf = vec![0_u8; 4096];
    let len = tokio::time::timeout(Duration::from_secs(5), socket.recv(&mut buf))
        .await
        .expect("Antwort innerhalb von 5 s")
        .expect("recv");
    buf.truncate(len);
    buf
}

/// Schickt Bytes per TCP inklusive Längenpräfix und liest die Antwort.
async fn ask_tcp(server: SocketAddr, packet: &[u8]) -> Vec<u8> {
    let mut stream = TcpStream::connect(server).await.expect("connect");
    let len = u16::try_from(packet.len()).expect("Testpaket passt in 16 Bit");
    stream
        .write_all(&len.to_be_bytes())
        .await
        .expect("write len");
    stream.write_all(packet).await.expect("write body");
    stream.flush().await.expect("flush");

    let mut len_buf = [0_u8; 2];
    stream.read_exact(&mut len_buf).await.expect("read len");
    let mut body = vec![0_u8; usize::from(u16::from_be_bytes(len_buf))];
    stream.read_exact(&mut body).await.expect("read body");
    body
}

#[tokio::test]
async fn udp_query_is_forwarded_and_answered() {
    let fake = fake_upstream(Duration::ZERO, |req| Some(answer_with_a(req))).await;
    let server = start_server(fake.addr, Duration::from_secs(3)).await;

    let request = query_message("example.com.", RecordType::A);
    let bytes = ask_udp(server.udp, &request.to_vec().expect("kodierbar")).await;
    let response = Message::from_vec(&bytes).expect("dekodierbar");

    assert_eq!(response.metadata.id, request.metadata.id, "ID des Clients");
    assert_eq!(response.metadata.response_code, ResponseCode::NoError);
    assert_eq!(response.answers.len(), 1);
    assert!(response.metadata.recursion_available);
    assert_eq!(fake.hits.load(Ordering::SeqCst), 1);

    server.shutdown.cancel();
    server.handle.await.expect("sauberes Ende");
}

#[tokio::test]
async fn tcp_query_returns_the_same_answer() {
    let fake = fake_upstream(Duration::ZERO, |req| Some(answer_with_a(req))).await;
    let server = start_server(fake.addr, Duration::from_secs(3)).await;

    let request = query_message("example.com.", RecordType::A);
    let bytes = ask_tcp(server.tcp, &request.to_vec().expect("kodierbar")).await;
    let response = Message::from_vec(&bytes).expect("dekodierbar");

    assert_eq!(response.metadata.id, request.metadata.id);
    assert_eq!(response.answers.len(), 1);

    server.shutdown.cancel();
    server.handle.await.expect("sauberes Ende");
}

#[tokio::test]
async fn upstream_answer_with_wrong_id_is_discarded() {
    // Der Fake antwortet mit einer ID, die nicht zur Frage gehört — genau das,
    // was ein Off-Path-Angreifer versuchen würde.
    let fake = fake_upstream(Duration::ZERO, |req| {
        let mut response = answer_with_a(req);
        response.metadata.id = response.metadata.id.wrapping_add(1);
        Some(response)
    })
    .await;
    let server = start_server(fake.addr, Duration::from_millis(300)).await;

    let request = query_message("example.com.", RecordType::A);
    let bytes = ask_udp(server.udp, &request.to_vec().expect("kodierbar")).await;
    let response = Message::from_vec(&bytes).expect("dekodierbar");

    assert_eq!(response.metadata.response_code, ResponseCode::ServFail);
    assert!(
        response.answers.is_empty(),
        "gefälschte Antwort durchgereicht"
    );
    assert_eq!(response.metadata.id, request.metadata.id);

    server.shutdown.cancel();
    server.handle.await.expect("sauberes Ende");
}

#[tokio::test]
async fn upstream_answer_for_wrong_name_is_discarded() {
    let fake = fake_upstream(Duration::ZERO, |req| {
        let mut response = Message::response(req.metadata.id, OpCode::Query);
        response.add_query(Query::query(name("evil.example.com."), RecordType::A));
        Some(response)
    })
    .await;
    let server = start_server(fake.addr, Duration::from_millis(300)).await;

    let bytes = ask_udp(
        server.udp,
        &query_message("example.com.", RecordType::A)
            .to_vec()
            .expect("kodierbar"),
    )
    .await;
    let response = Message::from_vec(&bytes).expect("dekodierbar");
    assert_eq!(response.metadata.response_code, ResponseCode::ServFail);

    server.shutdown.cancel();
    server.handle.await.expect("sauberes Ende");
}

#[tokio::test]
async fn large_answer_sets_tc_over_udp_but_is_complete_over_tcp() {
    let fake = fake_upstream(Duration::ZERO, |req| {
        let mut response = Message::response(req.metadata.id, OpCode::Query);
        response.add_queries(req.queries.iter().cloned());
        // Rund 2 kB: sicher über udp_payload_size (1232) und sicher unter dem,
        // was ein Upstream über UDP überhaupt schicken darf.
        for _ in 0..10 {
            response.add_answer(Record::from_rdata(
                name("example.com."),
                60,
                RData::TXT(TXT::new(vec!["x".repeat(200)])),
            ));
        }
        Some(response)
    })
    .await;
    let server = start_server(fake.addr, Duration::from_secs(3)).await;
    let request = query_message("example.com.", RecordType::TXT)
        .to_vec()
        .expect("kodierbar");

    let over_udp = Message::from_vec(&ask_udp(server.udp, &request).await).expect("dekodierbar");
    assert!(over_udp.metadata.truncation, "TC-Flag fehlt");
    assert!(
        over_udp.answers.is_empty(),
        "gekürzte Antwort trägt Records"
    );

    let over_tcp = Message::from_vec(&ask_tcp(server.tcp, &request).await).expect("dekodierbar");
    assert!(!over_tcp.metadata.truncation, "TCP-Antwort ist gekürzt");
    assert_eq!(over_tcp.answers.len(), 10);

    server.shutdown.cancel();
    server.handle.await.expect("sauberes Ende");
}

#[tokio::test]
async fn malformed_request_gets_formerr() {
    let fake = fake_upstream(Duration::ZERO, |req| Some(answer_with_a(req))).await;
    let server = start_server(fake.addr, Duration::from_secs(3)).await;

    // Gültige ID in den ersten zwei Bytes, danach Müll.
    let bytes = ask_udp(server.udp, &[0xab, 0xcd, 0xff, 0xff, 0x01]).await;
    let response = Message::from_vec(&bytes).expect("dekodierbar");

    assert_eq!(response.metadata.id, 0xabcd);
    assert_eq!(response.metadata.response_code, ResponseCode::FormErr);
    assert_eq!(
        fake.hits.load(Ordering::SeqCst),
        0,
        "kaputte Anfrage darf den Upstream nicht erreichen"
    );

    server.shutdown.cancel();
    server.handle.await.expect("sauberes Ende");
}

#[tokio::test]
async fn silent_upstream_yields_servfail() {
    let fake = fake_upstream(Duration::ZERO, |_| None).await;
    let server = start_server(fake.addr, Duration::from_millis(200)).await;

    let bytes = ask_udp(
        server.udp,
        &query_message("example.com.", RecordType::A)
            .to_vec()
            .expect("kodierbar"),
    )
    .await;
    let response = Message::from_vec(&bytes).expect("dekodierbar");
    assert_eq!(response.metadata.response_code, ResponseCode::ServFail);

    server.shutdown.cancel();
    server.handle.await.expect("sauberes Ende");
}

#[tokio::test]
async fn shutdown_answers_the_request_that_is_still_running() {
    // Der Upstream braucht 400 ms. Wir lösen nach 100 ms den Shutdown aus: die
    // laufende Anfrage muss trotzdem beantwortet werden und run() danach enden.
    let fake = fake_upstream(Duration::from_millis(400), |req| Some(answer_with_a(req))).await;
    let server = start_server(fake.addr, Duration::from_secs(3)).await;

    let request = query_message("example.com.", RecordType::A)
        .to_vec()
        .expect("kodierbar");
    let target = server.udp;
    let client = tokio::spawn(async move { ask_udp(target, &request).await });

    tokio::time::sleep(Duration::from_millis(100)).await;
    server.shutdown.cancel();

    let bytes = client.await.expect("Client-Task");
    let response = Message::from_vec(&bytes).expect("dekodierbar");
    assert_eq!(response.metadata.response_code, ResponseCode::NoError);
    assert_eq!(response.answers.len(), 1);

    tokio::time::timeout(Duration::from_secs(2), server.handle)
        .await
        .expect("run() kehrt nach dem Shutdown zurück")
        .expect("sauberes Ende");
}

#[tokio::test]
async fn second_query_is_served_from_the_cache() {
    let fake = fake_upstream(Duration::ZERO, |req| Some(answer_with_a(req))).await;
    let server = start_server(fake.addr, Duration::from_secs(3)).await;
    let request = query_message("example.com.", RecordType::A)
        .to_vec()
        .expect("kodierbar");

    let first = Message::from_vec(&ask_udp(server.udp, &request).await).expect("dekodierbar");
    let second = Message::from_vec(&ask_udp(server.udp, &request).await).expect("dekodierbar");

    assert_eq!(first.answers.len(), 1);
    assert_eq!(second.answers.len(), 1);
    assert_eq!(
        fake.hits.load(Ordering::SeqCst),
        1,
        "die zweite Anfrage hätte den Upstream nicht erreichen dürfen"
    );

    server.shutdown.cancel();
    server.handle.await.expect("sauberes Ende");
}

#[tokio::test]
async fn cached_answer_keeps_the_id_and_question_of_the_asking_client() {
    let fake = fake_upstream(Duration::ZERO, |req| Some(answer_with_a(req))).await;
    let server = start_server(fake.addr, Duration::from_secs(3)).await;

    let mut first = query_message("example.com.", RecordType::A);
    first.metadata.id = 0x1111;
    let _ = ask_udp(server.udp, &first.to_vec().expect("kodierbar")).await;

    // Andere ID, andere Schreibweise — die Antwort kommt aus dem Cache und muss
    // trotzdem zu dieser Anfrage passen.
    let mut second = query_message("ExAmPlE.CoM.", RecordType::A);
    second.metadata.id = 0x2222;
    let bytes = ask_udp(server.udp, &second.to_vec().expect("kodierbar")).await;
    let response = Message::from_vec(&bytes).expect("dekodierbar");

    assert_eq!(response.metadata.id, 0x2222);
    assert_eq!(
        response
            .queries
            .first()
            .expect("eine Frage")
            .name()
            .to_ascii(),
        "ExAmPlE.CoM.",
        "die Frage des Clients wurde nicht gespiegelt"
    );
    assert_eq!(fake.hits.load(Ordering::SeqCst), 1);

    server.shutdown.cancel();
    server.handle.await.expect("sauberes Ende");
}

#[tokio::test]
async fn hundred_concurrent_queries_reach_the_upstream_once() {
    // Der Upstream braucht lange genug, dass alle Anfragen zusammenlaufen.
    let fake = fake_upstream(Duration::from_millis(200), |req| Some(answer_with_a(req))).await;
    let server = start_server(fake.addr, Duration::from_secs(3)).await;
    let request = query_message("example.com.", RecordType::A)
        .to_vec()
        .expect("kodierbar");

    let mut clients = Vec::with_capacity(100);
    for _ in 0..100 {
        let target = server.udp;
        let packet = request.clone();
        clients.push(tokio::spawn(async move { ask_udp(target, &packet).await }));
    }
    for client in clients {
        let bytes = client.await.expect("Client-Task");
        let response = Message::from_vec(&bytes).expect("dekodierbar");
        assert_eq!(response.answers.len(), 1, "ein Client ging leer aus");
    }

    assert_eq!(
        fake.hits.load(Ordering::SeqCst),
        1,
        "100 gleichzeitige Anfragen erzeugten mehr als eine Upstream-Anfrage"
    );

    server.shutdown.cancel();
    server.handle.await.expect("sauberes Ende");
}

#[tokio::test]
async fn expired_entry_is_served_stale_when_the_upstream_is_gone() {
    let fake = fake_upstream(Duration::ZERO, |req| Some(answer_with_a(req))).await;
    let config = CacheConfig {
        // Prefetch aus, damit nur serve-stale geprüft wird.
        prefetch: false,
        ..CacheConfig::default()
    };
    let server = start_server_with(fake.addr, Duration::from_millis(200), config).await;
    let request = query_message("example.com.", RecordType::A)
        .to_vec()
        .expect("kodierbar");

    let _ = ask_udp(server.udp, &request).await;
    assert_eq!(fake.hits.load(Ordering::SeqCst), 1);

    // Antwort ist abgelaufen (TTL 60 aus answer_with_a), Upstream schweigt.
    server.clock.advance(Duration::from_secs(120));
    fake.silent.store(true, Ordering::SeqCst);

    let response = Message::from_vec(&ask_udp(server.udp, &request).await).expect("dekodierbar");
    assert_eq!(response.metadata.response_code, ResponseCode::NoError);
    assert_eq!(
        response.answers.len(),
        1,
        "abgelaufener Eintrag wurde nicht ausgeliefert"
    );

    server.shutdown.cancel();
    server.handle.await.expect("sauberes Ende");
}

#[tokio::test]
async fn stale_hit_refreshes_in_the_background() {
    let fake = fake_upstream(Duration::ZERO, |req| Some(answer_with_a(req))).await;
    let server = start_server(fake.addr, Duration::from_secs(3)).await;
    let request = query_message("example.com.", RecordType::A)
        .to_vec()
        .expect("kodierbar");

    let _ = ask_udp(server.udp, &request).await;
    server.clock.advance(Duration::from_secs(120));

    // Der Client bekommt sofort die alte Antwort …
    let response = Message::from_vec(&ask_udp(server.udp, &request).await).expect("dekodierbar");
    assert_eq!(response.answers.len(), 1);
    // … und im Hintergrund wird nachgeladen.
    wait_until("Auffrischung erreicht den Upstream", || {
        fake.hits.load(Ordering::SeqCst) >= 2
    })
    .await;

    server.shutdown.cancel();
    server.handle.await.expect("sauberes Ende");
}

#[tokio::test]
async fn prefetch_refreshes_before_the_entry_expires() {
    let fake = fake_upstream(Duration::ZERO, |req| Some(answer_with_a(req))).await;
    let server = start_server(fake.addr, Duration::from_secs(3)).await;
    let request = query_message("example.com.", RecordType::A)
        .to_vec()
        .expect("kodierbar");

    let _ = ask_udp(server.udp, &request).await;
    assert_eq!(fake.hits.load(Ordering::SeqCst), 1);

    // TTL 60, Schwelle 85 % — bei 54 Sekunden ist der Eintrag noch gültig.
    server.clock.advance(Duration::from_secs(54));
    let response = Message::from_vec(&ask_udp(server.udp, &request).await).expect("dekodierbar");
    assert!(!response.metadata.truncation);
    assert_eq!(
        response.answers.len(),
        1,
        "Client hat gewartet statt zu bekommen"
    );

    wait_until("Prefetch erreicht den Upstream", || {
        fake.hits.load(Ordering::SeqCst) >= 2
    })
    .await;

    server.shutdown.cancel();
    server.handle.await.expect("sauberes Ende");
}

#[tokio::test]
async fn servfail_is_not_cached() {
    // Erst schweigt der Upstream (SERVFAIL), dann antwortet er. Wäre der Fehler
    // gecacht, bliebe der Client dauerhaft auf SERVFAIL sitzen.
    let fake = fake_upstream(Duration::ZERO, |req| Some(answer_with_a(req))).await;
    fake.silent.store(true, Ordering::SeqCst);
    let server = start_server(fake.addr, Duration::from_millis(200)).await;
    let request = query_message("example.com.", RecordType::A)
        .to_vec()
        .expect("kodierbar");

    let first = Message::from_vec(&ask_udp(server.udp, &request).await).expect("dekodierbar");
    assert_eq!(first.metadata.response_code, ResponseCode::ServFail);

    fake.silent.store(false, Ordering::SeqCst);
    let second = Message::from_vec(&ask_udp(server.udp, &request).await).expect("dekodierbar");
    assert_eq!(second.metadata.response_code, ResponseCode::NoError);
    assert_eq!(second.answers.len(), 1);

    server.shutdown.cancel();
    server.handle.await.expect("sauberes Ende");
}
