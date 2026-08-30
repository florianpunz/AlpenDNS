//! Logging-Modi, API und Live-Strom.
//!
//! Der wichtigste Test hier ist [`no_query_name_leaves_the_process_in_the_quiet_modes`]:
//! er ist der automatisierte Nachweis für das zentrale Versprechen des Projekts
//! (ADR-0004, B.1 Regel 3).

// Testcode darf panicken, siehe B.1 und clippy.toml.
#![allow(clippy::expect_used, clippy::indexing_slicing, clippy::panic)]

use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use alpendns::api::{ApiState, ListInfo, PolicyInfo, StatusSource};
use alpendns::cache::Stats as CacheStats;
use alpendns::config::LoggingConfig;
use alpendns::logging::{Mode, QueryEvent, QueryLog};
use alpendns::metrics::Snapshot;
use alpendns::policy::PolicyStats;
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
                successes: 4,
                failures: 0,
                rtt: Some(Duration::from_millis(34)),
                down: false,
            }],
            uptime: Duration::from_secs(42),
            blocking_mode: alpendns::filter::block::BlockMode::Nxdomain,
            logging_mode: self.log.mode(),
            list_formats: vec![("hosts".to_owned(), 1)],
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
    fn grants(&self) -> Vec<(String, Duration)> {
        self.granted.lock().expect("Lock").clone()
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
