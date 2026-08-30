//! Logging-Modi, API und Live-Strom.
//!
//! Der wichtigste Test hier ist [`no_query_name_leaves_the_process_in_the_quiet_modes`]:
//! er ist der automatisierte Nachweis für das zentrale Versprechen des Projekts
//! (ADR-0004, B.1 Regel 3).

// Testcode darf panicken, siehe B.1 und clippy.toml.
#![allow(
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic,
    clippy::integer_division
)]

use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use alpendns::api::{ApiState, HistoryView, ListInfo, PolicyInfo, StatusSource};
use alpendns::cache::Stats as CacheStats;
use alpendns::config::LoggingConfig;
use alpendns::history::{History, Sample};
use alpendns::logging::{BlockReason, Mode, QueryEvent, QueryLog};
use alpendns::metrics::Snapshot;
use alpendns::policy::{Explanation, PolicyStats};
use alpendns::upstream::pool::UpstreamStats;
use hickory_proto::op::ResponseCode;

/// Der Name, nach dem in jeder Ausgabe gesucht wird.
const SECRET: &str = "sehr-verraeterisch.example.com";

struct TempDir(PathBuf);

impl TempDir {
    fn new(label: &str) -> Self {
        use std::hash::{BuildHasher as _, RandomState};
        let path = std::env::temp_dir().join(format!(
            "alpendns-obs-{label}-{}-{}",
            std::process::id(),
            RandomState::new().hash_one(label)
        ));
        std::fs::create_dir_all(&path).expect("Testverzeichnis");
        Self(path)
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn config(mode: Mode, path: PathBuf) -> LoggingConfig {
    LoggingConfig {
        mode,
        ring_seconds: Duration::from_secs(300),
        aggregate_k: 5,
        path,
    }
}

fn event(name: &str, blocked: bool) -> QueryEvent {
    QueryEvent {
        name: name.to_owned(),
        query_type: "A".to_owned(),
        client: Arc::from("kids-tablet"),
        blocked,
        reason: blocked.then_some(BlockReason::Blocklist),
        rcode: if blocked {
            ResponseCode::NXDomain
        } else {
            ResponseCode::NoError
        },
        why: vec![
            "Client 'kids-tablet' über die Quelladresse erkannt".to_owned(),
            format!("Blockliste 'test' Zeile 1: '{name}'"),
        ],
        elapsed: Duration::from_micros(300),
        // Seit Phase 8 tragen die Detektor-Begründungen den Query-Namen. Genau
        // deshalb steht hier einer: der Leck-Test unten sucht ihn in allem, was
        // der Prozess ausgeben kann.
        findings: vec![alpendns::logging::Flagged {
            detector: "dga",
            label: "Algorithmisch erzeugter Name",
            score: "0.930".to_owned(),
            action: "flag",
            reason: format!("'{name}' passt nicht zu gewachsenen Namen"),
        }],
    }
}

// ---------------------------------------------------------------------------
// Die vier Modi
// ---------------------------------------------------------------------------

#[test]
fn the_none_mode_keeps_only_counters() {
    let dir = TempDir::new("none");
    let log = QueryLog::new(&config(Mode::None, dir.0.join("q.jsonl"))).expect("QueryLog");
    for _ in 0..20 {
        log.record(&event(SECRET, true));
    }

    let stats = log.stats();
    assert_eq!(stats.queries, 20, "Zähler fehlen");
    assert_eq!(stats.blocked, 20);
    assert!(log.recent(100).is_empty());
    assert!(log.top_domains(100).is_empty(), "Namen trotz Modus none");
    assert!(!dir.0.join("q.jsonl").exists(), "Datei trotz Modus none");
}

#[test]
fn the_aggregate_mode_reports_only_above_the_threshold() {
    let dir = TempDir::new("aggregate");
    let log = QueryLog::new(&config(Mode::Aggregate, dir.0.join("q.jsonl"))).expect("QueryLog");

    // Vier Treffer: unter der Schwelle von fünf.
    for _ in 0..4 {
        log.record(&event("selten.example.com", false));
    }
    for _ in 0..40 {
        log.record(&event("haeufig.example.com", false));
    }

    let top: Vec<String> = log.top_domains(100).into_iter().map(|d| d.name).collect();
    assert!(top.contains(&"haeufig.example.com".to_owned()), "{top:?}");
    assert!(
        !top.contains(&"selten.example.com".to_owned()),
        "eine Domain unter der k-Schwelle wurde gemeldet: {top:?}"
    );
    assert!(
        log.recent(100).is_empty(),
        "aggregate führt keinen Ringpuffer"
    );
}

#[test]
fn the_ring_mode_keeps_names_in_memory_only() {
    let dir = TempDir::new("ring");
    let log = QueryLog::new(&config(Mode::Ring, dir.0.join("q.jsonl"))).expect("QueryLog");
    log.record(&event(SECRET, true));

    let recent = log.recent(10);
    assert_eq!(recent.len(), 1);
    assert_eq!(recent.first().map(|e| e.name.as_str()), Some(SECRET));
    assert!(
        !recent.first().expect("Eintrag").why.is_empty(),
        "die Begründung fehlt im Ringpuffer"
    );
    assert!(
        !dir.0.join("q.jsonl").exists(),
        "der ring-Modus hat auf Platte geschrieben"
    );
}

#[test]
fn the_full_mode_also_writes_a_file() {
    let dir = TempDir::new("full");
    let path = dir.0.join("q.jsonl");
    let log = QueryLog::new(&config(Mode::Full, path.clone())).expect("QueryLog");
    log.record(&event(SECRET, true));
    drop(log);

    let text = std::fs::read_to_string(&path).expect("Datei");
    assert!(text.contains(SECRET), "der Name fehlt in der Datei");
    let line: serde_json::Value =
        serde_json::from_str(text.lines().next().expect("eine Zeile")).expect("gültiges JSON");
    assert_eq!(line["name"], SECRET);
    assert_eq!(line["blocked"], true);
    assert_eq!(line["client"], "kids-tablet");
}

/// Der automatisierte Nachweis für das zentrale Versprechen des Projekts.
///
/// Seit Phase 8 durchsucht er auch die Begründungen der Heuristiken. Die
/// enthalten den Query-Namen — sie sollen ihn enthalten, sonst wäre ein
/// Fehlalarm nicht nachvollziehbar (FEATURES.md D6) — und sind damit die
/// neueste Stelle, an der einer entkommen könnte.
#[test]
fn no_query_name_leaves_the_process_in_the_quiet_modes() {
    for mode in [Mode::None, Mode::Aggregate] {
        let dir = TempDir::new("leak");
        let path = dir.0.join("q.jsonl");
        let log = QueryLog::new(&config(mode, path.clone())).expect("QueryLog");

        // Einmal gefragt — genau das ist der verräterische Fall.
        log.record(&event(SECRET, true));

        // Alles zusammentragen, was der Prozess nach außen geben kann.
        let mut output = String::new();
        output.push_str(&format!("{:?}", log.stats()));
        output.push_str(&serde_json::to_string(&log.top_domains(1000)).expect("JSON"));
        output.push_str(&serde_json::to_string(&log.recent(1000)).expect("JSON"));
        // Seit Phase 8 dazugekommen: die Liste der auffälligen Anfragen. Sie
        // kommt aus demselben Ringpuffer und muss deshalb denselben Regeln
        // folgen — der Test hält fest, dass sie es tut, statt sich darauf zu
        // verlassen.
        output.push_str(&serde_json::to_string(&log.flagged(1000)).expect("JSON"));
        if let Ok(file) = std::fs::read_to_string(&path) {
            output.push_str(&file);
        }

        assert!(
            !output.contains(SECRET),
            "im Modus {mode:?} tauchte der Query-Name in der Ausgabe auf:\n{output}"
        );
        // Der Puls muss trotzdem sichtbar sein.
        assert_eq!(log.stats().queries, 1, "im Modus {mode:?} fehlt der Zähler");
    }
}

#[tokio::test]
async fn the_live_stream_carries_no_names_in_the_quiet_modes() {
    // Roadmap Schritt 5: "bei Log-Modus none kommen nur Zähler".
    let dir = TempDir::new("stream");
    let log = QueryLog::new(&config(Mode::Aggregate, dir.0.join("q.jsonl"))).expect("QueryLog");
    let mut stream = log.subscribe();

    log.record(&event(SECRET, true));
    let received = stream.recv().await.expect("Ereignis");
    let json = serde_json::to_string(&received).expect("JSON");

    assert!(!json.contains(SECRET), "Name im Live-Strom: {json}");
    assert!(json.contains("\"blocked\":true"), "{json}");
    assert!(json.contains("\"rcode\""), "{json}");
}

/// Im Modus `ring` ist der Fund da, samt Begründung.
///
/// Das Gegenstück zum Leck-Test: er zeigt, dass die Zurückhaltung wirkt, dieser
/// hier, dass sie nicht alles wegnimmt. Ohne ihn könnte `flagged()` einfach
/// immer leer zurückgeben und beide Tests wären grün.
#[test]
fn the_flagged_list_carries_the_finding_in_the_ring_mode() {
    let dir = TempDir::new("flagged-ring");
    let log = QueryLog::new(&config(Mode::Ring, dir.0.join("q.jsonl"))).expect("QueryLog");
    log.record(&event(SECRET, true));
    log.record(&event("unauffaellig.example.com", false));

    let flagged = log.flagged(50);
    assert_eq!(flagged.len(), 2, "beide Ereignisse tragen einen Fund");

    let json = serde_json::to_string(&flagged).expect("JSON");
    assert!(json.contains(SECRET), "{json}");
    assert!(json.contains("Algorithmisch erzeugter Name"), "{json}");
    assert!(json.contains("0.930"), "{json}");
}

/// Anfragen ohne Fund stehen nicht in der Liste der auffälligen.
#[test]
fn the_flagged_list_holds_only_what_was_flagged() {
    let dir = TempDir::new("flagged-only");
    let log = QueryLog::new(&config(Mode::Ring, dir.0.join("q.jsonl"))).expect("QueryLog");
    let mut plain = event("gewoehnlich.example.com", false);
    plain.findings.clear();
    log.record(&plain);

    assert_eq!(log.recent(50).len(), 1, "die Anfrage fehlt im Protokoll");
    assert!(
        log.flagged(50).is_empty(),
        "eine Anfrage ohne Fund steht in der Liste der auffälligen"
    );
}

#[tokio::test]
async fn the_live_stream_carries_names_in_the_ring_mode() {
    let dir = TempDir::new("stream-ring");
    let log = QueryLog::new(&config(Mode::Ring, dir.0.join("q.jsonl"))).expect("QueryLog");
    let mut stream = log.subscribe();

    log.record(&event(SECRET, true));
    let received = stream.recv().await.expect("Ereignis");
    let json = serde_json::to_string(&received).expect("JSON");
    assert!(json.contains(SECRET), "{json}");
    assert!(json.contains("Blockliste"), "die Begründung fehlt: {json}");
}

// ---------------------------------------------------------------------------
// API
// ---------------------------------------------------------------------------

struct Fake {
    log: Arc<QueryLog>,
    granted: std::sync::Mutex<Vec<(String, Duration)>>,
    denied: std::sync::Mutex<Vec<(String, Duration)>>,
}

impl StatusSource for Fake {
    fn snapshot(&self) -> Snapshot {
        Snapshot {
            log: self.log.stats(),
            cache: CacheStats {
                hits: 6,
                stale_hits: 0,
                misses: 4,
                inserts: 4,
            },
            cache_entries: 4,
            policy: PolicyStats {
                blocked: 1,
                allowed: 0,
                passed: 9,
                entries: 79_747,
            },
            upstreams: vec![UpstreamStats {
                name: "quad9".to_owned(),
                scheme: "dot",
                successes: 4,
                failures: 0,
                rtt: Some(Duration::from_millis(34)),
                down: false,
            }],
            uptime: Duration::from_secs(42),
            blocking_mode: alpendns::filter::block::BlockMode::Nxdomain,
            logging_mode: self.log.mode(),
            list_formats: vec![("hosts".to_owned(), 1)],
            privacy: alpendns::privacy::counters(),
            aggregate_k: self.log.aggregate_k(),
            below_threshold_queries: self.log.top(0).below_threshold_queries,
            zone_seed_rotation: Duration::from_secs(24 * 60 * 60),
            zone_seed_rotations: 0,
            dnssec_enabled: true,
            dnssec: alpendns::dnssec::counters(),
            detectors: Vec::new(),
            detections: alpendns::detect::counters(),
            rate_limit: None,
        }
    }
    fn lists(&self) -> Vec<ListInfo> {
        vec![ListInfo {
            name: "stevenblack".to_owned(),
            entries: 79_747,
            format: "hosts".to_owned(),
        }]
    }
    fn policies(&self) -> Vec<PolicyInfo> {
        vec![PolicyInfo {
            name: "default".to_owned(),
            blocklists: vec!["stevenblack".to_owned()],
            allowlists: Vec::new(),
            regex: 0,
            schedules: Vec::new(),
            clients: Vec::new(),
        }]
    }
    fn grant(&self, domain: &str, ttl: Duration) {
        self.granted
            .lock()
            .expect("Lock")
            .push((domain.to_owned(), ttl));
    }
    fn revoke(&self, domain: &str) {
        self.granted
            .lock()
            .expect("Lock")
            .retain(|(name, _)| name != domain);
    }
    fn deny(&self, domain: &str, ttl: Duration) {
        self.denied
            .lock()
            .expect("Lock")
            .push((domain.to_owned(), ttl));
    }

    fn undeny(&self, domain: &str) {
        self.denied
            .lock()
            .expect("Lock")
            .retain(|(name, _)| name != domain);
    }

    fn denials(&self) -> Vec<(String, Duration)> {
        self.denied.lock().expect("Lock").clone()
    }

    fn grants(&self) -> Vec<(String, Duration)> {
        self.granted.lock().expect("Lock").clone()
    }
    fn history(&self) -> HistoryView {
        let now = std::time::Instant::now();
        let mut history = History::new(now);
        history.observe(sample(0), &["quad9".to_owned()], now);
        history.observe(sample(10), &["quad9".to_owned()], now);
        HistoryView {
            bucket_seconds: history.bucket_seconds(),
            upstreams: history.upstreams().to_vec(),
            buckets: history.buckets(),
        }
    }
    fn explain(&self, domain: &str, client: Option<&str>) -> Result<Explanation, String> {
        if domain.is_empty() {
            return Err("leerer Name".to_owned());
        }
        Ok(Explanation {
            domain: domain.to_owned(),
            client: client.unwrap_or("default").to_owned(),
            blocked: true,
            steps: vec![format!("Blockliste 'test' Zeile 1: '{domain}'")],
        })
    }
}

fn sample(queries: u64) -> Sample {
    Sample {
        queries,
        blocked: queries / 2,
        cache_hits: queries,
        cache_misses: 0,
        upstreams: vec![queries],
    }
}

struct Api {
    base: String,
    client: reqwest::Client,
    source: Arc<Fake>,
}

async fn start_api(mode: Mode) -> (Api, TempDir) {
    // reqwest ist mit `rustls-no-provider` gebaut und nimmt den Prozess-Default.
    // Im Betrieb installiert ihn der Listen-Loader; hier müssen wir es selbst tun.
    alpendns::filter::source::install_crypto_provider();
    let dir = TempDir::new("api");
    let log = Arc::new(QueryLog::new(&config(mode, dir.0.join("q.jsonl"))).expect("QueryLog"));
    let source = Arc::new(Fake {
        log: Arc::clone(&log),
        granted: std::sync::Mutex::new(Vec::new()),
        denied: std::sync::Mutex::new(Vec::new()),
    });
    let state = ApiState::new(
        Arc::clone(&source) as Arc<dyn StatusSource>,
        log,
        Arc::from("test-token"),
    );

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind");
    let addr: SocketAddr = listener.local_addr().expect("local_addr");
    tokio::spawn(async move {
        let _ = axum::serve(listener, alpendns::api::router(state)).await;
    });

    (
        Api {
            base: format!("http://{addr}"),
            client: reqwest::Client::new(),
            source,
        },
        dir,
    )
}

impl Api {
    async fn get(&self, path: &str) -> reqwest::Response {
        self.client
            .get(format!("{}{path}", self.base))
            .bearer_auth("test-token")
            .send()
            .await
            .expect("Anfrage")
    }

    async fn get_without_token(&self, path: &str) -> reqwest::Response {
        self.client
            .get(format!("{}{path}", self.base))
            .send()
            .await
            .expect("Anfrage")
    }
}

#[tokio::test]
async fn every_api_endpoint_refuses_without_a_token() {
    // Roadmap Schritt 1.
    let (api, _dir) = start_api(Mode::Ring).await;
    for path in [
        "/api/status",
        "/api/top",
        "/api/history",
        "/api/explain?domain=x.example",
        "/api/recent",
        "/api/lists",
        "/api/policies",
        "/api/allow",
        "/api/events",
    ] {
        let response = api.get_without_token(path).await;
        assert_eq!(
            response.status(),
            reqwest::StatusCode::UNAUTHORIZED,
            "{path} war ohne Token erreichbar"
        );
    }
}

#[tokio::test]
async fn a_wrong_token_is_refused() {
    let (api, _dir) = start_api(Mode::Ring).await;
    let response = api
        .client
        .get(format!("{}/api/status", api.base))
        .bearer_auth("falsch")
        .send()
        .await
        .expect("Anfrage");
    assert_eq!(response.status(), reqwest::StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn every_api_endpoint_answers_with_a_token() {
    let (api, _dir) = start_api(Mode::Ring).await;
    for path in [
        "/api/status",
        "/api/top",
        "/api/history",
        "/api/explain?domain=x.example",
        "/api/recent",
        "/api/lists",
        "/api/policies",
        "/api/allow",
    ] {
        let response = api.get(path).await;
        assert!(
            response.status().is_success(),
            "{path}: {}",
            response.status()
        );
        let body: serde_json::Value = response.json().await.expect("JSON");
        assert!(!body.is_null(), "{path} lieferte null");
    }
}

#[tokio::test]
async fn the_status_endpoint_reports_the_logging_mode() {
    // ADR-0004: der aktive Modus wird dauerhaft angezeigt, nicht versteckt.
    let (api, _dir) = start_api(Mode::Aggregate).await;
    let body: serde_json::Value = api.get("/api/status").await.json().await.expect("JSON");
    assert_eq!(body["logging_mode"], "aggregate");
    assert_eq!(body["list_entries"], 79_747);
    assert_eq!(body["upstreams"][0]["name"], "quad9");
}

#[tokio::test]
async fn a_temporary_grant_can_be_created_and_revoked_over_the_api() {
    // Der offene Punkt aus Phase 5, Schritt 5.
    let (api, _dir) = start_api(Mode::Ring).await;

    let response = api
        .client
        .post(format!("{}/api/allow", api.base))
        .bearer_auth("test-token")
        .json(&serde_json::json!({ "domain": "Gesperrt.Example.", "seconds": 60 }))
        .send()
        .await
        .expect("Anfrage");
    assert!(response.status().is_success(), "{}", response.status());
    let body: serde_json::Value = response.json().await.expect("JSON");
    assert_eq!(body["domain"], "gesperrt.example", "nicht normalisiert");
    assert_eq!(body["remaining_seconds"], 60);
    assert_eq!(api.source.grants().len(), 1);

    let response = api
        .client
        .delete(format!("{}/api/allow/gesperrt.example", api.base))
        .bearer_auth("test-token")
        .send()
        .await
        .expect("Anfrage");
    assert_eq!(response.status(), reqwest::StatusCode::NO_CONTENT);
    assert!(
        api.source.grants().is_empty(),
        "die Freigabe blieb bestehen"
    );
}

#[tokio::test]
async fn a_grant_is_capped_and_validated() {
    let (api, _dir) = start_api(Mode::Ring).await;

    // Länger als einen Tag gibt es nicht.
    let body: serde_json::Value = api
        .client
        .post(format!("{}/api/allow", api.base))
        .bearer_auth("test-token")
        .json(&serde_json::json!({ "domain": "x.example", "seconds": 999_999 }))
        .send()
        .await
        .expect("Anfrage")
        .json()
        .await
        .expect("JSON");
    assert_eq!(body["remaining_seconds"], 86_400);

    for bad in [
        serde_json::json!({ "domain": "", "seconds": 60 }),
        serde_json::json!({ "domain": "mit leerzeichen", "seconds": 60 }),
        serde_json::json!({ "domain": "x.example", "seconds": 0 }),
    ] {
        let response = api
            .client
            .post(format!("{}/api/allow", api.base))
            .bearer_auth("test-token")
            .json(&bad)
            .send()
            .await
            .expect("Anfrage");
        assert_eq!(
            response.status(),
            reqwest::StatusCode::BAD_REQUEST,
            "{bad} wurde angenommen"
        );
    }
}

#[tokio::test]
async fn the_ui_is_served_without_a_token() {
    // Die Seite ist statisch und enthält keine Daten; den Token fragt sie selbst ab.
    let (api, _dir) = start_api(Mode::Ring).await;
    for (path, expected) in [
        ("/", "text/html"),
        ("/app.css", "text/css"),
        ("/app.js", "text/javascript"),
    ] {
        let response = api.get_without_token(path).await;
        assert!(
            response.status().is_success(),
            "{path}: {}",
            response.status()
        );
        let content_type = response
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .unwrap_or_default()
            .to_owned();
        assert!(content_type.starts_with(expected), "{path}: {content_type}");
    }
}

#[tokio::test]
async fn the_event_stream_accepts_the_token_in_the_url() {
    // EventSource im Browser kann keine Header setzen.
    let (api, _dir) = start_api(Mode::Ring).await;
    let response = api.get_without_token("/api/events?token=test-token").await;
    assert!(response.status().is_success(), "{}", response.status());
    assert_eq!(
        response
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok()),
        Some("text/event-stream")
    );
}

#[tokio::test]
async fn the_metrics_endpoint_needs_no_token_and_names_nothing() {
    alpendns::filter::source::install_crypto_provider();
    let dir = TempDir::new("metrics");
    let log =
        Arc::new(QueryLog::new(&config(Mode::Ring, dir.0.join("q.jsonl"))).expect("QueryLog"));
    log.record(&event(SECRET, true));
    let source = Arc::new(Fake {
        log: Arc::clone(&log),
        granted: std::sync::Mutex::new(Vec::new()),
        denied: std::sync::Mutex::new(Vec::new()),
    });
    let state = ApiState::new(source as Arc<dyn StatusSource>, log, Arc::from(""));

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind");
    let addr: SocketAddr = listener.local_addr().expect("local_addr");
    tokio::spawn(async move {
        let _ = axum::serve(listener, alpendns::api::metrics_router(state, "/metrics")).await;
    });

    let text = reqwest::get(format!("http://{addr}/metrics"))
        .await
        .expect("Anfrage")
        .text()
        .await
        .expect("Text");

    assert!(text.contains("alpendns_queries_total 1"), "{text}");
    assert!(
        !text.contains(SECRET),
        "der Metrik-Endpunkt nennt einen Query-Namen:\n{text}"
    );
}

// ---------------------------------------------------------------------------
// Metriken
// ---------------------------------------------------------------------------

/// Kein Prometheus-Label trägt einen Domain- oder Client-Namen.
///
/// Der Unterschied zu den Tests in `metrics.rs`: dort steht eine gebaute
/// Momentaufnahme, hier läuft echter Verkehr durch das Query-Log, bevor die
/// Ausgabe gerendert wird. Prometheus behält jede Zeitreihe für immer — ein
/// Name in einem Label wäre ein Query-Log, das keine Retention kennt und das
/// niemand als solches erkennt.
#[tokio::test]
async fn no_metric_label_carries_a_domain_or_client_name() {
    for mode in [Mode::None, Mode::Aggregate, Mode::Ring, Mode::Full] {
        let dir = TempDir::new("metrics-leak");
        let log = Arc::new(QueryLog::new(&config(mode, dir.0.join("q.jsonl"))).expect("QueryLog"));
        let source = Fake {
            log: Arc::clone(&log),
            granted: std::sync::Mutex::new(Vec::new()),
            denied: std::sync::Mutex::new(Vec::new()),
        };

        // Genug Treffer, um auch die k-Schwelle zu überschreiten: gerade der
        // häufige Name ist der, den eine Statistik gern ausplaudert.
        for _ in 0..50 {
            log.record(&event(SECRET, true));
        }
        log.record(&event("einmalig.example.org", false));

        let text = alpendns::metrics::render(&source.snapshot());
        assert!(
            !text.contains(SECRET),
            "im Modus {mode:?} steht der Query-Name in den Metriken:\n{text}"
        );
        assert!(
            !text.contains("einmalig.example.org"),
            "im Modus {mode:?} steht ein seltener Name in den Metriken:\n{text}"
        );
        assert!(
            !text.contains("kids-tablet"),
            "im Modus {mode:?} steht der Client-Name in den Metriken:\n{text}"
        );

        // Der Puls muss trotzdem in der Ausgabe stehen, sonst prüft der Test
        // bloß, dass die Datei leer ist.
        assert!(text.contains("alpendns_queries_total 51"), "{text}");
        assert!(
            text.contains("alpendns_blocked_by_reason_total{reason=\"blocklist\"} 50"),
            "{text}"
        );
    }
}

// ---------------------------------------------------------------------------
// k-Schwelle in der API
// ---------------------------------------------------------------------------

#[tokio::test]
async fn the_top_list_aggregates_everything_below_the_threshold() {
    // ADR-0004: was unter der Schwelle bleibt, verschwindet nicht — es wird zu
    // einer Zahl ohne Namen. Sonst sähe ein Server mit viel seltenem Verkehr
    // aus wie einer ohne Verkehr.
    let (api, _dir) = start_api(Mode::Aggregate).await;

    for _ in 0..12 {
        api.source.log.record(&event("haeufig.example.com", false));
    }
    // Drei verschiedene Namen mit je zwei Treffern: alle unter k = 5.
    for index in 0..3 {
        for _ in 0..2 {
            api.source
                .log
                .record(&event(&format!("selten{index}.example.com"), false));
        }
    }

    let body: serde_json::Value = api.get("/api/top").await.json().await.expect("JSON");
    assert_eq!(body["threshold"], 5);
    assert_eq!(body["domains"][0]["name"], "haeufig.example.com");
    assert_eq!(body["domains"][0]["count"], 12);
    assert_eq!(body["domains"][1], serde_json::Value::Null, "{body}");
    assert_eq!(body["below_threshold_queries"], 6);
    assert_eq!(body["below_threshold_names"], 3);

    let text = body.to_string();
    for index in 0..3 {
        assert!(
            !text.contains(&format!("selten{index}")),
            "ein Name unter der Schwelle wurde ausgeliefert: {text}"
        );
    }
}

#[tokio::test]
async fn the_status_endpoint_states_what_the_server_keeps() {
    // Der Privacy-Badge im Kopf der UI lebt von diesen drei Feldern.
    let (api, _dir) = start_api(Mode::Aggregate).await;
    let body: serde_json::Value = api.get("/api/status").await.json().await.expect("JSON");
    assert_eq!(body["logging_mode"], "aggregate");
    assert_eq!(body["aggregate_k"], 5);
    assert_eq!(body["persists_to_disk"], false);
    assert_eq!(body["upstreams"][0]["transport"], "dot");
    assert!(body["block_reasons"].is_array(), "{body}");
    assert!(body["privacy"]["ecs_stripped"].is_u64(), "{body}");
}

#[tokio::test]
async fn the_full_mode_admits_that_it_writes_to_disk() {
    // Die UI darf nicht "nur im RAM" behaupten, während eine Datei mitläuft.
    let (api, _dir) = start_api(Mode::Full).await;
    let body: serde_json::Value = api.get("/api/status").await.json().await.expect("JSON");
    assert_eq!(body["persists_to_disk"], true);
}

#[tokio::test]
async fn the_history_is_made_of_counters_only() {
    let (api, _dir) = start_api(Mode::Ring).await;
    api.source.log.record(&event(SECRET, true));

    let response = api.get("/api/history").await;
    let text = response.text().await.expect("Text");
    assert!(!text.contains(SECRET), "Name in der Zeitreihe: {text}");
    let body: serde_json::Value = serde_json::from_str(&text).expect("JSON");
    assert_eq!(body["bucket_seconds"], 300);
    assert_eq!(body["upstreams"][0], "quad9");
    assert!(body["buckets"][0]["queries"].is_u64(), "{body}");
}

#[tokio::test]
async fn a_row_can_be_explained_over_the_api() {
    // Punkt 6: der Klick auf eine Zeile fragt dieselbe Auswertung wie
    // `alpendns policy test`.
    let (api, _dir) = start_api(Mode::Ring).await;
    let body: serde_json::Value = api
        .get("/api/explain?domain=ads.example.com&client=kids-tablet")
        .await
        .json()
        .await
        .expect("JSON");
    assert_eq!(body["domain"], "ads.example.com");
    assert_eq!(body["client"], "kids-tablet");
    assert_eq!(body["blocked"], true);
    assert!(
        body["steps"][0]
            .as_str()
            .is_some_and(|s| s.contains("Blockliste")),
        "{body}"
    );

    let response = api.get("/api/explain?domain=").await;
    assert_eq!(response.status(), reqwest::StatusCode::BAD_REQUEST);
}

// ---------------------------------------------------------------------------
// Der Live-Strom unter Last
// ---------------------------------------------------------------------------

/// Ein Lasttest darf die Oberfläche nicht lahmlegen.
///
/// Vorher schickte der Server eine Nachricht je Anfrage. Bei einem `dnsperf`-Lauf
/// sind das zehntausende JSON-Frames pro Sekunde: der Browser hängt, und der
/// Server verbrennt fürs Formatieren Rechenzeit, die er zum Auflösen braucht.
/// Die Statistik bleibt davon unberührt — gedeckelt ist nur der Strom.
#[tokio::test]
async fn a_burst_of_queries_does_not_flood_the_live_stream() {
    let dir = TempDir::new("burst");
    let log = QueryLog::new(&config(Mode::Ring, dir.0.join("q.jsonl"))).expect("QueryLog");
    let mut stream = log.subscribe();

    const BURST: u64 = 20_000;
    for _ in 0..BURST {
        log.record(&event(SECRET, false));
    }

    let mut received = Vec::new();
    while let Ok(message) = stream.try_recv() {
        received.push(message);
    }

    // Der Puffer des Kanals fasst 256; ohne Deckel wäre er längst übergelaufen
    // und der Zuhörer hätte Ereignisse verloren, ohne es zu merken.
    assert!(
        received.len() <= 64,
        "der Strom schickte {} Nachrichten für {BURST} Anfragen",
        received.len()
    );
    assert!(!received.is_empty(), "der Strom ist ganz verstummt");

    // Nichts geht verloren, was den Puls betrifft: die Zähler stehen vollständig,
    // und die erste Nachricht der nächsten Sekunde trägt die Ausgelassenen nach.
    assert_eq!(log.stats().queries, BURST);
    assert!(
        received.iter().all(|message| message.name.is_some()),
        "im Modus ring gehören Namen in den Strom"
    );
}
