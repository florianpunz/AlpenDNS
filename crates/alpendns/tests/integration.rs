//! Integrationstests gegen einen echten Serverprozess auf einem Loopback-Port.
//!
//! Der Upstream ist immer ein Fake im selben Prozess. Tests gehen nie ins Netz
//! (docs/TESTING.md §4).

// Testcode darf laut B.1 panicken. `clippy.toml` erlaubt das nur in
// #[cfg(test)]-Modulen — ein Integrationstest ist ein eigenes Crate und fällt
// nicht darunter, deshalb hier ausdrücklich.
#![allow(clippy::expect_used, clippy::indexing_slicing)]

use std::net::{Ipv4Addr, SocketAddr};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use alpendns::config::Config;
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
    let counter = Arc::clone(&hits);

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

    Fake { addr, hits }
}

struct Running {
    udp: SocketAddr,
    tcp: SocketAddr,
    shutdown: CancellationToken,
    handle: tokio::task::JoinHandle<()>,
}

async fn start_server(upstream: SocketAddr, timeout: Duration) -> Running {
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

    let backend = ForwardBackend::new(upstream, timeout);
    let bound = Server::new(backend, config.server.edns.udp_payload_size)
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
        shutdown,
        handle,
    }
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
