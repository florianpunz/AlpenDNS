//! HTTP-API und Auslieferung der Web-UI.
//!
//! Zwei getrennte Listener, weil sie verschiedene Zielgruppen haben:
//!
//! * die **API** auf `api.listen`, mit Token — sie zeigt Namen und Begründungen,
//! * die **Metriken** auf `metrics.listen`, ohne Token — ein Prometheus-Scraper
//!   schickt keinen Bearer-Header, und die Ausgabe enthält per Konstruktion
//!   keine Namen (siehe [`crate::metrics`]).
//!
//! Das API-Modul kennt die konkreten Typen der Pipeline nicht, sondern nur
//! [`StatusSource`]. Sonst wäre der ganze Router generisch über Cache-, Uhr- und
//! Backend-Typ.

mod ui;

use std::sync::Arc;
use std::time::Duration;

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::sse::{Event, Sse};
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, get};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};

use crate::logging::{LoggedQuery, QueryLog, TopDomain};
use crate::metrics::Snapshot;

/// Was die API über den laufenden Server erfährt.
///
/// Wird vom Binary implementiert, das die konkreten Typen kennt.
pub trait StatusSource: Send + Sync + 'static {
    fn snapshot(&self) -> Snapshot;
    fn lists(&self) -> Vec<ListInfo>;
    fn policies(&self) -> Vec<PolicyInfo>;
    fn grant(&self, domain: &str, ttl: Duration);
    fn revoke(&self, domain: &str);
    fn grants(&self) -> Vec<(String, Duration)>;
}

/// Eine geladene Liste.
#[derive(Debug, Clone, Serialize)]
pub struct ListInfo {
    pub name: String,
    pub entries: usize,
    /// In welchem Format sie gelesen wurde. Steht hier, damit die Metrik
    /// belegen kann, welche Formate im Feld tatsächlich vorkommen.
    pub format: String,
}

/// Eine konfigurierte Policy mit den Clients, für die sie gilt.
#[derive(Debug, Clone, Serialize)]
pub struct PolicyInfo {
    pub name: String,
    pub blocklists: Vec<String>,
    pub allowlists: Vec<String>,
    pub regex: usize,
    pub schedules: Vec<String>,
    pub clients: Vec<String>,
}

#[derive(Clone)]
pub struct ApiState {
    source: Arc<dyn StatusSource>,
    log: Arc<QueryLog>,
    token: Arc<str>,
}

impl std::fmt::Debug for ApiState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Ohne den Token: er hat in keinem Log etwas zu suchen.
        f.debug_struct("ApiState").finish_non_exhaustive()
    }
}

impl ApiState {
    pub fn new(source: Arc<dyn StatusSource>, log: Arc<QueryLog>, token: Arc<str>) -> Self {
        Self { source, log, token }
    }
}

/// Der Router für die API samt UI.
pub fn router(state: ApiState) -> Router {
    Router::new()
        .route("/api/status", get(status))
        .route("/api/top", get(top))
        .route("/api/recent", get(recent))
        .route("/api/lists", get(lists))
        .route("/api/policies", get(policies))
        .route("/api/allow", get(grants).post(grant))
        .route("/api/allow/{domain}", delete(revoke))
        .route("/api/events", get(events))
        // Die UI selbst braucht keinen Token: sie ist eine statische Datei und
        // fragt ihn beim ersten Aufruf ab, um ihn dann mitzuschicken.
        .layer(axum::middleware::from_fn_with_state(
            state.clone(),
            authenticate,
        ))
        .merge(ui::router())
        .with_state(state)
}

/// Der Router für den Metrik-Endpunkt, ohne Token.
pub fn metrics_router(state: ApiState, path: &str) -> Router {
    Router::new().route(path, get(metrics)).with_state(state)
}

/// Prüft den Token aus dem Header oder — für Server-Sent Events — aus der URL.
///
/// `EventSource` im Browser kann keine Header setzen. Deshalb ist der Token dort
/// auch als Query-Parameter zulässig. Das ist ein Zugeständnis: er landet damit
/// womöglich in einem Proxy-Log. Beides ohne Token zuzulassen wäre schlechter.
async fn authenticate(
    State(state): State<ApiState>,
    request: axum::extract::Request,
    next: axum::middleware::Next,
) -> Response {
    let from_header = request
        .headers()
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "))
        .map(str::to_owned);
    let from_query = request.uri().query().and_then(|query| {
        query.split('&').find_map(|pair| {
            pair.strip_prefix("token=")
                .map(|value| value.replace("%3D", "="))
        })
    });

    let presented = from_header.or(from_query);
    // Vergleich in konstanter Zeit: ein Zeichen-für-Zeichen-Abbruch verrät
    // sonst über die Antwortzeit, wie weit ein Rateversuch gekommen ist.
    let ok = presented.is_some_and(|value| constant_time_eq(&value, &state.token));
    if !ok {
        return (
            StatusCode::UNAUTHORIZED,
            [(axum::http::header::WWW_AUTHENTICATE, "Bearer")],
            Json(ApiError {
                error: "Token fehlt oder ist falsch".to_owned(),
            }),
        )
            .into_response();
    }
    next.run(request).await
}

fn constant_time_eq(a: &str, b: &str) -> bool {
    if a.len() != b.len() {
        return false;
    }
    a.bytes()
        .zip(b.bytes())
        .fold(0_u8, |acc, (x, y)| acc | (x ^ y))
        == 0
}

#[derive(Debug, Serialize)]
struct ApiError {
    error: String,
}

#[derive(Debug, Deserialize)]
struct Limit {
    limit: Option<usize>,
}

impl Limit {
    /// Nie mehr als 500 Einträge auf einmal.
    fn get(&self) -> usize {
        self.limit.unwrap_or(50).min(500)
    }
}

#[derive(Debug, Serialize)]
struct Status {
    version: &'static str,
    uptime_seconds: u64,
    logging_mode: String,
    queries: u64,
    blocked: u64,
    block_rate: f64,
    cache_hit_rate: f64,
    cache_entries: usize,
    list_entries: usize,
    upstreams: Vec<UpstreamInfo>,
}

#[derive(Debug, Serialize)]
struct UpstreamInfo {
    name: String,
    ok: u64,
    failed: u64,
    rtt_ms: Option<f64>,
    down: bool,
}

async fn status(State(state): State<ApiState>) -> Json<Status> {
    let snapshot = state.source.snapshot();
    #[expect(clippy::cast_precision_loss, reason = "Zählwerte weit unter 2^53")]
    let block_rate = if snapshot.log.queries == 0 {
        0.0
    } else {
        snapshot.log.blocked as f64 / snapshot.log.queries as f64
    };

    Json(Status {
        version: env!("CARGO_PKG_VERSION"),
        uptime_seconds: snapshot.uptime.as_secs(),
        logging_mode: format!("{:?}", state.log.mode()).to_lowercase(),
        queries: snapshot.log.queries,
        blocked: snapshot.log.blocked,
        block_rate,
        cache_hit_rate: snapshot.cache.hit_rate(),
        cache_entries: snapshot.cache_entries,
        list_entries: snapshot.policy.entries,
        upstreams: snapshot
            .upstreams
            .iter()
            .map(|upstream| UpstreamInfo {
                name: upstream.name.clone(),
                ok: upstream.successes,
                failed: upstream.failures,
                rtt_ms: upstream.rtt.map(|rtt| rtt.as_secs_f64() * 1000.0),
                down: upstream.down,
            })
            .collect(),
    })
}

async fn top(State(state): State<ApiState>, Query(limit): Query<Limit>) -> Json<Vec<TopDomain>> {
    Json(state.log.top_domains(limit.get()))
}

async fn recent(
    State(state): State<ApiState>,
    Query(limit): Query<Limit>,
) -> Json<Vec<LoggedQuery>> {
    Json(state.log.recent(limit.get()))
}

async fn lists(State(state): State<ApiState>) -> Json<Vec<ListInfo>> {
    Json(state.source.lists())
}

async fn policies(State(state): State<ApiState>) -> Json<Vec<PolicyInfo>> {
    Json(state.source.policies())
}

#[derive(Debug, Serialize)]
struct GrantInfo {
    domain: String,
    remaining_seconds: u64,
}

async fn grants(State(state): State<ApiState>) -> Json<Vec<GrantInfo>> {
    Json(
        state
            .source
            .grants()
            .into_iter()
            .map(|(domain, remaining)| GrantInfo {
                domain,
                remaining_seconds: remaining.as_secs(),
            })
            .collect(),
    )
}

#[derive(Debug, Deserialize)]
struct GrantRequest {
    domain: String,
    #[serde(default = "default_grant_seconds")]
    seconds: u64,
}

const fn default_grant_seconds() -> u64 {
    300
}

/// Längste befristete Freigabe. Wer länger will, gehört auf eine Allowlist —
/// sonst ist "befristet" nur ein anderes Wort für "vergessen".
const MAX_GRANT: Duration = Duration::from_secs(24 * 60 * 60);

async fn grant(
    State(state): State<ApiState>,
    Json(request): Json<GrantRequest>,
) -> Result<Json<GrantInfo>, (StatusCode, Json<ApiError>)> {
    let domain = request.domain.trim().trim_matches('.').to_ascii_lowercase();
    if domain.is_empty() || domain.contains(char::is_whitespace) {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(ApiError {
                error: format!("'{}' ist kein Domainname", request.domain),
            }),
        ));
    }
    let ttl = Duration::from_secs(request.seconds).min(MAX_GRANT);
    if ttl.is_zero() {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(ApiError {
                error: "seconds muss größer als null sein".to_owned(),
            }),
        ));
    }

    state.source.grant(&domain, ttl);
    // Kein Query-Name ins Log: eine Freigabe ist eine Konfigurationsänderung,
    // aber der Name darin ist derselbe, den B.1 Regel 3 schützt.
    tracing::info!(seconds = ttl.as_secs(), "befristete Freigabe erteilt");
    Ok(Json(GrantInfo {
        domain,
        remaining_seconds: ttl.as_secs(),
    }))
}

async fn revoke(State(state): State<ApiState>, Path(domain): Path<String>) -> StatusCode {
    state.source.revoke(&domain);
    StatusCode::NO_CONTENT
}

/// Live-Strom der Anfragen.
async fn events(
    State(state): State<ApiState>,
) -> Sse<impl futures_util::Stream<Item = Result<Event, std::convert::Infallible>>> {
    let mut receiver = state.log.subscribe();
    let stream = async_stream::stream! {
        loop {
            match receiver.recv().await {
                Ok(event) => {
                    if let Ok(json) = serde_json::to_string(&event) {
                        yield Ok(Event::default().data(json));
                    }
                }
                // Der Zuhörer kam nicht hinterher; weitermachen statt abbrechen.
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
            }
        }
    };
    Sse::new(stream).keep_alive(axum::response::sse::KeepAlive::default())
}

async fn metrics(State(state): State<ApiState>) -> Response {
    let text = crate::metrics::render(&state.source.snapshot());
    (
        [(
            axum::http::header::CONTENT_TYPE,
            "text/plain; version=0.0.4; charset=utf-8",
        )],
        text,
    )
        .into_response()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_token_comparison_does_not_short_circuit() {
        assert!(constant_time_eq("geheim", "geheim"));
        assert!(!constant_time_eq("geheim", "geheiM"));
        assert!(!constant_time_eq("geheim", "geheim2"));
        assert!(!constant_time_eq("", "x"));
        assert!(constant_time_eq("", ""));
    }

    #[test]
    fn the_limit_is_capped() {
        assert_eq!(Limit { limit: None }.get(), 50);
        assert_eq!(Limit { limit: Some(10) }.get(), 10);
        assert_eq!(
            Limit {
                limit: Some(100_000)
            }
            .get(),
            500
        );
    }

    #[test]
    fn the_debug_output_hides_the_token() {
        let state = ApiState {
            source: Arc::new(Dummy),
            log: Arc::new(
                QueryLog::new(&crate::config::LoggingConfig::default()).expect("QueryLog"),
            ),
            token: Arc::from("streng-geheim"),
        };
        assert!(
            !format!("{state:?}").contains("streng-geheim"),
            "der Token steht im Debug-Ausdruck"
        );
    }

    struct Dummy;
    impl StatusSource for Dummy {
        fn snapshot(&self) -> Snapshot {
            unimplemented!("nicht benutzt")
        }
        fn lists(&self) -> Vec<ListInfo> {
            Vec::new()
        }
        fn policies(&self) -> Vec<PolicyInfo> {
            Vec::new()
        }
        fn grant(&self, _domain: &str, _ttl: Duration) {}
        fn revoke(&self, _domain: &str) {}
        fn grants(&self) -> Vec<(String, Duration)> {
            Vec::new()
        }
    }
}
