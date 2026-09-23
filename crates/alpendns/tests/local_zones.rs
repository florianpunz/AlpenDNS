//! Lokale Zonen durch die echte Pipeline.
//!
//! Die Tabelle selbst hat ihre Tests neben sich in `local.rs`. Hier geht es um
//! das eine, was nur ein laufender Server zeigen kann: **eine Anfrage, die
//! lokal beantwortet wird, hat den Upstream nie erreicht.** Der Fake-Upstream
//! zählt deshalb jede Anfrage, die bei ihm ankommt; die Null ist die Aussage.
//!
//! Kein Internet, kein echter Resolver, kein echter Nameserver im LAN
//! (`docs/TESTING.md`).

// Testcode darf panicken, siehe B.1 und clippy.toml.
#![allow(clippy::expect_used, clippy::indexing_slicing, clippy::panic)]

use std::net::{Ipv4Addr, SocketAddr};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use alpendns::caching::CachingBackend;
use alpendns::clock::{SystemClock, SystemWallClock};
use alpendns::config::{BlockingConfig, CacheConfig, Config};
use alpendns::detect::Detectors;
use alpendns::filter::LoadedLists;
use alpendns::filter::matcher::Builder as MatcherBuilder;
use alpendns::filter::parser::{Format, parse};
use alpendns::local::{LocalBackend, build_zones};
use alpendns::logging::QueryLog;
use alpendns::policy::rules::RegexRules;
use alpendns::policy::{Blueprint, Engine, PolicyBackend, PolicyBlueprint};
use alpendns::privacy;
use alpendns::server::Server;
use alpendns::upstream::ForwardBackend;
use hickory_proto::op::{Message, MessageType, OpCode, Query, ResponseCode};
use hickory_proto::rr::rdata::A;
use hickory_proto::rr::{Name, RData, Record, RecordType};
use tokio::net::UdpSocket;
use tokio_util::sync::CancellationToken;

/// Der Teil der Konfiguration, der in jedem Test gleich ist.
const BASE: &str = r#"
[server]
listen_udp = ["127.0.0.1:0"]
listen_tcp = ["127.0.0.1:0"]

[[upstream_pool]]
name = "test"

[[upstream_pool.resolver]]
name = "fake"
addr = "dot://9.9.9.9:853"
tls_name = "dns.example.net"
"#;

/// Fake-Upstream, der jeden Namen beantwortet und die Anfragen zählt.
///
/// Die Adresse in der Antwort ist bewusst eine andere als die der lokalen
/// Zone: so ist am Wert zu sehen, wer geantwortet hat.
async fn fake_upstream(hits: Arc<AtomicUsize>) -> SocketAddr {
    let socket = UdpSocket::bind("127.0.0.1:0").await.expect("bind");
    let addr = socket.local_addr().expect("local_addr");
    tokio::spawn(async move {
        let mut buf = vec![0_u8; 4096];
        loop {
            let Ok((len, peer)) = socket.recv_from(&mut buf).await else {
                return;
            };
            hits.fetch_add(1, Ordering::SeqCst);
            let Ok(request) = Message::from_vec(&buf[..len]) else {
                continue;
            };
            let mut response = Message::response(request.metadata.id, OpCode::Query);
            response.metadata.recursion_available = true;
            response.add_queries(request.queries.iter().cloned());
            if let Some(query) = request.queries.first() {
                response.add_answer(Record::from_rdata(
                    query.name().clone(),
                    3600,
                    RData::A(A(Ipv4Addr::new(203, 0, 113, 7))),
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
    hits: Arc<AtomicUsize>,
    shutdown: CancellationToken,
}

impl Drop for Harness {
    fn drop(&mut self) {
        self.shutdown.cancel();
    }
}

/// Startet den Server mit einer Blockliste, die `blocked` enthält.
///
/// Die Kette ist die aus `main`, nur ohne `ZoneRouter` — dieser Test
/// konfiguriert keine `forward_zone`.
async fn start(local_zone: &str, blocked: &[&str]) -> Harness {
    let hits = Arc::new(AtomicUsize::new(0));
    let upstream = fake_upstream(Arc::clone(&hits)).await;

    // `Config::validate` ist privat; sie läuft hier nicht mit. Was sie für
    // lokale Zonen tut, prüft `build_zones` — dieselbe Funktion, die auch
    // `validate` aufruft.
    let config: Config =
        toml::from_str(&format!("{BASE}\n{local_zone}")).expect("Testkonfiguration parst");
    let zones = build_zones(&config.local_zone).expect("lokale Zonen");

    let text = blocked.join("\n");
    let parsed = parse(&text, Format::Wildcard);
    let mut builder = MatcherBuilder::new();
    builder.add("test", &parsed);
    let lists = LoadedLists::from_matchers(vec![(Arc::from("test"), Arc::new(builder.build()))]);

    let entries = lists.total_entries();
    let blueprint = Blueprint::new(
        Vec::new(),
        Arc::from("default"),
        vec![PolicyBlueprint {
            name: Arc::from("default"),
            blocklists: vec![Arc::from("test")],
            allowlists: Vec::new(),
            regex: Arc::new(RegexRules::default()),
            schedules: Vec::new(),
        }],
    );
    let engine = Arc::new(
        Engine::new(
            blueprint.build(&lists).expect("Regelstand"),
            entries,
            &BlockingConfig::default(),
            SystemClock,
            SystemWallClock,
        )
        .with_detectors(Detectors::new()),
    );

    let backend = PolicyBackend::new(
        engine,
        CachingBackend::new(
            LocalBackend::new(
                zones,
                ForwardBackend::new(
                    upstream,
                    Duration::from_secs(2),
                    privacy::Settings::default(),
                ),
            ),
            &CacheConfig::default(),
            SystemClock,
        ),
    );

    let bound = Server::new(
        backend,
        config.server.edns.udp_payload_size,
        Arc::new(QueryLog::new(&config.privacy.logging).expect("QueryLog")),
    )
    .bind(&config.server)
    .await
    .expect("bind");
    let addr = *bound.udp_addrs().first().expect("ein UDP-Listener");

    let shutdown = CancellationToken::new();
    tokio::spawn(bound.run(shutdown.clone()));
    Harness {
        addr,
        hits,
        shutdown,
    }
}

impl Harness {
    async fn ask(&self, name: &str, qtype: RecordType) -> Message {
        let socket = UdpSocket::bind("127.0.0.1:0").await.expect("bind");
        socket.connect(self.addr).await.expect("connect");
        let mut message = Message::new(0x4242, MessageType::Query, OpCode::Query);
        message.metadata.recursion_desired = true;
        message.add_query(Query::query(
            Name::from_ascii(name).expect("gültiger Name"),
            qtype,
        ));
        socket
            .send(&message.to_vec().expect("kodierbar"))
            .await
            .expect("send");
        let mut buf = vec![0_u8; 4096];
        let len = tokio::time::timeout(Duration::from_secs(2), socket.recv(&mut buf))
            .await
            .expect("Antwort innerhalb von zwei Sekunden")
            .expect("recv");
        Message::from_vec(&buf[..len]).expect("lesbare Antwort")
    }

    fn upstream_hits(&self) -> usize {
        self.hits.load(Ordering::SeqCst)
    }

    /// Die Adresse aus der Antwort des Fake-Upstreams.
    const UPSTREAM: Ipv4Addr = Ipv4Addr::new(203, 0, 113, 7);
}

const SPLIT_HORIZON: &str = r#"
[[local_zone]]
zone = "miloo.at"
records = [
  { name = "@",   type = "A", value = "192.168.1.5" },
  { name = "nas", type = "A", value = "192.168.1.5" },
]
"#;

const CLOSED: &str = r#"
[[local_zone]]
zone = "miloo.at"
fallback = "nxdomain"
"#;

#[tokio::test]
async fn a_local_name_never_reaches_the_upstream() {
    let server = start(SPLIT_HORIZON, &[]).await;
    let response = server.ask("nas.miloo.at.", RecordType::A).await;

    assert_eq!(response.metadata.response_code, ResponseCode::NoError);
    assert_eq!(
        response.answers.first().map(|record| &record.data),
        Some(&RData::A(A(Ipv4Addr::new(192, 168, 1, 5)))),
        "die Antwort kam nicht aus der Tabelle"
    );
    assert_eq!(
        server.upstream_hits(),
        0,
        "der Name hat den Upstream erreicht"
    );
}

#[tokio::test]
async fn the_apex_is_the_zone_itself() {
    let server = start(SPLIT_HORIZON, &[]).await;
    let response = server.ask("miloo.at.", RecordType::A).await;

    assert_eq!(
        response.answers.first().map(|record| &record.data),
        Some(&RData::A(A(Ipv4Addr::new(192, 168, 1, 5))))
    );
    assert_eq!(server.upstream_hits(), 0);
}

#[tokio::test]
async fn a_closed_zone_answers_nxdomain_and_asks_nobody() {
    let server = start(CLOSED, &[]).await;
    let response = server.ask("www.miloo.at.", RecordType::A).await;

    assert_eq!(response.metadata.response_code, ResponseCode::NXDomain);
    assert!(response.answers.is_empty());
    assert_eq!(
        server.upstream_hits(),
        0,
        "aus einer geschlossenen Zone ging etwas hinaus"
    );
}

#[tokio::test]
async fn a_missing_type_is_nodata_not_a_question_to_the_upstream() {
    let server = start(SPLIT_HORIZON, &[]).await;
    let response = server.ask("nas.miloo.at.", RecordType::AAAA).await;

    assert_eq!(response.metadata.response_code, ResponseCode::NoError);
    assert!(response.answers.is_empty(), "NODATA heißt ohne Antwortsatz");
    assert_eq!(
        server.upstream_hits(),
        0,
        "ein fehlender Typ öffnete den zweiten Horizont"
    );
}

#[tokio::test]
async fn split_horizon_lets_everything_else_through() {
    // Die Gegenprobe zu allem darüber: ohne sie könnten die Tests bestehen,
    // weil der Fake-Upstream gar nicht erreichbar ist.
    let server = start(SPLIT_HORIZON, &[]).await;
    let response = server.ask("www.miloo.at.", RecordType::A).await;

    assert_eq!(
        response.answers.first().map(|record| &record.data),
        Some(&RData::A(A(Harness::UPSTREAM))),
        "die Antwort kam nicht vom Upstream"
    );
    assert_eq!(server.upstream_hits(), 1);
}

#[tokio::test]
async fn an_unknown_zone_reaches_the_upstream() {
    // Die zweite Gegenprobe: eine Zone, die gar nicht konfiguriert ist.
    let server = start(SPLIT_HORIZON, &[]).await;
    let response = server.ask("example.com.", RecordType::A).await;

    assert_eq!(
        response.answers.first().map(|record| &record.data),
        Some(&RData::A(A(Harness::UPSTREAM)))
    );
    assert_eq!(server.upstream_hits(), 1);
}

#[tokio::test]
async fn blocking_beats_the_table() {
    // Sonst wäre die Filterung über eine Zeile in der Tabelle aushebelbar:
    // wer `nas.miloo.at` lokal festnagelt, hätte damit eine Blockliste
    // ausgehebelt, die genau diesen Namen sperrt.
    let server = start(SPLIT_HORIZON, &["nas.miloo.at"]).await;
    let response = server.ask("nas.miloo.at.", RecordType::A).await;

    assert_eq!(response.metadata.response_code, ResponseCode::NXDomain);
    assert!(response.answers.is_empty());
    assert_eq!(server.upstream_hits(), 0);
}

#[tokio::test]
async fn a_local_answer_carries_its_zone_ttl() {
    let server = start(
        r#"
[[local_zone]]
zone = "miloo.at"
ttl = 42
records = [{ name = "nas", type = "A", value = "192.168.1.5" }]
"#,
        &[],
    )
    .await;
    let response = server.ask("nas.miloo.at.", RecordType::A).await;

    assert_eq!(response.answers.first().map(|record| record.ttl), Some(42));
    assert_eq!(server.upstream_hits(), 0);
}

#[tokio::test]
async fn the_zone_of_the_clients_spelling_stays() {
    // 0x20-Kodierung: der Owner-Name kommt so zurück, wie gefragt wurde.
    let server = start(SPLIT_HORIZON, &[]).await;
    let response = server.ask("NaS.MiLoO.aT.", RecordType::A).await;

    assert_eq!(
        response
            .answers
            .first()
            .map(|record| record.name.to_ascii()),
        Some("NaS.MiLoO.aT.".to_owned())
    );
    assert_eq!(server.upstream_hits(), 0);
}

#[tokio::test]
async fn a_reverse_zone_answers_ptr() {
    // `dig -x 192.168.1.5` braucht eine eigene Reverse-Zone — aus den
    // A-Records entsteht sie bewusst nicht.
    let server = start(
        r#"
[[local_zone]]
zone = "1.168.192.in-addr.arpa"
records = [{ name = "5", type = "PTR", value = "nas.miloo.at." }]
"#,
        &[],
    )
    .await;
    let response = server
        .ask("5.1.168.192.in-addr.arpa.", RecordType::PTR)
        .await;

    assert_eq!(response.metadata.response_code, ResponseCode::NoError);
    assert_eq!(response.answers.len(), 1);
    assert_eq!(server.upstream_hits(), 0);
}

#[tokio::test]
async fn the_answer_claims_no_authority() {
    let server = start(SPLIT_HORIZON, &[]).await;
    let response = server.ask("nas.miloo.at.", RecordType::A).await;

    assert!(
        !response.metadata.authoritative,
        "AA wäre eine Autorität, die AlpenDNS nicht hat"
    );
    assert!(response.metadata.recursion_available);
    assert_eq!(response.metadata.id, 0x4242);
}
