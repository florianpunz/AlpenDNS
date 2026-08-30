//! Integrationstests für Blocklisten: Laden, Aktualisieren, Filtern.
//!
//! Der HTTP-Server ist ein Fake im selben Prozess; es wird nie eine echte
//! Blocklisten-URL abgerufen (docs/TESTING.md §4).

// Testcode darf panicken, siehe B.1 und clippy.toml.
#![allow(clippy::expect_used, clippy::indexing_slicing, clippy::panic)]

use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::Duration;

use alpendns::clock::{SystemWallClock, TestClock};
use alpendns::config::BlockingConfig;
use alpendns::filter::Lists;
use alpendns::filter::block::BlockMode;
use alpendns::filter::parser::Format;
use alpendns::filter::source::{ListSpec, Loader, Origin, Source};
use alpendns::policy::rules::RegexRules;
use alpendns::policy::{Blueprint, Decision, Engine, PolicyBackend, PolicyBlueprint};
use alpendns::resolve::{ResolveBackend, ResolveError};
use alpendns::trace::{Ctx, Step};
use hickory_proto::op::{Message, MessageType, OpCode, Query, ResponseCode};
use hickory_proto::rr::{Name, RecordType};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

/// Ein Verzeichnis, das sich beim Verlassen des Gültigkeitsbereichs aufräumt.
struct TempDir(PathBuf);

impl TempDir {
    fn new(label: &str) -> Self {
        let path = std::env::temp_dir().join(format!(
            "alpendns-test-{label}-{}-{}",
            std::process::id(),
            rand_suffix()
        ));
        std::fs::create_dir_all(&path).expect("Testverzeichnis");
        Self(path)
    }

    fn path(&self) -> PathBuf {
        self.0.clone()
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn rand_suffix() -> u64 {
    use std::hash::{BuildHasher as _, RandomState};
    RandomState::new().hash_one("suffix")
}

/// Zustand des HTTP-Fakes, von den Tests umschaltbar.
struct HttpFake {
    addr: SocketAddr,
    requests: Arc<AtomicUsize>,
    /// Wie oft mit 304 geantwortet wurde.
    not_modified: Arc<AtomicUsize>,
    down: Arc<AtomicBool>,
}

/// Ein HTTP/1.1-Server, der genau eine Liste ausliefert und ETag beherrscht.
async fn http_fake(body: &'static str, etag: &'static str) -> HttpFake {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let addr = listener.local_addr().expect("local_addr");
    let requests = Arc::new(AtomicUsize::new(0));
    let not_modified = Arc::new(AtomicUsize::new(0));
    let down = Arc::new(AtomicBool::new(false));

    let (counter, cached, offline) = (
        Arc::clone(&requests),
        Arc::clone(&not_modified),
        Arc::clone(&down),
    );
    tokio::spawn(async move {
        while let Ok((mut stream, _)) = listener.accept().await {
            counter.fetch_add(1, Ordering::SeqCst);
            if offline.load(Ordering::SeqCst) {
                // Verbindung ohne Antwort schließen: der Server ist "weg".
                drop(stream);
                continue;
            }
            let cached = Arc::clone(&cached);
            tokio::spawn(async move {
                let Some(headers) = read_headers(&mut stream).await else {
                    return;
                };
                let matches_etag = headers.lines().any(|line| {
                    line.to_ascii_lowercase().starts_with("if-none-match:") && line.contains(etag)
                });
                let response = if matches_etag {
                    cached.fetch_add(1, Ordering::SeqCst);
                    format!(
                        "HTTP/1.1 304 Not Modified\r\netag: {etag}\r\ncontent-length: 0\r\n\r\n"
                    )
                } else {
                    format!(
                        "HTTP/1.1 200 OK\r\netag: {etag}\r\ncontent-type: text/plain\r\ncontent-length: {}\r\n\r\n{body}",
                        body.len()
                    )
                };
                let _ = stream.write_all(response.as_bytes()).await;
                let _ = stream.flush().await;
            });
        }
    });

    HttpFake {
        addr,
        requests,
        not_modified,
        down,
    }
}

async fn read_headers(stream: &mut TcpStream) -> Option<String> {
    let mut buffer = Vec::new();
    let mut byte = [0_u8; 1];
    // Bis zum Ende des Kopfes lesen; der Fake kennt keine Bodies.
    while buffer.len() < 8192 {
        match stream.read(&mut byte).await {
            Ok(0) | Err(_) => return None,
            Ok(_) => buffer.push(byte[0]),
        }
        if buffer.ends_with(b"\r\n\r\n") {
            break;
        }
    }
    String::from_utf8(buffer).ok()
}

const LIST_BODY: &str = "# Testliste\n0.0.0.0 ads.example.com\n0.0.0.0 tracker.example.net\n";

fn spec(name: &str, source: Source, format: Format) -> ListSpec {
    ListSpec {
        name: name.to_owned(),
        source,
        format,
    }
}

/// Ein Kontext für Tests, die sich nicht für den Trace interessieren.
fn ctx() -> Ctx {
    Ctx::new(std::net::SocketAddr::from(([127, 0, 0, 1], 5555)))
}

fn ask(name: &str) -> Message {
    let mut message = Message::new(0x1234, MessageType::Query, OpCode::Query);
    message.metadata.recursion_desired = true;
    message.add_query(Query::query(
        Name::from_ascii(name).expect("gültiger Name"),
        RecordType::A,
    ));
    message
}

/// Ein Backend, das jede Anfrage beantwortet und mitzählt.
#[derive(Debug, Default)]
struct Upstream {
    calls: AtomicUsize,
}

impl ResolveBackend for Upstream {
    fn resolve(
        &self,
        request: &Message,
        _ctx: &mut Ctx,
    ) -> impl std::future::Future<Output = Result<Message, ResolveError>> + Send {
        let id = request.metadata.id;
        let queries = request.queries.clone();
        async move {
            self.calls.fetch_add(1, Ordering::SeqCst);
            let mut response = Message::response(id, OpCode::Query);
            response.add_queries(queries);
            Ok(response)
        }
    }
}

fn blocking_config(mode: BlockMode) -> BlockingConfig {
    BlockingConfig {
        mode,
        ..BlockingConfig::default()
    }
}

// ---------------------------------------------------------------------------
// Laden über HTTP, ETag, Platten-Cache
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_list_is_downloaded_and_parsed() {
    let fake = http_fake(LIST_BODY, "\"v1\"").await;
    let cache = TempDir::new("download");
    let lists = Lists::new(
        Loader::new(cache.path()).expect("Loader"),
        vec![spec(
            "test",
            Source::Url(format!("http://{}/liste", fake.addr)),
            Format::Hosts,
        )],
    );

    let set = lists.load(true).await.expect("Liste lädt");
    assert_eq!(set.total_entries(), 2);
    assert_eq!(fake.requests.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn the_second_load_gets_a_304_and_uses_the_cached_copy() {
    // Roadmap Schritt 8: kein erneutes Herunterladen, wenn sich nichts geändert hat.
    let fake = http_fake(LIST_BODY, "\"v1\"").await;
    let cache = TempDir::new("etag");
    let lists = Lists::new(
        Loader::new(cache.path()).expect("Loader"),
        vec![spec(
            "test",
            Source::Url(format!("http://{}/liste", fake.addr)),
            Format::Hosts,
        )],
    );

    lists.load(true).await.expect("erster Abruf");
    let second = lists.load(true).await.expect("zweiter Abruf");

    assert_eq!(
        second.total_entries(),
        2,
        "die Liste ging beim 304 verloren"
    );
    assert_eq!(
        fake.not_modified.load(Ordering::SeqCst),
        1,
        "kein If-None-Match geschickt"
    );
}

#[tokio::test]
async fn an_unreachable_server_falls_back_to_the_cached_copy() {
    // B.1 Regel 6: ein späterer Ausfall darf nicht ungefiltert machen.
    let fake = http_fake(LIST_BODY, "\"v1\"").await;
    let cache = TempDir::new("fallback");
    let loader = Loader::new(cache.path()).expect("Loader");
    let list = spec(
        "test",
        Source::Url(format!("http://{}/liste", fake.addr)),
        Format::Hosts,
    );

    let first = loader.load(&list).await.expect("erster Abruf");
    assert_eq!(first.origin, Origin::Network);

    fake.down.store(true, Ordering::SeqCst);
    let second = loader.load(&list).await.expect("Rückfall auf den Cache");
    assert_eq!(second.origin, Origin::StaleCache);
    assert!(second.text.contains("ads.example.com"));
}

#[tokio::test]
async fn a_first_start_without_a_reachable_list_fails() {
    // Roadmap Schritt 10, erster Fall: lieber kein DNS als ungefiltertes DNS.
    let fake = http_fake(LIST_BODY, "\"v1\"").await;
    fake.down.store(true, Ordering::SeqCst);
    let cache = TempDir::new("firststart");
    let lists = Lists::new(
        Loader::new(cache.path()).expect("Loader"),
        vec![spec(
            "test",
            Source::Url(format!("http://{}/liste", fake.addr)),
            Format::Hosts,
        )],
    );

    let error = lists
        .load(true)
        .await
        .expect_err("ohne Liste und ohne Cache muss der Start scheitern");
    assert!(error.to_string().contains("test"), "{error}");
}

#[tokio::test]
async fn a_later_failure_keeps_the_server_running() {
    // Roadmap Schritt 10, zweiter Fall.
    let fake = http_fake(LIST_BODY, "\"v1\"").await;
    fake.down.store(true, Ordering::SeqCst);
    let cache = TempDir::new("laterfail");
    let lists = Lists::new(
        Loader::new(cache.path()).expect("Loader"),
        vec![spec(
            "test",
            Source::Url(format!("http://{}/liste", fake.addr)),
            Format::Hosts,
        )],
    );

    let set = lists
        .load(false)
        .await
        .expect("ein Ausfall im Betrieb ist kein Grund aufzuhören");
    assert_eq!(set.total_entries(), 0, "es gab nichts zu laden");
}

#[tokio::test]
async fn a_list_can_come_from_a_file() {
    let dir = TempDir::new("file");
    let path = dir.path().join("allow.txt");
    std::fs::write(&path, "ads.example.com\n").expect("schreiben");

    let lists = Lists::new(
        Loader::new(dir.path()).expect("Loader"),
        vec![spec("lokal", Source::File(path), Format::Domains)],
    );
    let set = lists.load(true).await.expect("Datei lädt");
    assert_eq!(set.total_entries(), 1);
}

// ---------------------------------------------------------------------------
// Filtern
// ---------------------------------------------------------------------------

/// Der Engine-Typ, den diese Tests benutzen: stellbare Uhr für Fristen,
/// echte Kalenderuhr (Zeitpläne prüft tests/policies.rs).
type TestEngine = Engine<Arc<TestClock>, SystemWallClock>;

async fn filter_with(block: &str, allow: &str, mode: BlockMode) -> Arc<TestEngine> {
    let dir = TempDir::new("filter");
    let block_path = dir.path().join("block.txt");
    let allow_path = dir.path().join("allow.txt");
    std::fs::write(&block_path, block).expect("schreiben");
    std::fs::write(&allow_path, allow).expect("schreiben");

    let lists = Lists::new(
        Loader::new(dir.path()).expect("Loader"),
        vec![
            spec("block", Source::File(block_path), Format::Wildcard),
            spec("allow", Source::File(allow_path), Format::Wildcard),
        ],
    );
    let loaded = lists.load(true).await.expect("Listen laden");
    let blueprint = Blueprint::new(
        Vec::new(),
        Arc::from("default"),
        vec![PolicyBlueprint {
            name: Arc::from("default"),
            blocklists: vec![Arc::from("block")],
            allowlists: vec![Arc::from("allow")],
            regex: Arc::new(RegexRules::default()),
            schedules: Vec::new(),
        }],
    );
    let entries = loaded.total_entries();
    Arc::new(Engine::new(
        blueprint.build(&loaded).expect("Regelstand"),
        entries,
        &blocking_config(mode),
        Arc::new(TestClock::new()),
        SystemWallClock,
    ))
}

#[tokio::test]
async fn a_blocked_name_never_reaches_the_upstream() {
    let filter = filter_with("ads.example.com\n", "", BlockMode::Nxdomain).await;
    let upstream = Upstream::default();
    let backend = PolicyBackend::new(filter, upstream);

    let response = backend
        .resolve(&ask("ads.example.com."), &mut ctx())
        .await
        .expect("Antwort");
    assert_eq!(response.metadata.response_code, ResponseCode::NXDomain);
}

#[tokio::test]
async fn an_unlisted_name_goes_through() {
    let filter = filter_with("ads.example.com\n", "", BlockMode::Nxdomain).await;
    let backend = PolicyBackend::new(filter, Upstream::default());

    let response = backend
        .resolve(&ask("example.org."), &mut ctx())
        .await
        .expect("Antwort");
    assert_eq!(response.metadata.response_code, ResponseCode::NoError);
}

#[tokio::test]
async fn the_allowlist_beats_the_blocklist() {
    // Roadmap Schritt 6: eine Domain auf beiden Listen wird durchgelassen.
    let filter = filter_with(
        "ads.example.com\n",
        "ads.example.com\n",
        BlockMode::Nxdomain,
    )
    .await;
    let backend = PolicyBackend::new(Arc::clone(&filter), Upstream::default());

    let response = backend
        .resolve(&ask("ads.example.com."), &mut ctx())
        .await
        .expect("Antwort");
    assert_eq!(
        response.metadata.response_code,
        ResponseCode::NoError,
        "die Allowlist wurde übergangen"
    );
    assert_eq!(filter.stats().allowed, 1);
    assert_eq!(filter.stats().blocked, 0);
}

#[tokio::test]
async fn an_allowlisted_subdomain_survives_a_blocked_parent() {
    let filter = filter_with("example.com\n", "gut.example.com\n", BlockMode::Nxdomain).await;
    let backend = PolicyBackend::new(filter, Upstream::default());

    assert_eq!(
        backend
            .resolve(&ask("gut.example.com."), &mut ctx())
            .await
            .expect("Antwort")
            .metadata
            .response_code,
        ResponseCode::NoError
    );
    assert_eq!(
        backend
            .resolve(&ask("boese.example.com."), &mut ctx())
            .await
            .expect("Antwort")
            .metadata
            .response_code,
        ResponseCode::NXDomain
    );
}

#[tokio::test]
async fn the_verdict_names_the_rule_that_matched() {
    let engine = filter_with("# Kopf\nads.example.com\n", "", BlockMode::Nxdomain).await;
    let name = Name::from_ascii("sub.ads.example.com.").expect("gültig");
    let mut ctx = ctx();
    let peer = ctx.peer.ip();

    assert_eq!(engine.evaluate(&name, peer, &mut ctx), Decision::Block);
    let hit = ctx
        .steps()
        .iter()
        .find_map(|step| match step {
            Step::BlocklistHit {
                list,
                line,
                matched,
            } => Some((list, *line, matched)),
            _ => None,
        })
        .expect("ein Blocklisten-Schritt");
    assert_eq!(&**hit.0, "block");
    assert_eq!(hit.1, 2, "Zeilennummer");
    assert_eq!(hit.2, "ads.example.com", "der zutreffende Eintrag");
}

#[tokio::test]
async fn swapping_the_rules_under_load_neither_fails_nor_stalls() {
    // Roadmap Schritt 9: kein Ausfall und keine Latenzspitze beim Tausch.
    let filter = filter_with("ads.example.com\n", "", BlockMode::Nxdomain).await;
    let backend = Arc::new(PolicyBackend::new(Arc::clone(&filter), Upstream::default()));

    let stop = Arc::new(AtomicBool::new(false));
    let swapper = {
        let filter = Arc::clone(&filter);
        let stop = Arc::clone(&stop);
        tokio::spawn(async move {
            let mut swaps = 0_u32;
            while !stop.load(Ordering::SeqCst) {
                filter.replace(
                    Blueprint::new(Vec::new(), Arc::from("default"), Vec::new())
                        .build(&Default::default())
                        .expect("leerer Regelstand"),
                    0,
                );
                swaps = swaps.saturating_add(1);
                tokio::task::yield_now().await;
            }
            swaps
        })
    };

    let mut clients = Vec::new();
    for _ in 0..8 {
        let backend = Arc::clone(&backend);
        clients.push(tokio::spawn(async move {
            let mut slowest = Duration::ZERO;
            for _ in 0..500 {
                let started = std::time::Instant::now();
                backend
                    .resolve(&ask("ads.example.com."), &mut ctx())
                    .await
                    .expect("keine Anfrage darf während des Tauschs scheitern");
                slowest = slowest.max(started.elapsed());
            }
            slowest
        }));
    }

    let mut slowest = Duration::ZERO;
    for client in clients {
        slowest = slowest.max(client.await.expect("Client-Task"));
    }
    stop.store(true, Ordering::SeqCst);
    let swaps = swapper.await.expect("Tausch-Task");

    assert!(swaps > 0, "es wurde gar nicht getauscht");
    assert!(
        slowest < Duration::from_millis(50),
        "Latenzspitze von {slowest:?} während des Tauschs"
    );
}
