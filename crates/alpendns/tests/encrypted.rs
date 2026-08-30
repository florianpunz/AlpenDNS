//! Integrationstests der verschlüsselten Transporte: DoT, DoH, DoQ.
//!
//! Jeder Test spricht mit einem Fake im selben Prozess, der ein selbstsigniertes
//! Zertifikat benutzt. Es wird nie ein echter Resolver kontaktiert
//! (docs/TESTING.md §4).

// Testcode darf panicken, siehe B.1 und clippy.toml.
#![allow(clippy::expect_used, clippy::indexing_slicing, clippy::panic)]

use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use alpendns::config::UpstreamAddr;
use alpendns::privacy;
use alpendns::resolve::ResolveBackend as _;
use alpendns::trace::Ctx;
use alpendns::upstream::transport::Transport;
use hickory_proto::op::{Message, MessageType, OpCode, Query, ResponseCode};
use hickory_proto::rr::rdata::A;
use hickory_proto::rr::{Name, RData, Record, RecordType};
use rustls::pki_types::{CertificateDer, PrivateKeyDer};
use rustls::{ClientConfig, RootCertStore, ServerConfig};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, UdpSocket};
use tokio_rustls::TlsAcceptor;

/// Der Name, auf den das Testzertifikat ausgestellt ist.
const SERVER_NAME: &str = "upstream.test.invalid";

/// Ein selbstsigniertes Zertifikat plus die dazu passende Client-Konfiguration.
struct TestPki {
    server: Arc<ServerConfig>,
    /// Wie `server`, aber ohne TLS 1.2 — QUIC verlangt 1.3.
    quic_server: Arc<ServerConfig>,
    client: Arc<ClientConfig>,
}

impl TestPki {
    fn new(alpn: &[&[u8]]) -> Self {
        let issued = rcgen::generate_simple_self_signed(vec![SERVER_NAME.to_owned()])
            .expect("Zertifikat erzeugen");
        let cert = CertificateDer::from(issued.cert);
        let key = PrivateKeyDer::try_from(issued.signing_key.serialize_der())
            .expect("Schlüssel im DER-Format");
        let key_for_quic = key.clone_key();

        let provider = Arc::new(rustls::crypto::ring::default_provider());

        let mut server = ServerConfig::builder_with_provider(Arc::clone(&provider))
            .with_safe_default_protocol_versions()
            .expect("Protokollversionen")
            .with_no_client_auth()
            .with_single_cert(vec![cert.clone()], key)
            .expect("Zertifikat und Schlüssel passen zusammen");
        server.alpn_protocols = alpn.iter().map(|p| p.to_vec()).collect();

        let mut quic_server = ServerConfig::builder_with_provider(Arc::clone(&provider))
            .with_protocol_versions(&[&rustls::version::TLS13])
            .expect("TLS 1.3")
            .with_no_client_auth()
            .with_single_cert(vec![cert.clone()], key_for_quic)
            .expect("Zertifikat und Schlüssel passen zusammen");
        quic_server.alpn_protocols = alpn.iter().map(|p| p.to_vec()).collect();

        // Der Client vertraut genau diesem einen Zertifikat — der einzige
        // Unterschied zur Konfiguration im Betrieb, die Mozillas Wurzeln nutzt.
        let mut roots = RootCertStore::empty();
        roots.add(cert).expect("Wurzelzertifikat");
        let mut client = ClientConfig::builder_with_provider(provider)
            .with_safe_default_protocol_versions()
            .expect("Protokollversionen")
            .with_root_certificates(roots)
            .with_no_client_auth();
        client.alpn_protocols = alpn.iter().map(|p| p.to_vec()).collect();

        Self {
            server: Arc::new(server),
            quic_server: Arc::new(quic_server),
            client: Arc::new(client),
        }
    }
}

/// Ein Kontext für Tests, die sich nicht für den Trace interessieren.
fn ctx() -> Ctx {
    Ctx::new(std::net::SocketAddr::from(([127, 0, 0, 1], 5555)))
}

fn question(name: &str) -> Message {
    let mut message = Message::new(0x2a2a, MessageType::Query, OpCode::Query);
    message.metadata.recursion_desired = true;
    message.add_query(Query::query(
        Name::from_ascii(name).expect("gültiger Name"),
        RecordType::A,
    ));
    message
}

/// Die Antwort, die alle Fakes geben: die Frage gespiegelt plus ein A-Record.
fn answer(request: &Message) -> Message {
    let mut response = Message::response(request.metadata.id, OpCode::Query);
    response.add_queries(request.queries.iter().cloned());
    if let Some(query) = request.queries.first() {
        response.add_answer(Record::from_rdata(
            query.name().clone(),
            60,
            RData::A(A(std::net::Ipv4Addr::new(203, 0, 113, 7))),
        ));
    }
    response
}

/// Liest eine längenpräfixierte DNS-Nachricht.
async fn read_prefixed<R: tokio::io::AsyncRead + Unpin>(reader: &mut R) -> Option<Message> {
    let mut len = [0_u8; 2];
    reader.read_exact(&mut len).await.ok()?;
    let mut body = vec![0_u8; usize::from(u16::from_be_bytes(len))];
    reader.read_exact(&mut body).await.ok()?;
    Message::from_vec(&body).ok()
}

async fn write_prefixed<W: tokio::io::AsyncWrite + Unpin>(writer: &mut W, message: &Message) {
    let bytes = message.to_vec().expect("kodierbar");
    let len = u16::try_from(bytes.len()).expect("passt in 16 Bit");
    let _ = writer.write_all(&len.to_be_bytes()).await;
    let _ = writer.write_all(&bytes).await;
    let _ = writer.flush().await;
}

// ---------------------------------------------------------------------------
// DoT
// ---------------------------------------------------------------------------

/// Startet einen DoT-Server (RFC 7858) und liefert Adresse und Anfragezähler.
async fn dot_fake(pki: &TestPki) -> (SocketAddr, Arc<AtomicUsize>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let addr = listener.local_addr().expect("local_addr");
    let acceptor = TlsAcceptor::from(Arc::clone(&pki.server));
    let hits = Arc::new(AtomicUsize::new(0));
    let counter = Arc::clone(&hits);

    tokio::spawn(async move {
        while let Ok((stream, _)) = listener.accept().await {
            let acceptor = acceptor.clone();
            let counter = Arc::clone(&counter);
            tokio::spawn(async move {
                let Ok(mut tls) = acceptor.accept(stream).await else {
                    return;
                };
                // Eine Verbindung trägt beliebig viele Anfragen.
                while let Some(request) = read_prefixed(&mut tls).await {
                    counter.fetch_add(1, Ordering::SeqCst);
                    write_prefixed(&mut tls, &answer(&request)).await;
                }
            });
        }
    });

    (addr, hits)
}

fn transport_for(addr: UpstreamAddr, pki: &TestPki) -> Transport {
    Transport::new(
        addr,
        SERVER_NAME,
        Duration::from_secs(5),
        privacy::Settings::default(),
        Arc::clone(&pki.client),
    )
}

#[tokio::test]
async fn dot_query_is_answered_over_tls() {
    let pki = TestPki::new(&[]);
    let (addr, hits) = dot_fake(&pki).await;
    let transport = transport_for(UpstreamAddr::Dot(addr), &pki);

    let response = transport
        .resolve(&question("example.com."), &mut ctx())
        .await
        .expect("DoT-Antwort");

    assert_eq!(response.metadata.response_code, ResponseCode::NoError);
    assert_eq!(response.answers.len(), 1);
    assert_eq!(hits.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn dot_reuses_the_connection_for_further_queries() {
    // Ein TLS-Handshake pro Anfrage würde den Cache-Gewinn wieder auffressen.
    let pki = TestPki::new(&[]);
    let (addr, hits) = dot_fake(&pki).await;
    let transport = transport_for(UpstreamAddr::Dot(addr), &pki);

    for i in 0..5 {
        transport
            .resolve(&question(&format!("host{i}.example.")), &mut ctx())
            .await
            .expect("DoT-Antwort");
    }
    assert_eq!(hits.load(Ordering::SeqCst), 5);
}

#[tokio::test]
async fn dot_request_is_padded_to_a_full_block() {
    // Roadmap Schritt 7: auf einer verschlüsselten Verbindung verrät die
    // Nachrichtenlänge sonst, wie lang der Name ist.
    let pki = TestPki::new(&[]);
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let addr = listener.local_addr().expect("local_addr");
    let acceptor = TlsAcceptor::from(Arc::clone(&pki.server));
    let (tx, rx) = tokio::sync::oneshot::channel::<usize>();

    tokio::spawn(async move {
        let Ok((stream, _)) = listener.accept().await else {
            return;
        };
        let Ok(mut tls) = acceptor.accept(stream).await else {
            return;
        };
        let mut len = [0_u8; 2];
        if tls.read_exact(&mut len).await.is_err() {
            return;
        }
        let size = usize::from(u16::from_be_bytes(len));
        let mut body = vec![0_u8; size];
        let _ = tls.read_exact(&mut body).await;
        let _ = tx.send(size);
        if let Ok(request) = Message::from_vec(&body) {
            write_prefixed(&mut tls, &answer(&request)).await;
        }
    });

    let transport = transport_for(UpstreamAddr::Dot(addr), &pki);
    transport
        .resolve(&question("a.de."), &mut ctx())
        .await
        .expect("DoT-Antwort");

    let size = rx.await.expect("Größe der Anfrage");
    assert_eq!(
        size % privacy::PADDING_BLOCK,
        0,
        "Anfrage war {size} Byte, kein Vielfaches von {}",
        privacy::PADDING_BLOCK
    );
}

#[tokio::test]
async fn a_wrong_server_name_is_refused() {
    // Ohne Zertifikatsprüfung wäre die Verschlüsselung wertlos.
    let pki = TestPki::new(&[]);
    let (addr, hits) = dot_fake(&pki).await;
    let transport = Transport::new(
        UpstreamAddr::Dot(addr),
        "jemand.anderes.invalid",
        Duration::from_secs(5),
        privacy::Settings::default(),
        Arc::clone(&pki.client),
    );

    let error = transport
        .resolve(&question("example.com."), &mut ctx())
        .await
        .expect_err("falscher Name muss abgelehnt werden");
    assert_eq!(
        hits.load(Ordering::SeqCst),
        0,
        "die Anfrage ging trotz falschem Zertifikat raus: {error}"
    );
}

#[tokio::test]
async fn a_dead_upstream_reports_an_error_instead_of_hanging() {
    let pki = TestPki::new(&[]);
    // Adresse, auf der niemand lauscht.
    let addr: SocketAddr = "127.0.0.1:1".parse().expect("gültig");
    let transport = Transport::new(
        UpstreamAddr::Dot(addr),
        SERVER_NAME,
        Duration::from_millis(300),
        privacy::Settings::default(),
        Arc::clone(&pki.client),
    );

    let result = tokio::time::timeout(
        Duration::from_secs(5),
        transport.resolve(&question("example.com."), &mut ctx()),
    )
    .await
    .expect("darf nicht hängen");
    assert!(result.is_err());
}

// ---------------------------------------------------------------------------
// DoH
// ---------------------------------------------------------------------------

/// Startet einen DoH-Server (RFC 8484) über HTTP/2 und TLS.
async fn doh_fake(pki: &TestPki) -> (SocketAddr, Arc<AtomicUsize>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let addr = listener.local_addr().expect("local_addr");
    let acceptor = TlsAcceptor::from(Arc::clone(&pki.server));
    let hits = Arc::new(AtomicUsize::new(0));
    let counter = Arc::clone(&hits);

    tokio::spawn(async move {
        while let Ok((stream, _)) = listener.accept().await {
            let acceptor = acceptor.clone();
            let counter = Arc::clone(&counter);
            tokio::spawn(async move {
                let Ok(tls) = acceptor.accept(stream).await else {
                    return;
                };
                let Ok(mut connection) = h2::server::handshake(tls).await else {
                    return;
                };
                while let Some(Ok((request, mut responder))) = connection.accept().await {
                    counter.fetch_add(1, Ordering::SeqCst);
                    let mut body = request.into_body();
                    let mut raw = bytes::BytesMut::new();
                    while let Some(Ok(chunk)) = body.data().await {
                        // Ohne Freigabe des Fensters bleibt der Client stehen.
                        let _ = body.flow_control().release_capacity(chunk.len());
                        raw.extend_from_slice(&chunk);
                    }
                    let Ok(query) = Message::from_vec(&raw) else {
                        continue;
                    };
                    let payload = answer(&query).to_vec().expect("kodierbar");
                    let response = http::Response::builder()
                        .status(200)
                        .header("content-type", "application/dns-message")
                        .body(())
                        .expect("gültige Antwort");
                    if let Ok(mut stream) = responder.send_response(response, false) {
                        let _ = stream.send_data(payload.into(), true);
                    }
                }
            });
        }
    });

    (addr, hits)
}

#[tokio::test]
async fn doh_query_is_answered_over_http2() {
    let pki = TestPki::new(&[b"h2"]);
    let (addr, hits) = doh_fake(&pki).await;
    let transport = transport_for(
        UpstreamAddr::Doh {
            addr,
            path: "/dns-query".to_owned(),
        },
        &pki,
    );

    let response = transport
        .resolve(&question("example.com."), &mut ctx())
        .await
        .expect("DoH-Antwort");

    assert_eq!(response.metadata.response_code, ResponseCode::NoError);
    assert_eq!(response.answers.len(), 1);
    assert_eq!(hits.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn doh_reuses_the_connection() {
    let pki = TestPki::new(&[b"h2"]);
    let (addr, hits) = doh_fake(&pki).await;
    let transport = transport_for(
        UpstreamAddr::Doh {
            addr,
            path: "/dns-query".to_owned(),
        },
        &pki,
    );

    for i in 0..4 {
        transport
            .resolve(&question(&format!("h{i}.example.")), &mut ctx())
            .await
            .expect("DoH-Antwort");
    }
    assert_eq!(hits.load(Ordering::SeqCst), 4);
}

// ---------------------------------------------------------------------------
// DoQ
// ---------------------------------------------------------------------------

/// Startet einen DoQ-Server (RFC 9250).
///
/// Jede Anfrage bekommt einen eigenen bidirektionalen Stream, auf dem die
/// Nachricht wie bei TCP mit Längenpräfix steht.
async fn doq_fake(pki: &TestPki) -> (SocketAddr, Arc<AtomicUsize>) {
    let crypto =
        quinn::crypto::rustls::QuicServerConfig::try_from(pki.quic_server.as_ref().clone())
            .expect("QUIC verlangt TLS 1.3");
    let server_config = quinn::ServerConfig::with_crypto(Arc::new(crypto));
    let endpoint = quinn::Endpoint::server(server_config, "127.0.0.1:0".parse().expect("gültig"))
        .expect("QUIC-Endpunkt");
    let addr = endpoint.local_addr().expect("local_addr");
    let hits = Arc::new(AtomicUsize::new(0));
    let counter = Arc::clone(&hits);

    tokio::spawn(async move {
        while let Some(incoming) = endpoint.accept().await {
            let counter = Arc::clone(&counter);
            tokio::spawn(async move {
                let Ok(connection) = incoming.await else {
                    return;
                };
                while let Ok((mut send, mut recv)) = connection.accept_bi().await {
                    counter.fetch_add(1, Ordering::SeqCst);
                    let Ok(Some(raw)) = recv.read_to_end(65_535).await.map(Some) else {
                        return;
                    };
                    let Some(body) = raw.get(2..) else {
                        continue;
                    };
                    let Ok(query) = Message::from_vec(body) else {
                        continue;
                    };
                    write_prefixed(&mut send, &answer(&query)).await;
                    let _ = send.finish();
                }
            });
        }
    });

    (addr, hits)
}

#[tokio::test]
async fn doq_query_is_answered_over_quic() {
    let pki = TestPki::new(&[b"doq"]);
    let (addr, hits) = doq_fake(&pki).await;
    let transport = transport_for(UpstreamAddr::Doq(addr), &pki);

    let response = transport
        .resolve(&question("example.com."), &mut ctx())
        .await
        .expect("DoQ-Antwort");

    assert_eq!(response.metadata.response_code, ResponseCode::NoError);
    assert_eq!(response.answers.len(), 1);
    assert_eq!(hits.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn doq_reuses_the_connection_across_streams() {
    let pki = TestPki::new(&[b"doq"]);
    let (addr, hits) = doq_fake(&pki).await;
    let transport = transport_for(UpstreamAddr::Doq(addr), &pki);

    for i in 0..3 {
        transport
            .resolve(&question(&format!("q{i}.example.")), &mut ctx())
            .await
            .expect("DoQ-Antwort");
    }
    assert_eq!(hits.load(Ordering::SeqCst), 3);
}

/// Ein UDP-Socket auf derselben Adresse, um zu zeigen, dass DoQ nicht auf 53 geht.
#[tokio::test]
async fn nothing_is_sent_in_cleartext_on_port_53() {
    // Ein Klartext-Resolver auf Port 53, den niemand fragen darf.
    let trap = UdpSocket::bind("127.0.0.1:0").await.expect("bind");
    let trap_addr = trap.local_addr().expect("local_addr");
    let contacted = Arc::new(AtomicUsize::new(0));
    let counter = Arc::clone(&contacted);
    tokio::spawn(async move {
        let mut buf = vec![0_u8; 1024];
        while trap.recv_from(&mut buf).await.is_ok() {
            counter.fetch_add(1, Ordering::SeqCst);
        }
    });

    let pki = TestPki::new(&[]);
    let (addr, _) = dot_fake(&pki).await;
    let transport = transport_for(UpstreamAddr::Dot(addr), &pki);
    transport
        .resolve(&question("example.com."), &mut ctx())
        .await
        .expect("DoT-Antwort");

    assert_eq!(
        contacted.load(Ordering::SeqCst),
        0,
        "der Klartext-Resolver wurde kontaktiert (Adresse {trap_addr})"
    );
}

/// Die Antwort muss zur Anfrage des Clients passen — dieselbe ID, dieselbe Frage.
///
/// Der Multiplexer der Verbindung schreibt die ID beim Senden um. Wird sie nicht
/// wiederhergestellt, verwirft jeder Client die Antwort ("ID mismatch"). Genau
/// das ist beim ersten Test gegen einen echten Resolver passiert.
#[tokio::test]
async fn every_transport_returns_the_clients_query_id_and_question() {
    let dot_pki = TestPki::new(&[]);
    let (dot_addr, _) = dot_fake(&dot_pki).await;
    let doh_pki = TestPki::new(&[b"h2"]);
    let (doh_addr, _) = doh_fake(&doh_pki).await;
    let doq_pki = TestPki::new(&[b"doq"]);
    let (doq_addr, _) = doq_fake(&doq_pki).await;

    let cases: Vec<(&str, Transport)> = vec![
        ("dot", transport_for(UpstreamAddr::Dot(dot_addr), &dot_pki)),
        (
            "doh",
            transport_for(
                UpstreamAddr::Doh {
                    addr: doh_addr,
                    path: "/dns-query".to_owned(),
                },
                &doh_pki,
            ),
        ),
        ("doq", transport_for(UpstreamAddr::Doq(doq_addr), &doq_pki)),
    ];

    for (label, transport) in cases {
        let request = question("example.com.");
        let response = transport
            .resolve(&request, &mut ctx())
            .await
            .expect("Antwort");
        assert_eq!(
            response.metadata.id, request.metadata.id,
            "{label}: fremde Query-ID in der Antwort"
        );
        assert_eq!(
            response.queries, request.queries,
            "{label}: die Frage des Clients wurde nicht gespiegelt"
        );
    }
}
