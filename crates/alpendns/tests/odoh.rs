//! Oblivious DoH: der Integrationstest aus Phase 7, Schritt 4.
//!
//! Aufgebaut wird die vollständige Kette aus RFC 9230, jeder Teil als eigener
//! Prozessteil auf Loopback:
//!
//! ```text
//! AlpenDNS ──ODoH-Nachricht──▶ Proxy ──weitergereicht──▶ Ziel
//!                              sieht:                    sieht:
//!                              unsere Adresse            die Frage
//!                              und Zielangabe            und den Proxy
//! ```
//!
//! Der Proxy hier ist bewusst *dumm*: er entschlüsselt nichts, er kann es auch
//! nicht — er hat den Schlüssel nicht. Genau das prüft der wichtigste Test
//! dieser Datei: was über den Proxy geht, enthält den Query-Namen nicht im
//! Klartext.
//!
//! Beide Fakes sprechen HTTP statt HTTPS. Für die Kryptografie der ODoH-Schicht
//! ist das egal — sie liegt *innerhalb* der HTTP-Nachricht —, und ein
//! TLS-Zertifikat für den Proxy würde nur den Aufbau vergrößern. Dass ein
//! `http://`-Proxy in der Konfiguration ein Startfehler ist, hält
//! `config.rs` fest.

#![allow(clippy::expect_used, clippy::indexing_slicing, clippy::panic)]

use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use alpendns::privacy;
use alpendns::resolve::ResolveBackend as _;
use alpendns::trace::Ctx;
use alpendns::upstream::odoh::OdohTransport;
use hickory_proto::op::{Message, MessageType, OpCode, Query, ResponseCode};
use hickory_proto::rr::rdata::A;
use hickory_proto::rr::{Name, RData, Record, RecordType};
use odoh_rs::{ObliviousDoHConfig, ObliviousDoHConfigs, ObliviousDoHKeyPair, ObliviousDoHMessage};
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
use tokio::net::TcpListener;

const TARGET_HOST: &str = "target.test.invalid";
const TARGET_PATH: &str = "/dns-query";
const ANSWER: [u8; 4] = [203, 0, 113, 9];

fn ctx() -> Ctx {
    Ctx::new(SocketAddr::from(([127, 0, 0, 1], 5555)))
}

fn question(name: &str) -> Message {
    let mut message = Message::new(0x4242, MessageType::Query, OpCode::Query);
    message.metadata.recursion_desired = true;
    message.add_query(Query::query(
        Name::from_ascii(name).expect("gültiger Name"),
        RecordType::A,
    ));
    message
}

// ---------------------------------------------------------------------------
// Ein sehr kleiner HTTP/1.1-Server
// ---------------------------------------------------------------------------

/// Was von einer HTTP-Anfrage gebraucht wird.
struct HttpRequest {
    method: String,
    target: String,
    body: Vec<u8>,
}

/// Liest genau eine Anfrage. Reicht: die Fakes halten keine Verbindung offen.
async fn read_request(stream: &mut tokio::net::TcpStream) -> Option<HttpRequest> {
    let mut buf = Vec::new();
    let mut chunk = [0_u8; 4096];
    loop {
        let read = stream.read(&mut chunk).await.ok()?;
        if read == 0 {
            break;
        }
        buf.extend_from_slice(chunk.get(..read)?);

        let head_end = buf.windows(4).position(|w| w == b"\r\n\r\n")?;
        let head = String::from_utf8_lossy(buf.get(..head_end)?).to_string();
        let length: usize = head
            .lines()
            .find_map(|line| {
                let (key, value) = line.split_once(':')?;
                key.eq_ignore_ascii_case("content-length")
                    .then(|| value.trim().parse().ok())?
            })
            .unwrap_or(0);

        let body_start = head_end.checked_add(4)?;
        if buf.len() < body_start.checked_add(length)? {
            continue;
        }
        let mut words = head.lines().next()?.split_whitespace();
        return Some(HttpRequest {
            method: words.next()?.to_owned(),
            target: words.next()?.to_owned(),
            body: buf
                .get(body_start..body_start.checked_add(length)?)?
                .to_vec(),
        });
    }
    None
}

async fn respond(stream: &mut tokio::net::TcpStream, status: u16, body: &[u8]) {
    let head = format!(
        "HTTP/1.1 {status} OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    );
    let _ = stream.write_all(head.as_bytes()).await;
    let _ = stream.write_all(body).await;
    let _ = stream.flush().await;
}

// ---------------------------------------------------------------------------
// Das Ziel
// ---------------------------------------------------------------------------

/// Was das Ziel gesehen hat.
#[derive(Debug, Default)]
struct TargetLog {
    queries: AtomicUsize,
    configs_served: AtomicUsize,
}

/// Ein ODoH-Ziel: veröffentlicht seinen Schlüssel und beantwortet Anfragen.
async fn target_fake(log: Arc<TargetLog>) -> SocketAddr {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let addr = listener.local_addr().expect("local_addr");
    // Ein Schlüsselpaar für die Lebensdauer des Fakes.
    let pair = Arc::new(ObliviousDoHKeyPair::new(&mut rand_adapter()));

    tokio::spawn(async move {
        while let Ok((mut stream, _)) = listener.accept().await {
            let pair = Arc::clone(&pair);
            let log = Arc::clone(&log);
            tokio::spawn(async move {
                let Some(request) = read_request(&mut stream).await else {
                    return;
                };
                if request.target.starts_with("/.well-known/odohconfigs") {
                    log.configs_served.fetch_add(1, Ordering::SeqCst);
                    let configs: ObliviousDoHConfigs =
                        vec![ObliviousDoHConfig::from(pair.public().clone())].into();
                    let bytes = odoh_rs::compose(&configs).expect("kodierbar").freeze();
                    respond(&mut stream, 200, &bytes).await;
                    return;
                }
                if request.method != "POST" {
                    respond(&mut stream, 405, b"").await;
                    return;
                }
                log.queries.fetch_add(1, Ordering::SeqCst);

                let mut cursor = request.body.as_slice();
                let encrypted: ObliviousDoHMessage =
                    odoh_rs::parse(&mut cursor).expect("ODoH-Nachricht");
                let (plaintext, secret) =
                    odoh_rs::decrypt_query(&encrypted, &pair).expect("entschlüsselbar");
                let query =
                    Message::from_vec(&plaintext.clone().into_msg()).expect("DNS-Nachricht");

                let mut answer = Message::response(query.metadata.id, OpCode::Query);
                answer.add_queries(query.queries.iter().cloned());
                if let Some(asked) = query.queries.first() {
                    answer.add_answer(Record::from_rdata(
                        asked.name().clone(),
                        60,
                        RData::A(A(std::net::Ipv4Addr::from(ANSWER))),
                    ));
                }
                let wire = answer.to_vec().expect("kodierbar");
                let response = odoh_rs::ObliviousDoHMessagePlaintext::new(&wire, 0);
                let encrypted = odoh_rs::encrypt_response(
                    &plaintext,
                    &response,
                    secret,
                    odoh_rs::ResponseNonce::default(),
                )
                .expect("verschlüsselbar");
                let body = odoh_rs::compose(&encrypted).expect("kodierbar").freeze();
                respond(&mut stream, 200, &body).await;
            });
        }
    });
    addr
}

// ---------------------------------------------------------------------------
// Der Proxy
// ---------------------------------------------------------------------------

/// Was der Proxy zu sehen bekam. Genau das, was er *nicht* sehen darf, wird
/// hier aufgehoben, um es hinterher zu durchsuchen.
#[derive(Debug, Default)]
struct ProxyLog {
    forwarded: AtomicUsize,
    /// Die Zieladresse, die uns der Client mitgeteilt hat.
    targets: std::sync::Mutex<Vec<String>>,
    /// Die Nachrichten, so wie sie durch den Proxy gingen.
    bodies: std::sync::Mutex<Vec<Vec<u8>>>,
}

/// Ein ODoH-Proxy: liest `targethost`/`targetpath` und reicht den Körper weiter.
async fn proxy_fake(log: Arc<ProxyLog>, target: SocketAddr) -> SocketAddr {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let addr = listener.local_addr().expect("local_addr");

    tokio::spawn(async move {
        while let Ok((mut stream, _)) = listener.accept().await {
            let log = Arc::clone(&log);
            tokio::spawn(async move {
                let Some(request) = read_request(&mut stream).await else {
                    return;
                };
                let Some((_, query)) = request.target.split_once('?') else {
                    respond(&mut stream, 400, b"").await;
                    return;
                };
                log.forwarded.fetch_add(1, Ordering::SeqCst);
                log.targets.lock().expect("Lock").push(query.to_owned());
                log.bodies.lock().expect("Lock").push(request.body.clone());

                // Der Proxy weiß nur, wohin — nicht was. Er reicht die Bytes
                // unverändert weiter.
                let forwarded = forward(target, &request.body).await;
                match forwarded {
                    Some(body) => respond(&mut stream, 200, &body).await,
                    None => respond(&mut stream, 502, b"").await,
                }
            });
        }
    });
    addr
}

/// Reicht einen Körper an das Ziel weiter und gibt dessen Antwort zurück.
async fn forward(target: SocketAddr, body: &[u8]) -> Option<Vec<u8>> {
    let mut stream = tokio::net::TcpStream::connect(target).await.ok()?;
    let head = format!(
        "POST {TARGET_PATH} HTTP/1.1\r\nHost: {TARGET_HOST}\r\nContent-Type: \
         application/oblivious-dns-message\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    );
    stream.write_all(head.as_bytes()).await.ok()?;
    stream.write_all(body).await.ok()?;
    stream.flush().await.ok()?;

    let mut answer = Vec::new();
    stream.read_to_end(&mut answer).await.ok()?;
    let head_end = answer.windows(4).position(|w| w == b"\r\n\r\n")?;
    Some(answer.get(head_end.checked_add(4)?..)?.to_vec())
}

// ---------------------------------------------------------------------------
// Zufall für den Fake
// ---------------------------------------------------------------------------

/// `ObliviousDoHKeyPair::new` verlangt einen RNG aus `hpke::rand_core`.
struct Rng;

impl hpke::rand_core::RngCore for Rng {
    fn next_u32(&mut self) -> u32 {
        rand::random()
    }
    fn next_u64(&mut self) -> u64 {
        rand::random()
    }
    fn fill_bytes(&mut self, dest: &mut [u8]) {
        for byte in dest.iter_mut() {
            *byte = rand::random();
        }
    }
}

impl hpke::rand_core::CryptoRng for Rng {}

const fn rand_adapter() -> Rng {
    Rng
}

// ---------------------------------------------------------------------------
// Aufbau
// ---------------------------------------------------------------------------

struct Setup {
    transport: OdohTransport,
    proxy: Arc<ProxyLog>,
    target: Arc<TargetLog>,
}

async fn setup() -> Setup {
    let target_log = Arc::new(TargetLog::default());
    let target_addr = target_fake(Arc::clone(&target_log)).await;
    let proxy_log = Arc::new(ProxyLog::default());
    let proxy_addr = proxy_fake(Arc::clone(&proxy_log), target_addr).await;

    // Der Schlüsselabruf geht direkt zum Ziel, nicht über den Proxy — das ist
    // die dokumentierte Grenze von ODoH (ADR-0017). Im Betrieb wäre das
    // `well_known_config_url(host)` über https; hier zeigt es auf den Fake.
    let transport = OdohTransport::new(
        format!("http://{proxy_addr}/proxy"),
        TARGET_HOST,
        TARGET_PATH,
        target_addr,
        format!("http://{target_addr}/.well-known/odohconfigs"),
        Duration::from_secs(5),
        privacy::Settings {
            // Die Validierung hat ihre eigene Testdatei; hier würde sie den
            // Fake nur dazu bringen, eine Kette zu suchen, die es nicht gibt.
            dnssec: false,
            ..privacy::Settings::default()
        },
    )
    .expect("Transport baubar");

    Setup {
        transport,
        proxy: proxy_log,
        target: target_log,
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_query_travels_through_the_proxy_to_the_target() {
    let setup = setup().await;
    let response = setup
        .transport
        .resolve(&question("example.com."), &mut ctx())
        .await
        .expect("ODoH-Antwort");

    assert_eq!(response.metadata.response_code, ResponseCode::NoError);
    assert_eq!(
        response.metadata.id, 0x4242,
        "der Client bekommt seine eigene Query-ID zurück"
    );
    let addresses: Vec<std::net::Ipv4Addr> = response
        .answers
        .iter()
        .filter_map(|record| match &record.data {
            RData::A(a) => Some(a.0),
            _ => None,
        })
        .collect();
    assert_eq!(addresses, vec![std::net::Ipv4Addr::from(ANSWER)]);

    assert_eq!(setup.proxy.forwarded.load(Ordering::SeqCst), 1);
    assert_eq!(setup.target.queries.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn the_proxy_never_sees_the_query_name() {
    // Der Kern des Ganzen. Der Proxy kennt unsere Adresse; wenn er zusätzlich
    // den Namen sähe, wäre ODoH ein aufwendiger DoH-Umweg und sonst nichts.
    let setup = setup().await;
    setup
        .transport
        .resolve(&question("geheim.example.org."), &mut ctx())
        .await
        .expect("ODoH-Antwort");

    let bodies = setup.proxy.bodies.lock().expect("Lock").clone();
    assert_eq!(bodies.len(), 1, "der Proxy hat nichts gesehen");
    for body in &bodies {
        assert!(!body.is_empty());
        for needle in [
            b"geheim".as_slice(),
            b"example".as_slice(),
            b"org".as_slice(),
        ] {
            assert!(
                !body.windows(needle.len()).any(|window| window == needle),
                "'{}' stand im Klartext in der Nachricht an den Proxy",
                String::from_utf8_lossy(needle)
            );
        }
    }

    // Was der Proxy sehr wohl weiß: wohin es geht. Das ist der Preis.
    let targets = setup.proxy.targets.lock().expect("Lock").clone();
    assert!(
        targets.iter().all(|t| t.contains("targethost=")),
        "{targets:?}"
    );
}

#[tokio::test]
async fn the_target_key_is_fetched_once_and_then_kept() {
    // Jede Anfrage neu zu holen wäre eine zweite Verbindung je Query — und ein
    // Muster, an dem das Ziel unsere Aktivität ablesen könnte.
    let setup = setup().await;
    for _ in 0..5 {
        setup
            .transport
            .resolve(&question("example.com."), &mut ctx())
            .await
            .expect("ODoH-Antwort");
    }
    assert_eq!(setup.target.configs_served.load(Ordering::SeqCst), 1);
    assert_eq!(setup.target.queries.load(Ordering::SeqCst), 5);
}

#[tokio::test]
async fn a_tampered_response_is_refused_instead_of_returned() {
    // Der Proxy ist der Einzige auf dem Weg, der eine Antwort verändern könnte.
    // Er hat den Schlüssel nicht, also darf ihm das nicht gelingen.
    let target_log = Arc::new(TargetLog::default());
    let target_addr = target_fake(Arc::clone(&target_log)).await;

    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let proxy_addr = listener.local_addr().expect("local_addr");
    tokio::spawn(async move {
        while let Ok((mut stream, _)) = listener.accept().await {
            tokio::spawn(async move {
                let Some(request) = read_request(&mut stream).await else {
                    return;
                };
                let Some(mut body) = forward(target_addr, &request.body).await else {
                    respond(&mut stream, 502, b"").await;
                    return;
                };
                // Ein Byte im Ciphertext umdrehen.
                if let Some(last) = body.last_mut() {
                    *last ^= 0xff;
                }
                respond(&mut stream, 200, &body).await;
            });
        }
    });

    let transport = OdohTransport::new(
        format!("http://{proxy_addr}/proxy"),
        TARGET_HOST,
        TARGET_PATH,
        target_addr,
        format!("http://{target_addr}/.well-known/odohconfigs"),
        Duration::from_secs(5),
        privacy::Settings {
            dnssec: false,
            ..privacy::Settings::default()
        },
    )
    .expect("Transport baubar");

    let error = transport
        .resolve(&question("example.com."), &mut ctx())
        .await
        .expect_err("eine veränderte Antwort darf nicht durchgehen");
    assert!(
        matches!(error, alpendns::resolve::ResolveError::Malformed(_)),
        "{error:?}"
    );
}

#[tokio::test]
async fn a_proxy_that_is_gone_reports_an_error_instead_of_hanging() {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let dead = listener.local_addr().expect("local_addr");
    drop(listener);

    let target_log = Arc::new(TargetLog::default());
    let target_addr = target_fake(target_log).await;
    let transport = OdohTransport::new(
        format!("http://{dead}/proxy"),
        TARGET_HOST,
        TARGET_PATH,
        target_addr,
        format!("http://{target_addr}/.well-known/odohconfigs"),
        Duration::from_millis(500),
        privacy::Settings {
            dnssec: false,
            ..privacy::Settings::default()
        },
    )
    .expect("Transport baubar");

    let error = transport
        .resolve(&question("example.com."), &mut ctx())
        .await
        .expect_err("ein toter Proxy ist ein Fehler");
    assert!(
        !matches!(error, alpendns::resolve::ResolveError::Mismatch(_)),
        "{error:?}"
    );
}
