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
use alpendns::config::{BlockingConfig, CacheConfig, Config};
use alpendns::filter::matcher::Builder as MatcherBuilder;
use alpendns::filter::parser::{Format, parse};
use alpendns::filter::{Filter, FilterBackend, FilterSet};
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
    start_with_filter(cache_config, FilterSet::default()).await
}

async fn start_with_filter(cache_config: CacheConfig, rules: FilterSet) -> Harness {
    let upstream_hits = Arc::new(AtomicUsize::new(0));
    let upstream = fake_upstream(Arc::clone(&upstream_hits)).await;

    let config: Config = toml::from_str(
        "[server]\nlisten_udp = [\"127.0.0.1:0\"]\n\n[[upstream_pool]]\nname = \"last\"\n",
    )
    .expect("Testkonfiguration");

    let filter = Arc::new(Filter::new(rules, &BlockingConfig::default()));
    let backend = FilterBackend::new(
        filter,
        CachingBackend::new(
            ForwardBackend::new(
                upstream,
                Duration::from_secs(2),
                privacy::Settings::default(),
            ),
            &cache_config,
            SystemClock,
        ),
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

/// So viele Einträge verlangt das Abnahmekriterium von Phase 4.
const BLOCKLIST_ENTRIES: usize = 2_000_000;

/// Erzeugt eine Blockliste mit `count` verschiedenen Domains im hosts-Format.
fn synthetic_blocklist(count: usize) -> String {
    let mut text = String::with_capacity(count * 32);
    for i in 0..count {
        // Verteilt über viele TLDs und Tiefen, damit der Suffix-Nachschlag
        // nicht durch lauter gleich lange Namen begünstigt wird.
        text.push_str(&format!(
            "0.0.0.0 host{i}.zone{}.example{}\n",
            i % 5000,
            i % 7
        ));
    }
    text
}

fn percentile(sorted: &[Duration], p: f64) -> Duration {
    if sorted.is_empty() {
        return Duration::ZERO;
    }
    #[expect(
        clippy::cast_precision_loss,
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "Stichprobengrößen weit unter 2^53"
    )]
    let index = ((sorted.len() - 1) as f64 * p) as usize;
    sorted.get(index).copied().unwrap_or_default()
}

#[test]
#[ignore = "Lastmessung: braucht Minuten und viel Speicher"]
fn matcher_with_two_million_entries() {
    let before = resident_kib();
    let started = Instant::now();
    let text = synthetic_blocklist(BLOCKLIST_ENTRIES);
    let generated = started.elapsed();

    let started = Instant::now();
    let parsed = parse(&text, Format::Hosts);
    let parse_time = started.elapsed();
    assert_eq!(parsed.entries.len(), BLOCKLIST_ENTRIES);

    let started = Instant::now();
    let mut builder = MatcherBuilder::new();
    builder.add("gross", &parsed);
    let matcher = builder.build();
    let build_time = started.elapsed();
    drop(text);
    drop(parsed);
    let after = resident_kib();

    // Nachschlagen messen: Treffer und Nicht-Treffer getrennt, weil ein
    // Nicht-Treffer alle Suffix-Ebenen durchläuft und der teurere Fall ist.
    let mut hits = Vec::with_capacity(20_000);
    let mut misses = Vec::with_capacity(20_000);
    for i in 0..20_000 {
        let hit_name = format!(
            "host{}.zone{}.example{}",
            i * 97,
            (i * 97) % 5000,
            (i * 97) % 7
        );
        let started = Instant::now();
        let found = matcher.lookup(&hit_name);
        hits.push(started.elapsed());
        assert!(found.is_some(), "{hit_name} sollte treffen");

        let miss_name = format!("a.b.c.nichtdrin{i}.example.com");
        let started = Instant::now();
        let found = matcher.lookup(&miss_name);
        misses.push(started.elapsed());
        assert!(found.is_none());
    }
    hits.sort_unstable();
    misses.sort_unstable();

    println!("\nMatcher mit {} Einträgen", matcher.len());
    println!("  Liste erzeugen:  {generated:>10.2?}");
    println!("  Parsen:          {parse_time:>10.2?}");
    println!("  Matcher bauen:   {build_time:>10.2?}");
    println!("  RSS vorher:      {before:>8} KiB");
    println!("  RSS nachher:     {after:>8} KiB");
    println!("  Zuwachs:         {:>8} KiB", after.saturating_sub(before));
    println!(
        "  Treffer     p50 {:>8.0?}  p99 {:>8.0?}",
        percentile(&hits, 0.5),
        percentile(&hits, 0.99)
    );
    println!(
        "  Nicht-Treffer p50 {:>6.0?}  p99 {:>8.0?}",
        percentile(&misses, 0.5),
        percentile(&misses, 0.99)
    );

    // Der Zuwachs oben ist eine Obergrenze: der Allokator gibt freigegebene
    // Blöcke nicht sofort ans System zurück, und Liste wie Parse-Ergebnis lagen
    // zwischenzeitlich zusätzlich im Speicher. Was nach dem Freigeben des
    // Matchers übrig bleibt, trennt das eine vom anderen.
    let entries = matcher.len();
    drop(matcher);
    let freed = resident_kib();
    let owned = after.saturating_sub(freed);
    println!("  RSS ohne Matcher:{freed:>8} KiB");
    #[expect(
        clippy::cast_precision_loss,
        reason = "Byte-Zahlen und Eintragszahlen liegen weit unter 2^53"
    )]
    let per_entry = (owned as f64 * 1024.0) / entries as f64;
    println!("  Matcher selbst:  {owned:>8} KiB, rund {per_entry:.0} Byte je Eintrag");

    assert!(
        percentile(&misses, 0.99) < Duration::from_micros(50),
        "p99 für einen Nicht-Treffer: {:?}",
        percentile(&misses, 0.99)
    );
    assert_eq!(entries, BLOCKLIST_ENTRIES);
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "Lastmessung: braucht Minuten und viel Speicher"]
async fn cache_hit_latency_with_two_million_blocklist_entries() {
    // Das Abnahmekriterium von Phase 4: mit zwei Millionen geladenen Einträgen
    // muss ein Cache-Treffer p99 unter einer Millisekunde bleiben.
    let parsed = parse(&synthetic_blocklist(BLOCKLIST_ENTRIES), Format::Hosts);
    let mut builder = MatcherBuilder::new();
    builder.add("gross", &parsed);
    let rules = FilterSet {
        block: builder.build(),
        allow: Default::default(),
    };
    let entries = rules.block.len();
    drop(parsed);

    let harness = start_with_filter(CacheConfig::default(), rules).await;
    let rss = resident_kib();

    // Einmal aufwärmen, danach kommt alles aus dem Cache.
    let request = packet("nicht-geblockt.example.", 1);
    let warmup = UdpSocket::bind("127.0.0.1:0").await.expect("bind");
    warmup.connect(harness.addr).await.expect("connect");
    warmup.send(&request).await.expect("send");
    let mut buf = vec![0_u8; 4096];
    let _ = tokio::time::timeout(Duration::from_secs(2), warmup.recv(&mut buf)).await;

    let mut latencies = Vec::with_capacity(20_000);
    for i in 0..20_000 {
        let packet = packet(
            "nicht-geblockt.example.",
            u16::try_from(i % 65_535).unwrap_or(0),
        );
        let started = Instant::now();
        warmup.send(&packet).await.expect("send");
        let _ = tokio::time::timeout(Duration::from_secs(2), warmup.recv(&mut buf))
            .await
            .expect("Antwort");
        latencies.push(started.elapsed());
    }
    latencies.sort_unstable();

    println!("\nCache-Treffer bei {entries} Blocklisten-Einträgen");
    println!("  RSS des Prozesses: {rss:>8} KiB");
    println!("  p50 {:>10.2?}", percentile(&latencies, 0.5));
    println!("  p99 {:>10.2?}", percentile(&latencies, 0.99));
    println!("  p999 {:>9.2?}", percentile(&latencies, 0.999));

    assert_eq!(
        harness.upstream_hits.load(Ordering::Relaxed),
        1,
        "Cache griff nicht"
    );
    assert!(
        percentile(&latencies, 0.99) < Duration::from_millis(1),
        "p99 war {:?}, verlangt sind unter 1 ms",
        percentile(&latencies, 0.99)
    );

    harness.shutdown.cancel();
}
