//! Lastmessung gegen einen Fake-Upstream im selben Prozess.
//!
//! Läuft **nicht** in CI und nicht bei `cargo test`: die Zahlen hängen an der
//! Maschine, und ein Lasttest als Teil der Definition of Done würde jeden
//! Durchlauf verlangsamen. Von Hand starten:
//!
//! ```text
//! cargo test --release --test load -- --ignored --nocapture --test-threads=1
//! ```
//!
//! `--test-threads=1` ist Pflicht: sonst laufen die Messungen gleichzeitig und
//! konkurrieren um dieselben Kerne, was den Durchsatz um ein Drittel drückt.
//!
//! Die Ergebnisse gehören nach `docs/BENCHMARKS.md` (docs/TESTING.md §5).

// Testcode darf panicken, siehe B.1 und clippy.toml.
#![allow(clippy::expect_used, clippy::indexing_slicing, clippy::panic)]

use std::net::{Ipv4Addr, SocketAddr};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use alpendns::caching::CachingBackend;
use alpendns::clock::SystemClock;
use alpendns::config::{CacheConfig, Config};
use alpendns::privacy;
use alpendns::server::Server;
use alpendns::upstream::ForwardBackend;
use hickory_proto::op::{Message, MessageType, OpCode, Query};
use hickory_proto::rr::rdata::A;
use hickory_proto::rr::{Name, RData, Record, RecordType};
use tokio::net::UdpSocket;
use tokio_util::sync::CancellationToken;

const CLIENTS: usize = 16;
const PER_CLIENT: usize = 2_000;

/// Resident Set Size dieses Prozesses in KiB, aus `/proc/self/statm`.
///
/// Linux-only, wie das ganze Projekt. Feld 2 ist die Zahl residenter Seiten.
fn resident_kib() -> u64 {
    let statm = std::fs::read_to_string("/proc/self/statm").expect("Linux mit /proc");
    let pages: u64 = statm
        .split_whitespace()
        .nth(1)
        .expect("statm hat ein zweites Feld")
        .parse()
        .expect("Seitenzahl ist eine Zahl");
    // 4 KiB Seiten auf allen Zielplattformen des Projekts.
    pages * 4
}

/// Fake-Upstream, der jeden Namen mit einem A-Record beantwortet.
async fn fake_upstream(hits: Arc<AtomicUsize>) -> SocketAddr {
    let socket = UdpSocket::bind("127.0.0.1:0").await.expect("bind");
    let addr = socket.local_addr().expect("local_addr");
    tokio::spawn(async move {
        let mut buf = vec![0_u8; 4096];
        loop {
            let Ok((len, peer)) = socket.recv_from(&mut buf).await else {
                return;
            };
            hits.fetch_add(1, Ordering::Relaxed);
            let Ok(request) = Message::from_vec(&buf[..len]) else {
                continue;
            };
            let mut response = Message::response(request.metadata.id, OpCode::Query);
            response.add_queries(request.queries.iter().cloned());
            if let Some(query) = request.queries.first() {
                response.add_answer(Record::from_rdata(
                    query.name().clone(),
                    3600,
                    RData::A(A(Ipv4Addr::new(10, 0, 0, 1))),
                ));
            }
            if let Ok(bytes) = response.to_vec() {
                let _ = socket.send_to(&bytes, peer).await;
            }
        }
    });
    addr
}

struct Harness {
    addr: SocketAddr,
    upstream_hits: Arc<AtomicUsize>,
    shutdown: CancellationToken,
}

async fn start(cache_config: CacheConfig) -> Harness {
    let upstream_hits = Arc::new(AtomicUsize::new(0));
    let upstream = fake_upstream(Arc::clone(&upstream_hits)).await;

    let config: Config = toml::from_str(
        "[server]\nlisten_udp = [\"127.0.0.1:0\"]\n\n[[upstream_pool]]\nname = \"last\"\n",
    )
    .expect("Testkonfiguration");

    let backend = CachingBackend::new(
        ForwardBackend::new(
            upstream,
            Duration::from_secs(2),
            privacy::Settings::default(),
        ),
        &cache_config,
        SystemClock,
    );
    let bound = Server::new(backend, config.server.edns.udp_payload_size)
        .bind(&config.server)
        .await
        .expect("bind");
    let addr = *bound.udp_addrs().first().expect("ein UDP-Listener");

    let shutdown = CancellationToken::new();
    tokio::spawn(bound.run(shutdown.clone()));
    Harness {
        addr,
        upstream_hits,
        shutdown,
    }
}

fn packet(name: &str, id: u16) -> Vec<u8> {
    let mut message = Message::new(id, MessageType::Query, OpCode::Query);
    message.add_query(Query::query(
        Name::from_ascii(name).expect("gültiger Name"),
        RecordType::A,
    ));
    message.to_vec().expect("kodierbar")
}

/// Feuert `CLIENTS * PER_CLIENT` Anfragen und liefert Anfragen pro Sekunde.
///
/// `unique` entscheidet, ob jede Anfrage ein neuer Name ist (der Cache kann
/// nicht helfen) oder immer derselbe (wiederholter Korpus).
async fn drive(server: SocketAddr, unique: bool, offset: usize) -> f64 {
    let start = Instant::now();
    let mut clients = Vec::with_capacity(CLIENTS);
    for client in 0..CLIENTS {
        clients.push(tokio::spawn(async move {
            let socket = UdpSocket::bind("127.0.0.1:0").await.expect("bind");
            socket.connect(server).await.expect("connect");
            let mut buf = vec![0_u8; 4096];
            for i in 0..PER_CLIENT {
                let name = if unique {
                    format!("h{}-{client}-{i}.example.", offset)
                } else {
                    "wiederholt.example.".to_owned()
                };
                let id = u16::try_from(i % 65_535).unwrap_or(0);
                if socket.send(&packet(&name, id)).await.is_err() {
                    continue;
                }
                let _ = tokio::time::timeout(Duration::from_secs(2), socket.recv(&mut buf)).await;
            }
        }));
    }
    for client in clients {
        client.await.expect("Client-Task");
    }
    #[expect(clippy::cast_precision_loss, reason = "Zählwerte weit unter 2^53")]
    let total = (CLIENTS * PER_CLIENT) as f64;
    total / start.elapsed().as_secs_f64()
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "Lastmessung: maschinenabhängig, gehört nicht in die Definition of Done"]
async fn throughput_repeated_versus_unique_corpus() {
    let harness = start(CacheConfig::default()).await;
    let total = CLIENTS * PER_CLIENT;
    println!("\nClients: {CLIENTS} × {PER_CLIENT} Anfragen = {total} gesamt");

    let before = harness.upstream_hits.load(Ordering::Relaxed);
    let unique_rate = drive(harness.addr, true, 0).await;
    let unique_upstream = harness.upstream_hits.load(Ordering::Relaxed) - before;

    let before = harness.upstream_hits.load(Ordering::Relaxed);
    let repeat_rate = drive(harness.addr, false, 0).await;
    let repeat_upstream = harness.upstream_hits.load(Ordering::Relaxed) - before;

    println!("\n| Korpus                      | Anfragen/s | Upstream-Anfragen |");
    println!("|-----------------------------|-----------:|------------------:|");
    println!("| jede Anfrage ein neuer Name | {unique_rate:>10.0} | {unique_upstream:>17} |");
    println!("| immer derselbe Name         | {repeat_rate:>10.0} | {repeat_upstream:>17} |");
    println!("\nFaktor Durchsatz: {:.1}x", repeat_rate / unique_rate);

    assert_eq!(
        repeat_upstream, 1,
        "wiederholter Korpus erzeugte {repeat_upstream} Upstream-Anfragen statt einer"
    );
    assert!(
        repeat_rate > unique_rate,
        "der Cache war nicht schneller als der Weg zum Upstream"
    );

    harness.shutdown.cancel();
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "Lastmessung: maschinenabhängig, gehört nicht in die Definition of Done"]
async fn resident_memory_stays_bounded_beyond_max_entries() {
    // Klein genug, dass die Verdrängung im Lauf mehrfach greift.
    let config = CacheConfig {
        max_entries: 10_000,
        ..CacheConfig::default()
    };
    let harness = start(config).await;
    let per_round = CLIENTS * PER_CLIENT;

    // Erste Runde füllt den Cache über die Grenze.
    drive(harness.addr, true, 0).await;
    let after_first = resident_kib();

    // Vier weitere Runden mit jeweils neuen Namen. Ohne Verdrängung müsste der
    // Speicher hier linear mitwachsen.
    for round in 1..5 {
        drive(harness.addr, true, round).await;
    }
    let after_five = resident_kib();

    println!("\nmax_entries: 10 000, je Runde {per_round} neue Namen");
    println!("  RSS nach 1 Runde:  {after_first:>8} KiB");
    println!("  RSS nach 5 Runden: {after_five:>8} KiB");
    println!(
        "  Zuwachs:           {:>8} KiB",
        after_five.saturating_sub(after_first)
    );

    let growth = after_five.saturating_sub(after_first);
    assert!(
        growth < after_first,
        "RSS wuchs um {growth} KiB und damit stärker als der Ausgangswert \
         ({after_first} KiB) — das sieht nach fehlender Verdrängung aus"
    );

    harness.shutdown.cancel();
}
