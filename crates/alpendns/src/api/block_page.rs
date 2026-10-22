//! Die "Blocked by AlpenDNS"-Seite.
//!
//! Ein eigener Listener und keine Route der API: die Seite ist offen, die API
//! verlangt einen Token. Zwei Zielgruppen teilen sich keine Routing-Fläche.
//!
//! Die Seite nennt den Namen aus dem `Host`-Kopf, den Grund aus der
//! Policy-Auswertung — sonst nichts. Kein Client, keine Policy, kein Regeltext,
//! keine Zähler, kein Link zur UI.
//!
//! **Kein `tracing` in diesem Modul.** Der Name darf nur im Antwortkörper zum
//! Fragenden hinaus, nirgends ins Log (B.1 Regel 3).

use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;

use axum::Router;
use axum::extract::{ConnectInfo, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use hickory_proto::rr::Name;

use crate::policy::BlockCause;

/// Die Seite, mit `{{TITLE}}`, `{{NAME}}` und `{{BODY}}` als Platzhalter.
const TEMPLATE: &str = include_str!("../../../../web/blocked.html");

/// Woher die Seite ihren Grund bekommt.
///
/// Ohne `Result`: ein Name, der sich nicht parsen lässt, ist Sache des Aufrufers
/// und wird dort zu einem 400.
pub trait BlockPageSource: Send + Sync + 'static {
    fn cause(&self, name: &str, peer: IpAddr) -> BlockCause;
}

#[derive(Clone)]
struct BlockPageState {
    source: Arc<dyn BlockPageSource>,
}

/// Der Router der Blockseite.
///
/// `fallback` statt einer einzelnen Route: wer auf dem Weg zu einem tiefen Pfad
/// hier landet, soll trotzdem die Erklärung sehen. `/app.css` kommt vom selben
/// Listener — sonst stünde die Seite ohne ihr Material da.
pub fn router(source: Arc<dyn BlockPageSource>) -> Router {
    Router::new()
        .route("/app.css", get(stylesheet))
        .fallback(page)
        .with_state(BlockPageState { source })
}

/// Der Name aus dem `Host`-Kopf, normalisiert wie in der Policy.
///
/// Ein angehängter numerischer Port wird entfernt; ein IPv6-Literal in
/// eckigen Klammern ist kein Domainname und fällt durch. `None` heißt: kein
/// brauchbarer Name — der Aufrufer antwortet mit 400 und gibt nichts zurück.
fn host_name(headers: &HeaderMap) -> Option<String> {
    let raw = headers.get(header::HOST)?.to_str().ok()?;
    let host = match raw.rsplit_once(':') {
        Some((name, port))
            if !name.ends_with(']')
                && !port.is_empty()
                && port.bytes().all(|byte| byte.is_ascii_digit()) =>
        {
            name
        }
        _ => raw,
    };
    let name = Name::from_str_relaxed(host).ok()?;
    let normalized = name.to_ascii().trim_end_matches('.').to_ascii_lowercase();
    (!normalized.is_empty()).then_some(normalized)
}

/// HTML-Escaping für jeden dynamischen Wert.
///
/// `Name::to_ascii()` liefert bereits nur sichere ASCII-Zeichen; das hier ist
/// die zweite Schranke und hält die Eigenschaft auch dann, wenn der Name später
/// aus einer anderen Quelle kommt. Der Test pinnt beides.
fn escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for character in text.chars() {
        match character {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            other => out.push(other),
        }
    }
    out
}

/// Ersetzt Platzhalter in **einem** Durchlauf.
///
/// Nacheinander aufgerufene `replace`-Schleifen würden einen eingesetzten Wert
/// erneut absuchen — ein Listenname mit einem Platzhalter darin könnte dann
/// weiteren Inhalt einschleusen. Deshalb wird nur die Vorlage durchlaufen und
/// hinter dem eingesetzten Wert weitergelesen.
fn fill(template: &str, values: &[(&str, &str)]) -> String {
    let mut out = String::with_capacity(template.len());
    let mut rest = template;
    while !rest.is_empty() {
        let next = values
            .iter()
            .filter_map(|(placeholder, value)| {
                rest.find(placeholder)
                    .map(|index| (index, *placeholder, *value))
            })
            .min_by_key(|(index, _, _)| *index);
        let Some((index, placeholder, value)) = next else {
            out.push_str(rest);
            return out;
        };
        if let Some(before) = rest.get(..index) {
            out.push_str(before);
        }
        out.push_str(value);
        rest = rest.get(index + placeholder.len()..).unwrap_or_default();
    }
    out
}

/// Setzt Name und Grund in die Vorlage.
///
/// `NotBlocked` heißt ausdrücklich nur, dass kein Listeneintrag greift. Die
/// Seite darf nicht behaupten, der Name sei erlaubt: die Auswertung lässt die
/// Detektoren aus, ein von einer Heuristik geblockter Name landet hier.
fn render(name: &str, cause: &BlockCause) -> String {
    // Der Titel folgt dem Grund. Für `NotBlocked` behauptet er nicht "blocked":
    // der neutrale Satz im Körper ist die ganze Auskunft dieser Seite, und der
    // Tab darf ihr nicht widersprechen.
    let (title, body) = match cause {
        BlockCause::List(list) => (
            "Blocked — AlpenDNS",
            format!(
                "<p class=\"verdict is-blocked\">This name is on the blocklist \
                 <code>{}</code> on this network.</p>",
                escape(list)
            ),
        ),
        BlockCause::Rule => (
            "Blocked — AlpenDNS",
            "<p class=\"verdict is-blocked\">This name is blocked by a rule \
             on this network.</p>"
                .to_owned(),
        ),
        BlockCause::NotBlocked => (
            "AlpenDNS",
            "<p class=\"verdict\">No blocklist entry applies to this name.</p>".to_owned(),
        ),
    };
    fill(
        TEMPLATE,
        &[
            ("{{NAME}}", &escape(name)),
            ("{{BODY}}", &body),
            ("{{TITLE}}", title),
        ],
    )
}

async fn page(
    State(state): State<BlockPageState>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
) -> Response {
    let Some(name) = host_name(&headers) else {
        // Der feindliche Wert wird nicht zurückgegeben: die Meldung ist fest.
        return (
            StatusCode::BAD_REQUEST,
            [(header::CONTENT_TYPE, "text/plain; charset=utf-8")],
            "AlpenDNS: the Host header does not name a domain",
        )
            .into_response();
    };
    let cause = state.source.cause(&name, peer.ip());
    (
        [
            (header::CONTENT_TYPE, "text/html; charset=utf-8"),
            // Nicht `no-cache`: der Browser soll die Seite nach dem Ende der
            // Sperre nicht aus dem Cache zeigen.
            (header::CACHE_CONTROL, "no-store"),
            (header::X_CONTENT_TYPE_OPTIONS, "nosniff"),
        ],
        render(&name, &cause),
    )
        .into_response()
}

async fn stylesheet() -> Response {
    (
        [
            (header::CONTENT_TYPE, "text/css; charset=utf-8"),
            (header::CACHE_CONTROL, "no-cache"),
        ],
        super::ui::STYLE,
    )
        .into_response()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::Ipv4Addr;
    use std::time::Duration;

    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    use crate::clock::{FixedWallClock, TestClock};
    use crate::config::BlockingConfig;
    use crate::detect::{Action, Detector, Detectors, Finding, NameDetector, Observation};
    use crate::filter::LoadedLists;
    use crate::filter::matcher::Builder as MatcherBuilder;
    use crate::filter::parser::{Format, parse};
    use crate::policy::clients::ClientRule;
    use crate::policy::rules::RegexRules;
    use crate::policy::{Blueprint, Engine, PolicyBlueprint};
    use crate::trace::Ctx;

    type TestEngine = Engine<Arc<TestClock>, Arc<FixedWallClock>>;

    fn lists(entries: &[(&str, &str)]) -> LoadedLists {
        LoadedLists::from_matchers(
            entries
                .iter()
                .map(|(name, text)| {
                    let parsed = parse(text, Format::Wildcard);
                    let mut builder = MatcherBuilder::new();
                    builder.add(name, &parsed);
                    (Arc::from(*name), Arc::new(builder.build()))
                })
                .collect(),
        )
    }

    fn policy(name: &str, blocklists: &[&str], regex: &[&str]) -> PolicyBlueprint {
        let patterns: Vec<String> = regex.iter().map(|pattern| (*pattern).to_owned()).collect();
        PolicyBlueprint {
            name: Arc::from(name),
            blocklists: blocklists.iter().map(|list| Arc::from(*list)).collect(),
            allowlists: Vec::new(),
            regex: Arc::new(RegexRules::compile(name, &patterns).expect("valid patterns")),
            schedules: Vec::new(),
        }
    }

    fn client(name: &str, address: &str, policy: &str) -> ClientRule {
        ClientRule {
            name: Arc::from(name),
            nets: vec![crate::config::parse_net(address).expect("valid network")],
            policy: Arc::from(policy),
        }
    }

    fn engine(
        clients: Vec<ClientRule>,
        policies: Vec<PolicyBlueprint>,
        loaded: &LoadedLists,
        detectors: Detectors,
    ) -> Arc<TestEngine> {
        let clock = Arc::new(TestClock::new());
        let wall = Arc::new(FixedWallClock::new(
            "2026-08-31T12:00:00[Europe/Vienna]"
                .parse()
                .expect("valid instant"),
        ));
        let blueprint = Blueprint::new(clients, Arc::from("default"), policies);
        let entries = loaded.total_entries();
        Arc::new(
            Engine::new(
                blueprint.build(loaded).expect("rule set"),
                entries,
                &BlockingConfig::default(),
                clock,
                wall,
            )
            .with_detectors(detectors),
        )
    }

    /// Der Quell-Adapter wie im Binary: Fehler beim Parsen sind kein Block.
    struct EngineSource(Arc<TestEngine>);

    impl BlockPageSource for EngineSource {
        fn cause(&self, name: &str, peer: IpAddr) -> BlockCause {
            crate::policy::block_cause(&self.0, name, peer).unwrap_or(BlockCause::NotBlocked)
        }
    }

    /// Startet den Router auf einem freien Port.
    ///
    /// `into_make_service_with_connect_info` ist Pflicht: mit dem blanken Router
    /// liefe der `ConnectInfo`-Extraktor in einen 500er, und zwar erst zur
    /// Laufzeit. Test 1 beweist damit zugleich, dass der Dienst die Peer-Adresse
    /// mitbringt.
    async fn serve(source: Arc<dyn BlockPageSource>) -> SocketAddr {
        // `reqwest` ist mit `rustls-no-provider` gebaut und nimmt den
        // Prozess-Default; im Betrieb installiert ihn der Listen-Loader.
        crate::filter::source::install_crypto_provider();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind");
        let addr = listener.local_addr().expect("local_addr");
        tokio::spawn(async move {
            let service = router(source).into_make_service_with_connect_info::<SocketAddr>();
            let _ = axum::serve(listener, service).await;
        });
        addr
    }

    async fn get(addr: SocketAddr, host: Option<&str>) -> reqwest::Response {
        let mut request = reqwest::Client::new().get(format!("http://127.0.0.1:{}/", addr.port()));
        if let Some(host) = host {
            request = request.header("Host", host);
        }
        request.send().await.expect("request")
    }

    async fn body(response: reqwest::Response) -> String {
        response.text().await.expect("body")
    }

    /// Rohe HTTP/1.1-Anfrage für den Fall, den `reqwest` nicht bauen kann:
    /// ein fehlender oder unbrauchbarer `Host`-Kopf.
    async fn raw_request(addr: SocketAddr, request: &str) -> String {
        let mut stream = tokio::net::TcpStream::connect(addr).await.expect("connect");
        stream
            .write_all(request.as_bytes())
            .await
            .expect("write request");
        stream.flush().await.expect("flush");
        let mut buffer = Vec::new();
        stream.read_to_end(&mut buffer).await.expect("read");
        String::from_utf8_lossy(&buffer).into_owned()
    }

    #[derive(Debug)]
    struct Dummy;

    impl NameDetector for Dummy {
        fn detector(&self) -> Detector {
            Detector::Dga
        }

        fn inspect(&self, observation: &Observation<'_>) -> Option<Finding> {
            observation
                .name
                .contains("verdaechtig")
                .then(|| Finding::new(Detector::Dga, 930, "test detector: suspicious"))
        }
    }

    // -----------------------------------------------------------------------
    // Die Testrouter auf einem echten Port
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn block_page_answers_with_the_name_and_the_matched_list() {
        let loaded = lists(&[("ads-list", "doubleclick.net\n")]);
        let engine = engine(
            Vec::new(),
            vec![policy("default", &["ads-list"], &[])],
            &loaded,
            Detectors::new(),
        );
        let addr = serve(Arc::new(EngineSource(engine))).await;

        let response = get(addr, Some("doubleclick.net")).await;
        assert_eq!(response.status(), 200);
        assert!(
            response
                .headers()
                .get(header::CONTENT_TYPE)
                .and_then(|value| value.to_str().ok())
                .is_some_and(|value| value.starts_with("text/html")),
            "not HTML: {:?}",
            response.headers()
        );
        assert_eq!(
            response
                .headers()
                .get(header::CACHE_CONTROL)
                .and_then(|value| value.to_str().ok()),
            Some("no-store")
        );
        assert_eq!(
            response
                .headers()
                .get(header::X_CONTENT_TYPE_OPTIONS)
                .and_then(|value| value.to_str().ok()),
            Some("nosniff")
        );

        let body = body(response).await;
        assert!(body.contains("doubleclick.net"), "{body}");
        assert!(body.contains("ads-list"), "{body}");
        assert!(body.contains("class=\"verdict is-blocked\""), "{body}");
    }

    #[tokio::test]
    async fn block_page_uses_the_http_peer_for_the_verdict() {
        let loaded = lists(&[("strict-list", "blocked.example\n")]);
        let engine = engine(
            vec![client("viewer", "127.0.0.2/32", "strict")],
            vec![
                policy("default", &[], &[]),
                policy("strict", &["strict-list"], &[]),
            ],
            &loaded,
            Detectors::new(),
        );
        let addr = serve(Arc::new(EngineSource(engine))).await;

        // 127.0.0.2 ist die zweite Loopback-Adresse; Linux gibt das ganze
        // 127.0.0.0/8 ohne Zutun an die Loopback-Schnittstelle.
        let strict = reqwest::Client::builder()
            .local_address(IpAddr::V4(Ipv4Addr::new(127, 0, 0, 2)))
            .build()
            .expect("client");
        let response = strict
            .get(format!("http://127.0.0.1:{}/", addr.port()))
            .header("Host", "blocked.example")
            .send()
            .await
            .expect("request");
        let rendered = body(response).await;
        assert!(rendered.contains("strict-list"), "{rendered}");
        assert!(
            rendered.contains("class=\"verdict is-blocked\""),
            "{rendered}"
        );

        // Dieselbe Frage von der Default-Adresse: die Policy dort ist leer.
        let neutral = body(get(addr, Some("blocked.example")).await).await;
        assert!(
            neutral.contains("No blocklist entry applies to this name."),
            "{neutral}"
        );
        assert!(
            !neutral.contains("class=\"verdict is-blocked\""),
            "{neutral}"
        );
    }

    #[tokio::test]
    async fn block_page_is_neutral_when_not_blocked() {
        let loaded = lists(&[("ads-list", "doubleclick.net\n")]);
        let engine = engine(
            Vec::new(),
            vec![policy("default", &["ads-list"], &[])],
            &loaded,
            Detectors::new(),
        );
        let addr = serve(Arc::new(EngineSource(engine))).await;

        let body = body(get(addr, Some("unlisted.example")).await).await;
        assert!(
            body.contains("No blocklist entry applies to this name."),
            "{body}"
        );
        assert!(!body.contains("class=\"verdict is-blocked\""), "{body}");
    }

    #[tokio::test]
    async fn block_page_names_no_list_for_a_rule_block() {
        let engine = engine(
            Vec::new(),
            vec![policy("default", &[], &[r"evil-[0-9]+\.example"])],
            &LoadedLists::from_matchers(Vec::new()),
            Detectors::new(),
        );
        let addr = serve(Arc::new(EngineSource(engine))).await;

        let body = body(get(addr, Some("evil-42.example")).await).await;
        assert!(
            body.contains("This name is blocked by a rule on this network."),
            "{body}"
        );
        assert!(body.contains("class=\"verdict is-blocked\""), "{body}");
        assert!(!body.contains("evil-[0-9]+"), "{body}");
    }

    #[tokio::test]
    async fn block_page_does_not_disclose_peer_policy_or_rule() {
        let loaded = lists(&[("strict-list", "blocked.example\n")]);
        let engine = engine(
            vec![client("viewer", "127.0.0.2/32", "strict-policy")],
            vec![
                policy("default", &[], &[]),
                policy(
                    "strict-policy",
                    &["strict-list"],
                    &[r"evil-[0-9]+\.example"],
                ),
            ],
            &loaded,
            Detectors::new(),
        );

        // 13 Blocks über den gewöhnlichen DNS-Pfad: die Seite darf die Zahl
        // weder zeigen noch verändern.
        let peer: IpAddr = "127.0.0.2".parse().expect("address");
        for _ in 0..13 {
            let mut ctx = Ctx::new(SocketAddr::new(peer, 4242));
            let name = Name::from_ascii("blocked.example").expect("name");
            assert_eq!(
                engine.evaluate(&name, peer, &mut ctx),
                crate::policy::Decision::Block
            );
        }
        assert_eq!(engine.stats().blocked, 13);

        let addr = serve(Arc::new(EngineSource(Arc::clone(&engine)))).await;
        let strict = reqwest::Client::builder()
            .local_address(IpAddr::V4(Ipv4Addr::new(127, 0, 0, 2)))
            .build()
            .expect("client");
        let response = strict
            .get(format!("http://127.0.0.1:{}/", addr.port()))
            .header("Host", "evil-42.example")
            .send()
            .await
            .expect("request");
        let body = body(response).await;

        for secret in [
            "127.0.0.2",
            "viewer",
            "strict-policy",
            "strict-list",
            "evil-[0-9]+",
            "13",
        ] {
            assert!(!body.contains(secret), "the page shows '{secret}': {body}");
        }
        assert_eq!(engine.stats().blocked, 13, "the page changed the counters");
    }

    #[tokio::test]
    async fn block_page_rejects_a_missing_or_invalid_host() {
        let engine = engine(
            Vec::new(),
            vec![policy("default", &[], &[])],
            &LoadedLists::from_matchers(Vec::new()),
            Detectors::new(),
        );
        let addr = serve(Arc::new(EngineSource(engine))).await;

        let missing = raw_request(addr, "GET / HTTP/1.1\r\nConnection: close\r\n\r\n").await;
        assert!(missing.starts_with("HTTP/1.1 400"), "{missing}");

        let hostile = raw_request(
            addr,
            "GET / HTTP/1.1\r\nHost: \"><script>alert(1)</script>\r\nConnection: close\r\n\r\n",
        )
        .await;
        assert!(hostile.starts_with("HTTP/1.1 400"), "{hostile}");
        assert!(!hostile.contains("<script>alert(1)"), "{hostile}");

        let literal = raw_request(
            addr,
            "GET / HTTP/1.1\r\nHost: [::1]:80\r\nConnection: close\r\n\r\n",
        )
        .await;
        assert!(literal.starts_with("HTTP/1.1 400"), "{literal}");
    }

    #[tokio::test]
    async fn block_page_serves_the_stylesheet() {
        let engine = engine(
            Vec::new(),
            vec![policy("default", &[], &[])],
            &LoadedLists::from_matchers(Vec::new()),
            Detectors::new(),
        );
        let addr = serve(Arc::new(EngineSource(engine))).await;

        let response = reqwest::Client::new()
            .get(format!("http://127.0.0.1:{}/app.css", addr.port()))
            .header("Host", "example.com")
            .send()
            .await
            .expect("request");
        assert_eq!(response.status(), 200);
        assert_eq!(
            response
                .headers()
                .get(header::CONTENT_TYPE)
                .and_then(|value| value.to_str().ok()),
            Some("text/css; charset=utf-8")
        );
        assert!(body(response).await.contains("--danger:"));
    }

    // -----------------------------------------------------------------------
    // Die Bausteine
    // -----------------------------------------------------------------------

    #[test]
    fn block_page_strips_a_numeric_port() {
        let mut headers = HeaderMap::new();
        headers.insert(header::HOST, "Ads.Example.:8080".parse().expect("value"));
        assert_eq!(host_name(&headers).as_deref(), Some("ads.example"));

        headers.insert(header::HOST, "[::1]:80".parse().expect("value"));
        assert_eq!(host_name(&headers), None);

        headers.insert(header::HOST, "example.com".parse().expect("value"));
        assert_eq!(host_name(&headers).as_deref(), Some("example.com"));

        let empty = HeaderMap::new();
        assert_eq!(host_name(&empty), None);
    }

    #[test]
    fn block_page_escapes_a_hostile_value() {
        let hostile = "\"><script>alert(1)</script>";
        let rendered = render(hostile, &BlockCause::List(Arc::from("list\"<x>")));
        assert!(
            !rendered.contains("\"><script>alert(1)"),
            "the hostile name became markup: {rendered}"
        );
        assert!(
            rendered.contains("&quot;&gt;&lt;script&gt;alert(1)&lt;/script&gt;"),
            "{rendered}"
        );
        assert!(
            rendered.contains("list&quot;&lt;x&gt;"),
            "the list name was not escaped: {rendered}"
        );
    }

    #[test]
    fn block_page_fill_does_not_rescan_inserted_values() {
        // Ein eingesetzter Wert, der selbst einen Platzhalter enthält, darf
        // nicht erneut ersetzt werden.
        let filled = fill("[{{A}}|{{B}}]", &[("{{A}}", "x{{B}}"), ("{{B}}", "y")]);
        assert_eq!(filled, "[x{{B}}|y]");
    }

    #[test]
    fn block_page_title_follows_the_cause() {
        // Der neutrale Satz im Körper sagt ausdrücklich nur, dass keine Liste
        // greift. Ein Titel, der "Blocked" behauptet, würde das im Tab
        // widerrufen — und der Test `block_page_is_neutral_when_not_blocked`
        // holt genau diese Seite.
        let neutral = render("unlisted.example", &BlockCause::NotBlocked);
        assert!(neutral.contains("<title>AlpenDNS</title>"), "{neutral}");
        assert!(!neutral.contains("Blocked"), "{neutral}");

        for cause in [
            BlockCause::List(Arc::from("ads-list")),
            BlockCause::Rule,
        ] {
            let blocked = render("ads.example", &cause);
            assert!(
                blocked.contains("<title>Blocked — AlpenDNS</title>"),
                "{blocked}"
            );
        }
    }

    // -----------------------------------------------------------------------
    // Die statische Seite
    // -----------------------------------------------------------------------

    #[test]
    fn block_page_references_nothing_external() {
        for needle in [
            "http://",
            "https://",
            "//cdn",
            "fonts.googleapis",
            "unpkg",
            "jsdelivr",
            "@import url(",
        ] {
            assert!(
                !TEMPLATE.contains(needle),
                "the block page references '{needle}'"
            );
        }
        assert!(TEMPLATE.contains("href=\"/app.css\""), "no stylesheet");
    }

    #[test]
    fn block_page_theme_follows_the_system() {
        assert!(TEMPLATE.contains("prefers-color-scheme: dark"));
        assert!(TEMPLATE.contains("dataset.theme"));
        assert!(!TEMPLATE.contains("localStorage"));
        assert!(!TEMPLATE.contains("theme-toggle"));
    }

    #[test]
    fn block_page_uses_one_panel() {
        assert_eq!(TEMPLATE.matches("class=\"panel").count(), 1);
        assert!(!TEMPLATE.contains("class=\"card"));
        assert!(TEMPLATE.contains("class=\"sky\""));
    }

    #[test]
    fn block_page_links_nothing_to_the_api_or_ui() {
        assert!(!TEMPLATE.contains("/api"));
        assert!(!TEMPLATE.contains("/app.js"));
    }

    #[test]
    fn block_page_colour_is_limited_to_the_blocked_verdict() {
        for colour in ["var(--success)", "var(--warn)", "var(--brand)"] {
            assert!(
                !TEMPLATE.contains(colour),
                "the block page uses {colour}, which carries no meaning here"
            );
        }
        let style = TEMPLATE
            .split_once("<style>")
            .and_then(|(_, rest)| rest.split_once("</style>"))
            .map(|(style, _)| style)
            .expect("inline style block");
        for block in style.split('}') {
            if block.contains("--danger") {
                assert!(
                    block.contains(".is-blocked"),
                    "danger colour outside the blocked verdict: {block}"
                );
            }
        }
    }

    // -----------------------------------------------------------------------
    // Die Auswertung ohne Nebenwirkung
    // -----------------------------------------------------------------------

    #[test]
    fn block_cause_returns_list_notblocked_and_rule() {
        let loaded = lists(&[("ads-list", "ads.example\n")]);
        let engine = engine(
            Vec::new(),
            vec![policy("default", &["ads-list"], &[r"evil-[0-9]+\.example"])],
            &loaded,
            Detectors::new(),
        );
        let peer: IpAddr = "127.0.0.1".parse().expect("address");

        assert_eq!(
            crate::policy::block_cause(&engine, "ads.example", peer),
            Ok(BlockCause::List(Arc::from("ads-list")))
        );
        assert_eq!(
            crate::policy::block_cause(&engine, "example.org", peer),
            Ok(BlockCause::NotBlocked)
        );
        assert_eq!(
            crate::policy::block_cause(&engine, "evil-42.example", peer),
            Ok(BlockCause::Rule)
        );

        engine
            .denied()
            .grant("denied.example", Duration::from_secs(60));
        assert_eq!(
            crate::policy::block_cause(&engine, "denied.example", peer),
            Ok(BlockCause::Rule)
        );

        assert!(
            crate::policy::block_cause(&engine, "not a name", peer).is_err(),
            "an invalid name is an error, not a verdict"
        );
    }

    #[test]
    fn block_cause_leaves_the_policy_counters_unchanged() {
        let loaded = lists(&[("ads-list", "ads.example\n")]);
        let engine = engine(
            Vec::new(),
            vec![policy("default", &["ads-list"], &[r"evil-[0-9]+\.example"])],
            &loaded,
            Detectors::new(),
        );
        let peer: IpAddr = "127.0.0.1".parse().expect("address");
        let before = engine.stats();

        for name in ["ads.example", "example.org", "evil-42.example"] {
            let _ = crate::policy::block_cause(&engine, name, peer);
        }

        assert_eq!(engine.stats(), before, "the page changed DNS statistics");
    }

    #[test]
    fn block_cause_does_not_run_detectors() {
        let detectors = Detectors::new().with_name(Dummy, Action::Block, 0);
        let loaded = LoadedLists::from_matchers(Vec::new());
        let engine = engine(
            Vec::new(),
            vec![policy("default", &[], &[])],
            &loaded,
            detectors,
        );
        let peer: IpAddr = "127.0.0.1".parse().expect("address");

        // Der DNS-Pfad blockt den Namen …
        let mut ctx = Ctx::new(SocketAddr::new(peer, 4242));
        let name = Name::from_ascii("verdaechtig.example").expect("name");
        assert_eq!(
            engine.evaluate(&name, peer, &mut ctx),
            crate::policy::Decision::Block
        );

        // … die Seite fragt die Detektoren nicht und sagt darum nur, dass keine
        // Liste greift. Genau deshalb behauptet ihr neutraler Satz nicht, der
        // Name sei erlaubt.
        assert_eq!(
            crate::policy::block_cause(&engine, "verdaechtig.example", peer),
            Ok(BlockCause::NotBlocked)
        );
    }
}
