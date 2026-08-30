//! Drosselung pro Client gegen einen echten Serverprozess.
//!
//! Das Abnahmekriterium aus ROADMAP Phase 9, Schritt 5 wörtlich: eine IP über
//! dem Limit wird gedrosselt, andere IPs bleiben unbeeinflusst. Deshalb reicht
//! ein Unit-Test hier nicht — geprüft werden muss, dass die Absenderadresse des
//! Pakets ankommt und nicht etwa die des Listeners.
//!
//! Die zweite Adresse ist `127.0.0.2`: Linux gibt das ganze `127.0.0.0/8` an
//! Loopback, ohne dass etwas konfiguriert werden muss.

// Testcode darf laut B.1 panicken; ein Integrationstest ist ein eigenes Crate
// und fällt nicht unter clippy.toml.
#![allow(clippy::expect_used, clippy::indexing_slicing, clippy::panic)]

use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::sync::Arc;
use std::time::Duration;

use alpendns::clock::{Clock, TestClock};
use alpendns::config::{Config, LoggingConfig, RateLimitConfig};
use alpendns::logging::QueryLog;
use alpendns::privacy;
use alpendns::ratelimit::RateLimiter;
use alpendns::server::Server;
use alpendns::upstream::ForwardBackend;
use hickory_proto::op::{Message, MessageType, OpCode, Query};
use hickory_proto::rr::rdata::A;
use hickory_proto::rr::{Name, RData, Record, RecordType};
use tokio::net::UdpSocket;
use tokio_util::sync::CancellationToken;

fn name(text: &str) -> Name {
    Name::from_ascii(text).expect("gültiger Name")
}

fn packet(id: u16) -> Vec<u8> {
    let mut message = Message::new(id, MessageType::Query, OpCode::Query);
    message.metadata.recursion_desired = true;
    message.add_query(Query::query(name("example.com."), RecordType::A));
    message.to_vec().expect("Testpaket")
}

/// Fake-Upstream, der jede Anfrage sofort beantwortet.
async fn fake_upstream() -> SocketAddr {
    let socket = UdpSocket::bind("127.0.0.1:0").await.expect("bind");
    let addr = socket.local_addr().expect("local_addr");
    tokio::spawn(async move {
        let mut buf = vec![0_u8; 4096];
        loop {
            let Ok((len, peer)) = socket.recv_from(&mut buf).await else {
                return;
            };
            let Ok(request) = Message::from_vec(&buf[..len]) else {
                continue;
            };
            let mut response = Message::response(request.metadata.id, OpCode::Query);
            response.add_queries(request.queries.iter().cloned());
            response.add_answer(Record::from_rdata(
                name("example.com."),
                60,
                RData::A(A(Ipv4Addr::new(93, 184, 216, 34))),
            ));
            if let Ok(bytes) = response.to_vec() {
                let _ = socket.send_to(&bytes, peer).await;
            }
        }
    });
    addr
}

struct Running {
    udp: SocketAddr,
    limiter: Arc<RateLimiter>,
    clock: Arc<TestClock>,
    shutdown: CancellationToken,
    handle: tokio::task::JoinHandle<()>,
}

impl Running {
    async fn stop(self) {
        self.shutdown.cancel();
        let _ = self.handle.await;
    }
}

async fn start(qps: u32, burst: u32) -> Running {
    let upstream = fake_upstream().await;
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
    let limiter = Arc::new(RateLimiter::new(
        qps,
        burst,
        1024,
        Arc::clone(&clock) as Arc<dyn Clock>,
    ));
    let backend = ForwardBackend::new(
        upstream,
        Duration::from_secs(3),
        privacy::Settings::default(),
    );
    let bound = Server::new(
        backend,
        config.server.edns.udp_payload_size,
        Arc::new(QueryLog::new(&LoggingConfig::default()).expect("QueryLog")),
    )
    .with_rate_limit(Some(Arc::clone(&limiter)))
    .bind(&config.server)
    .await
    .expect("bind");
    let udp = *bound.udp_addrs().first().expect("ein UDP-Listener");
    let shutdown = CancellationToken::new();
    let handle = tokio::spawn(bound.run(shutdown.clone()));
    Running {
        udp,
        limiter,
        clock,
        shutdown,
        handle,
    }
}

/// Schickt eine Anfrage von `source` und sagt, ob eine Antwort kam.
///
/// Die Wartezeit ist kurz, weil sie im Erfolgsfall nie ausgeschöpft wird: der
/// Upstream ist ein Fake im selben Prozess. Im Drosselungsfall wartet der Test
/// sie ab — das ist der Preis dafür, dass Verwerfen kein Paket erzeugt, an dem
/// man es schneller erkennen könnte.
async fn asked(server: SocketAddr, source: IpAddr, id: u16) -> bool {
    let socket = UdpSocket::bind(SocketAddr::new(source, 0))
        .await
        .expect("bind");
    socket.connect(server).await.expect("connect");
    socket.send(&packet(id)).await.expect("send");
    let mut buf = vec![0_u8; 4096];
    tokio::time::timeout(Duration::from_millis(300), socket.recv(&mut buf))
        .await
        .is_ok()
}

const LOUD: IpAddr = IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1));
const QUIET: IpAddr = IpAddr::V4(Ipv4Addr::new(127, 0, 0, 2));

#[tokio::test(flavor = "multi_thread")]
async fn one_client_over_the_limit_is_throttled_and_others_are_not() {
    let server = start(10, 5).await;

    // Der Burst geht durch.
    for id in 0..5_u16 {
        assert!(
            asked(server.udp, LOUD, id).await,
            "Anfrage {id} im Burst blieb unbeantwortet"
        );
    }
    // Darüber hinaus nicht — die Uhr steht, es läuft nichts nach.
    assert!(
        !asked(server.udp, LOUD, 100).await,
        "die sechste Anfrage desselben Clients wurde beantwortet"
    );

    // Der andere Client hat sein eigenes Guthaben.
    for id in 0..5_u16 {
        assert!(
            asked(server.udp, QUIET, 200 + id).await,
            "der zweite Client wurde bei Anfrage {id} mitgedrosselt"
        );
    }

    assert!(
        server.limiter.throttled() >= 1,
        "die Verwerfung steht nicht im Zähler"
    );
    server.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_throttled_client_recovers_when_credit_runs_back() {
    let server = start(10, 2).await;

    for id in 0..2_u16 {
        assert!(asked(server.udp, LOUD, id).await);
    }
    assert!(!asked(server.udp, LOUD, 50).await, "über dem Limit bedient");

    // 10 Anfragen je Sekunde: nach einer halben Sekunde sind zwei zurück.
    server.clock.advance(Duration::from_millis(500));
    assert!(
        asked(server.udp, LOUD, 51).await,
        "der Client blieb gesperrt, obwohl Guthaben nachgelaufen ist"
    );
    server.stop().await;
}

/// Ohne Limiter darf sich nichts ändern — sonst wäre `enabled = false` eine
/// stille Drosselung mit anderem Namen.
#[tokio::test(flavor = "multi_thread")]
async fn without_a_limiter_nothing_is_dropped() {
    let upstream = fake_upstream().await;
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
    let backend = ForwardBackend::new(
        upstream,
        Duration::from_secs(3),
        privacy::Settings::default(),
    );
    let bound = Server::new(
        backend,
        config.server.edns.udp_payload_size,
        Arc::new(QueryLog::new(&LoggingConfig::default()).expect("QueryLog")),
    )
    .with_rate_limit(RateLimiter::from_config(
        &RateLimitConfig {
            enabled: false,
            ..RateLimitConfig::default()
        },
        Arc::new(TestClock::new()),
    ))
    .bind(&config.server)
    .await
    .expect("bind");
    let udp = *bound.udp_addrs().first().expect("ein UDP-Listener");
    let shutdown = CancellationToken::new();
    let handle = tokio::spawn(bound.run(shutdown.clone()));

    for id in 0..50_u16 {
        assert!(
            asked(udp, LOUD, id).await,
            "Anfrage {id} blieb ohne Limiter unbeantwortet"
        );
    }

    shutdown.cancel();
    let _ = handle.await;
}
