//! Prozess-Test für den SIGHUP-Konfigurations-Reload.
//!
//! Spawnt das echte Binary (`alpendns -c …`), schickt `kill -HUP` und prüft
//! den Vertrag aus ARCHITECTURE.md §7: eine kaputte Konfiguration lässt den
//! alten Stand stehen und der Server antwortet weiter; eine gültige Änderung
//! (neue Blockliste) greift, sobald der Reload durch ist. Der Upstream ist ein
//! lokaler Fake über `forward_zone` — es geht nie ins Netz (docs/TESTING.md §4).

// Testcode darf laut B.1 panicken. `clippy.toml` erlaubt das nur in
// #[cfg(test)]-Modulen — ein Integrationstest ist ein eigenes Crate und fällt
// nicht darunter, deshalb hier ausdrücklich.
#![allow(clippy::expect_used, clippy::indexing_slicing, clippy::panic)]

use std::net::{Ipv4Addr, SocketAddr};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use hickory_proto::op::{Message, MessageType, OpCode, Query, ResponseCode};
use hickory_proto::rr::rdata::A;
use hickory_proto::rr::{Name, RData, Record, RecordType};
use tokio::net::UdpSocket;

/// Ein frisches Verzeichnis für Config und Listen-Dateien, das am Ende weggeräumt wird.
struct TempDir(PathBuf);

impl TempDir {
    fn new(tag: &str) -> Self {
        let path = std::env::temp_dir().join(format!(
            "alpendns-reload-proc-{tag}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("Uhr nach 1970")
                .as_nanos()
        ));
        std::fs::create_dir_all(&path).expect("Testverzeichnis");
        TempDir(path)
    }

    /// Schreibt eine Datei und gibt ihren Pfad als String zurück.
    fn file(&self, name: &str, content: &str) -> String {
        let path = self.0.join(name);
        std::fs::write(&path, content).expect("Datei schreiben");
        path.to_string_lossy().into_owned()
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// Ein Loopback-Port, auf dem weder UDP noch TCP belegt ist.
fn free_port() -> u16 {
    for _ in 0..100 {
        let udp = std::net::UdpSocket::bind("127.0.0.1:0").expect("UDP-Socket");
        let port = udp.local_addr().expect("Adresse").port();
        if std::net::TcpListener::bind(("127.0.0.1", port)).is_ok() {
            return port;
        }
    }
    panic!("kein freier UDP+TCP-Port gefunden");
}

/// Eine minimale Konfiguration. `fake` hängt eine `forward_zone` für die Zone
/// `example` an einen lokalen Fake-Upstream; ohne sie verlangt die Konfiguration
/// nur den (nie kontaktierten) dot-Pool.
fn config_text(port: u16, fake: Option<SocketAddr>, lists: &[(&str, &str)]) -> String {
    let mut text = format!(
        "[server]\nlisten_udp = [\"127.0.0.1:{port}\"]\nlisten_tcp = [\"127.0.0.1:{port}\"]\n\n"
    );
    text.push_str("[[upstream_pool]]\nname = \"default\"\n\n");
    text.push_str(
        "[[upstream_pool.resolver]]\nname = \"pool\"\naddr = \"dot://127.0.0.1:853\"\n\
         tls_name = \"dns.example.net\"\n",
    );
    if let Some(fake) = fake {
        text.push_str(&format!(
            "\n[[forward_zone]]\nzone = \"example\"\nupstream = \"udp://{fake}\"\n"
        ));
    }
    for (list_name, path) in lists {
        text.push_str(&format!(
            "\n[[blocklist]]\nname = \"{list_name}\"\npath = \"{path}\"\nformat = \"wildcard\"\n"
        ));
    }
    text
}

fn name(s: &str) -> Name {
    Name::from_ascii(s).expect("gültiger Name")
}

fn query_message(qname: &str, qtype: RecordType) -> Message {
    let mut msg = Message::new(0x4d2, MessageType::Query, OpCode::Query);
    msg.metadata.recursion_desired = true;
    msg.add_query(Query::query(name(qname), qtype));
    msg
}

/// Ein lokaler Fake-Upstream, der jede Anfrage mit einem A-Record beantwortet.
async fn fake_upstream() -> SocketAddr {
    let socket = UdpSocket::bind("127.0.0.1:0").await.expect("bind");
    let addr = socket.local_addr().expect("Adresse");
    tokio::spawn(async move {
        let mut buf = vec![0_u8; 4096];
        while let Ok((len, peer)) = socket.recv_from(&mut buf).await {
            let Ok(request) = Message::from_vec(&buf[..len]) else {
                continue;
            };
            let mut response = Message::response(request.metadata.id, OpCode::Query);
            response.add_queries(request.queries.iter().cloned());
            if let Some(qname) = request.queries.first().map(|q| q.name().clone()) {
                response.add_answer(Record::from_rdata(
                    qname,
                    60,
                    RData::A(A(Ipv4Addr::new(93, 184, 216, 34))),
                ));
            }
            if let Ok(bytes) = response.to_vec() {
                let _ = socket.send_to(&bytes, peer).await;
            }
        }
    });
    addr
}

/// Der laufende Server-Prozess samt Log-Datei. Beendet sich im Drop sauber.
struct Server {
    child: Child,
    log: String,
}

impl Server {
    fn start(config_path: &str, log_path: &str) -> Server {
        let log = std::fs::File::create(log_path).expect("Log-Datei");
        let child = Command::new(env!("CARGO_BIN_EXE_alpendns"))
            .arg("-c")
            .arg(config_path)
            .stdout(Stdio::from(log.try_clone().expect("clone stdout")))
            .stderr(Stdio::from(log))
            .spawn()
            .expect("Server starten");
        Server {
            child,
            log: log_path.to_owned(),
        }
    }

    fn pid(&self) -> u32 {
        self.child.id()
    }

    /// SIGTERM, dann bis zu fünf Sekunden warten, notfalls hart beenden.
    fn shutdown(&mut self) {
        match self.child.try_wait() {
            Ok(Some(_)) => {}
            Ok(None) => {
                let pid = self.child.id();
                let _ = Command::new("kill")
                    .arg("-TERM")
                    .arg(pid.to_string())
                    .status();
                let deadline = Instant::now() + Duration::from_secs(5);
                loop {
                    match self.child.try_wait() {
                        Ok(Some(_)) => break,
                        _ if Instant::now() > deadline => {
                            let _ = self.child.kill();
                            break;
                        }
                        _ => std::thread::sleep(Duration::from_millis(50)),
                    }
                }
                let _ = self.child.wait();
            }
            Err(_) => {}
        }
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        self.shutdown();
    }
}

fn hangup(pid: u32) {
    let status = Command::new("kill")
        .arg("-HUP")
        .arg(pid.to_string())
        .status()
        .expect("kill ausführen");
    assert!(status.success(), "kill -HUP schlug fehl");
}

/// Fragt `domain`, bis eine Antwort mit `want` kommt, und gibt sie zurück.
async fn query_until(port: u16, domain: &str, want: ResponseCode) -> Message {
    let server = SocketAddr::from(([127, 0, 0, 1], port));
    let packet = query_message(domain, RecordType::A)
        .to_vec()
        .expect("kodierbar");
    let deadline = Instant::now() + Duration::from_secs(10);
    let mut last = None;
    loop {
        let socket = UdpSocket::bind("127.0.0.1:0").await.expect("bind");
        socket.connect(server).await.expect("connect");
        socket.send(&packet).await.expect("send");
        let mut buf = vec![0_u8; 4096];
        if let Ok(Ok(len)) =
            tokio::time::timeout(Duration::from_millis(500), socket.recv(&mut buf)).await
        {
            buf.truncate(len);
            if let Ok(response) = Message::from_vec(&buf) {
                if response.metadata.response_code == want {
                    return response;
                }
                last = Some(response.metadata.response_code);
            }
        }
        if Instant::now() > deadline {
            panic!("{domain}: erwartet {want:?}, zuletzt {last:?}");
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

/// Wartet, bis `needle` im Log erscheint. Zeigt bei Timeout das ganze Log.
async fn wait_for_log(log_path: &str, needle: &str) {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let text = tokio::fs::read_to_string(log_path)
            .await
            .unwrap_or_default();
        if text.contains(needle) {
            return;
        }
        if Instant::now() > deadline {
            panic!("'{needle}' erschien nicht im Log. Log:\n{text}");
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

// ---------------------------------------------------------------------------

#[tokio::test]
async fn broken_reload_keeps_the_old_policy_and_the_server_keeps_answering() {
    let dir = TempDir::new("broken");
    let ads = dir.file("ads.list", "blocked.example\n");
    let port = free_port();
    let config = dir.file("alpendns.toml", &config_text(port, None, &[("ads", &ads)]));

    let log = dir.0.join("server.log").to_string_lossy().into_owned();
    let mut server = Server::start(&config, &log);

    // Bereit, und die Liste blockt bereits.
    query_until(port, "blocked.example.", ResponseCode::NXDomain).await;

    // Konfiguration kaputt machen und neu laden lassen.
    dir.file("alpendns.toml", "das ist [ kein toml");
    hangup(server.pid());

    // Der Reload wurde versucht und abgelehnt …
    wait_for_log(&server.log, "Reload abgelehnt").await;
    // … und der alte Stand gilt unverändert weiter.
    query_until(port, "blocked.example.", ResponseCode::NXDomain).await;

    server.shutdown();
}

#[tokio::test]
async fn valid_reload_blocks_a_newly_added_list() {
    let dir = TempDir::new("valid");
    let ads = dir.file("ads.list", "ads.example\n");
    let more = dir.file("more.list", "banned.example\n");
    let fake = fake_upstream().await;
    let port = free_port();
    let config = dir.file(
        "alpendns.toml",
        &config_text(port, Some(fake), &[("ads", &ads)]),
    );

    let log = dir.0.join("server.log").to_string_lossy().into_owned();
    let mut server = Server::start(&config, &log);

    // Bereit: die erste Liste blockt, die zweite Domain ist (noch) erlaubt und
    // kommt vom Fake-Upstream.
    query_until(port, "ads.example.", ResponseCode::NXDomain).await;
    let allowed = query_until(port, "banned.example.", ResponseCode::NoError).await;
    assert_eq!(
        allowed.answers.len(),
        1,
        "Fake-Upstream hätte antworten müssen"
    );

    // Zweite Liste in die Konfiguration aufnehmen und neu laden lassen.
    dir.file(
        "alpendns.toml",
        &config_text(port, Some(fake), &[("ads", &ads), ("more", &more)]),
    );
    hangup(server.pid());

    // Sobald der Reload durch ist, ist auch die zweite Liste aktiv.
    wait_for_log(&server.log, "Konfiguration neu geladen").await;
    query_until(port, "banned.example.", ResponseCode::NXDomain).await;
    // Die erste Liste gilt weiter.
    query_until(port, "ads.example.", ResponseCode::NXDomain).await;

    server.shutdown();
}
