//! Die Programmausgabe ist englisch.
//!
//! Der Wächter für die Sprachregelung in `docs/OPERATIONS.md`: wer kein Deutsch
//! kann, soll AlpenDNS installieren, prüfen und betreiben können. Geprüft wird
//! die **Ausgabe**, nicht der Quelltext — deutsche Kommentare können nicht in
//! die Ausgabe gelangen, und wer die Ausgabe prüft, hat das ganze Problem nicht.
//! Vorbild ist `no_query_name_leaves_the_process_in_the_quiet_modes` in
//! `tests/observability.rs`: dasselbe Vorgehen, andere Nadel.
//!
//! Zwei Flächen, weil zwei verschiedene Dinge schiefgehen können:
//!
//! * [`the_command_line_speaks_english`] ruft das Binary mit den Aufrufen auf,
//!   die ein Mensch in der Installation tippt — `check` übt dabei die meisten
//!   Druckstellen auf einmal.
//! * [`the_running_process_speaks_english`] lässt den Server laufen und liest
//!   ab, was er von selbst sagt: die Prometheus-HELP-Texte über `/metrics` und
//!   die Start- und Signal-Meldungen aus dem Log.
//!
//! Dazu die drei reinen String-Funktionen, die nur unter Bedingungen greifen,
//! die sich im Prozess nicht herstellen lassen (eine fehlende logrotate-Regel,
//! eine fehlende Zeitsynchronisation).

// Testcode darf panicken, siehe B.1 und clippy.toml.
#![allow(clippy::expect_used, clippy::indexing_slicing, clippy::panic)]

use std::io::{Read as _, Write as _};
use std::net::{Ipv4Addr, SocketAddr, TcpStream};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

/// Deutsche Wörter, die in englischer Ausgabe nicht vorkommen können.
///
/// **Tokenweise** verglichen, nie als Teilstring: `Blocklist` enthält `ist`,
/// `listen` ist ein englisches Verb. Genau deshalb fehlen hier auch die
/// Homographen — `die`, `man`, `war`, `will`, `in`, `an`, `so`, `also`,
/// `Name`, `Port`, `Liste`/`Listen` — sie wären garantierte Fehlalarme.
///
/// Ebenfalls nicht hier: alles, was Maschinenvokabular ist (`nxdomain`,
/// `zero_ip`, `aggregate`, `dot`, `flag`, `dga`, `tunneling`, …). Das bleibt
/// absichtlich stehen und muss sogar stehen bleiben.
const GERMAN: &[&str] = &[
    "abgelehnt",
    "angegeben",
    "angelegt",
    "anfrage",
    "anfragen",
    "antwort",
    "auch",
    "auflösung",
    "außerhalb",
    "begründung",
    "beschränkt",
    "bleibt",
    "blocklisten",
    "dass",
    "datei",
    "dateien",
    "drosselung",
    "durchgelassen",
    "eintrag",
    "einträge",
    "erreichbar",
    "erwartet",
    "erzeugt",
    "fehler",
    "fehlt",
    "für",
    "geblockt",
    "geladen",
    "gesetzt",
    "gestartet",
    "gültig",
    "hinweis",
    "ist",
    "kein",
    "keine",
    "keinen",
    "konfiguration",
    "konfiguriert",
    "konnte",
    "läuft",
    "nicht",
    "noch",
    "prüft",
    "prüfung",
    "regel",
    "schreiben",
    "ungültig",
    "unbekannt",
    "unbekannte",
    "unbekanntes",
    "unterkommando",
    "verworfen",
    "vollständig",
    "vollständigen",
    "wird",
    "werden",
    "wurde",
    "zeile",
    "zeigt",
];

/// Zerlegt die Ausgabe und gibt die deutschen Wörter darin zurück.
///
/// Getrennt wird an allem, was kein Buchstabe und keine Ziffer ist — `_`, `.`,
/// `:`, `-` und Leerraum also. Die Umlaute bleiben dadurch Teil ihres Tokens,
/// `für` wird gefunden und `überschreiben` nicht als `berschreiben`.
fn german_words(text: &str) -> Vec<String> {
    text.split(|c: char| !c.is_alphanumeric())
        .filter(|token| !token.is_empty())
        .map(str::to_lowercase)
        .filter(|token| GERMAN.contains(&token.as_str()))
        .collect()
}

fn assert_english(what: &str, text: &str) {
    let found = german_words(text);
    assert!(
        found.is_empty(),
        "{what} enthält deutsche Wörter {found:?}:\n{text}"
    );
}

/// Ruft das Binary auf und gibt `(stdout, stderr)` zurück.
///
/// `LC_ALL=C`, weil `io::Error` über `strerror` kommt: auf einem Rechner mit
/// `LANG=de_DE.UTF-8` stünde in der Fehlermeldung sonst „Datei oder Verzeichnis
/// nicht gefunden" — ein garantiert falscher Alarm, den dieser Test nicht
/// selbst verursacht hat.
fn run(args: &[&str]) -> (String, String) {
    let output = Command::new(env!("CARGO_BIN_EXE_alpendns"))
        .args(args)
        .env("LC_ALL", "C")
        .output()
        .expect("Binary aufrufen");
    (
        String::from_utf8_lossy(&output.stdout).into_owned(),
        String::from_utf8_lossy(&output.stderr).into_owned(),
    )
}

struct TempDir(PathBuf);

impl TempDir {
    fn new(tag: &str) -> Self {
        let path = std::env::temp_dir().join(format!(
            "alpendns-language-{tag}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("Uhr nach 1970")
                .as_nanos()
        ));
        std::fs::create_dir_all(&path).expect("Testverzeichnis");
        Self(path)
    }

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

/// Eine gültige Konfiguration ohne Blocklisten — `check` und der Start kommen
/// damit ohne Netz aus (docs/TESTING.md §4).
///
/// Die Namen darin sind bewusst sprachneutral: der Test prüft die Ausgabe, die
/// er selbst erzeugt, und ein `[[local_zone]]` namens `meinedomain.at` stünde
/// sonst als deutsches Wort in seinem eigenen Ergebnis.
fn config_text(dns_port: u16, metrics_port: u16) -> String {
    format!(
        "[server]\n\
         listen_udp = [\"127.0.0.1:{dns_port}\"]\n\
         listen_tcp = [\"127.0.0.1:{dns_port}\"]\n\
         query_timeout = \"3s\"\n\
         \n\
         [[upstream_pool]]\n\
         name = \"default\"\n\
         \n\
         [[upstream_pool.resolver]]\n\
         name = \"pool\"\n\
         addr = \"dot://127.0.0.1:853\"\n\
         tls_name = \"dns.example.net\"\n\
         \n\
         [api]\n\
         enabled = false\n\
         \n\
         [metrics]\n\
         enabled = true\n\
         listen = \"127.0.0.1:{metrics_port}\"\n\
         path = \"/metrics\"\n"
    )
}

// ---------------------------------------------------------------------------
// Die Aufrufe, die ein Mensch tippt
// ---------------------------------------------------------------------------

fn with_config(args: &[&str], config: &str) -> Vec<String> {
    args.iter()
        .map(|arg| {
            if *arg == "CONFIG" {
                config.to_owned()
            } else {
                (*arg).to_owned()
            }
        })
        .collect()
}

#[test]
fn the_command_line_speaks_english() {
    let dir = TempDir::new("cli");
    let dns_port = free_port();
    let metrics_port = free_port();
    let config = dir.file("alpendns.toml", &config_text(dns_port, metrics_port));

    // Was der Test ausgibt, prüft er; das Echo eines unbekannten Schalters gibt
    // dessen Argument zurück, deshalb steht hier `--nope` und nicht
    // `--gibts-nicht`.
    let cases: &[&[&str]] = &[
        &["--help"],
        &["--nope"],
        &[],
        &["-c"],
        &["-c", "/does/not/exist.toml", "check"],
        &["-c", "CONFIG", "check"],
        &["-c", "CONFIG", "policy", "test", "ads.example"],
        &[
            "-c",
            "CONFIG",
            "policy",
            "test",
            "ads.example",
            "--client",
            "nobody",
        ],
        &["-c", "CONFIG", "policy"],
        &["-c", "CONFIG", "policy", "test"],
    ];

    for case in cases {
        let args = with_config(case, &config);
        let borrowed: Vec<&str> = args.iter().map(String::as_str).collect();
        let (stdout, stderr) = run(&borrowed);
        let shown = format!("alpendns {}", borrowed.join(" "));
        assert_english(&format!("{shown} (stdout)"), &stdout);
        assert_english(&format!("{shown} (stderr)"), &stderr);
    }

    // Gegenprobe: der Wächter greift überhaupt. Ohne sie wäre ein Tippfehler in
    // `GERMAN` oder im Zerleger ein Test, der immer grün ist.
    assert!(!german_words("Fehler: die Konfiguration fehlt").is_empty());
    assert!(
        german_words("Blocklist ads.example, listen on 0.0.0.0, Name: pool").is_empty(),
        "Homographen dürfen nicht anschlagen"
    );
}

/// Die `--version`-Ausgabe ist Maschinenausgabe und war nie deutsch; geprüft
/// wird sie trotzdem, weil sie der erste Aufruf in jedem Skript ist.
#[test]
fn the_version_line_speaks_english() {
    let (stdout, stderr) = run(&["--version"]);
    assert!(stdout.starts_with("alpendns "), "{stdout}");
    assert_english("--version (stdout)", &stdout);
    assert_english("--version (stderr)", &stderr);
}

// ---------------------------------------------------------------------------
// Die Funktionen, die nur unter Bedingungen greifen
// ---------------------------------------------------------------------------

#[test]
fn the_hints_speak_english() {
    let dir = TempDir::new("hints");

    // `privacy.logging.mode = "full"` ohne logrotate-Regel.
    let log = dir.0.join("queries.jsonl");
    let hint = alpendns::logging::rotation_hint(
        alpendns::logging::Mode::Full,
        &log,
        &dir.0.join("logrotate.d"),
    )
    .expect("ohne Regel muss der Hinweis kommen");
    assert_english("rotation_hint", &hint);

    // DNSSEC an, aber keine Zeitsynchronisation.
    let hint = alpendns::clock::time_sync_hint(true, &dir.0.join("synchronized"))
        .expect("ohne Merkmal muss der Hinweis kommen");
    assert_english("time_sync_hint", &hint);
}

/// Die Zeile hinter `Detectors:` in `alpendns check`. Sie ist die einzige
/// Ausgabe, die den Zustand der fünf Heuristiken zusammenfasst, und sie steht
/// wörtlich in `docs/OPERATIONS.md`.
#[test]
fn the_detector_line_speaks_english() {
    let dir = TempDir::new("detectors");
    let config = dir.file(
        "alpendns.toml",
        &format!(
            "{}\n[detection]\ntunneling = {{ action = \"off\" }}\n\
             typosquat = {{ action = \"flag\" }}\n\
             nrd = {{ action = \"flag\", source = \"/does/not/exist.txt\" }}\n",
            config_text(free_port(), free_port())
        ),
    );
    let config = alpendns::config::Config::load(std::path::Path::new(&config)).expect("lädt");
    let line = config.detection.check_line();
    assert_english("check_line", &line);
    // Die Maschinen-Token stehen unverändert darin.
    for token in ["dga", "flag", "tunneling", "off", "rebinding"] {
        assert!(line.contains(token), "{token} fehlt in: {line}");
    }
}

// ---------------------------------------------------------------------------
// Der laufende Prozess
// ---------------------------------------------------------------------------

/// Der laufende Server samt Log-Datei. Beendet sich im Drop sauber.
struct Server {
    child: Child,
    log: PathBuf,
}

impl Server {
    fn start(config_path: &str, log_path: &PathBuf) -> Server {
        let log = std::fs::File::create(log_path).expect("Log-Datei");
        let child = Command::new(env!("CARGO_BIN_EXE_alpendns"))
            .arg("-c")
            .arg(config_path)
            .env("LC_ALL", "C")
            .stdout(Stdio::from(log.try_clone().expect("clone stdout")))
            .stderr(Stdio::from(log))
            .spawn()
            .expect("Server starten");
        Server {
            child,
            log: log_path.clone(),
        }
    }

    /// SIGTERM, dann bis zu fünf Sekunden warten, notfalls hart beenden.
    fn shutdown(&mut self) {
        match self.child.try_wait() {
            Ok(Some(_)) => {}
            Ok(None) => {
                let _ = Command::new("kill")
                    .arg("-TERM")
                    .arg(self.child.id().to_string())
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

    fn text(&self) -> String {
        std::fs::read_to_string(&self.log).unwrap_or_default()
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        self.shutdown();
    }
}

/// Ein HTTP/1.1-GET von Hand: eine Zeile Anfrage, dann lesen bis das Gegenüber
/// zumacht. `reqwest` wäre eine Abhängigkeit mehr für drei Zeilen.
///
/// `None`, solange der Port noch nicht lauscht — beim Hochfahren ist eine
/// abgelehnte Verbindung der Normalfall, kein Fehler.
fn http_get(addr: SocketAddr, path: &str) -> Option<String> {
    let mut stream = TcpStream::connect(addr).ok()?;
    stream
        .set_read_timeout(Some(Duration::from_secs(10)))
        .expect("Zeitlimit");
    let request = format!("GET {path} HTTP/1.1\r\nHost: {addr}\r\nConnection: close\r\n\r\n");
    stream.write_all(request.as_bytes()).ok()?;
    let mut response = String::new();
    let _ = stream.read_to_string(&mut response);
    Some(response)
}

/// Wartet, bis der Metrik-Port antwortet, und gibt den Rumpf zurück.
///
/// Scheitert der Start, steht der Grund im Log — deshalb hängt er an der
/// Abbruchmeldung.
fn metrics_until(server: &Server, addr: SocketAddr) -> String {
    for _ in 0..100 {
        if let Some(response) = http_get(addr, "/metrics")
            && response.contains("alpendns_")
        {
            return response;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    panic!("{addr} hat nicht geantwortet; Log:\n{}", server.text());
}

#[test]
fn the_running_process_speaks_english() {
    let dir = TempDir::new("process");
    let dns_port = free_port();
    let metrics_port = free_port();
    let config = dir.file("alpendns.toml", &config_text(dns_port, metrics_port));
    let log_path = dir.0.join("server.log");
    let mut server = Server::start(&config, &log_path);

    // Alle HELP-Texte auf einmal. Sie sind der Teil der Ausgabe, der sonst
    // nirgends geprüft wird — die Tests in `metrics.rs` sehen nur die selbst
    // gebaute Momentaufnahme, nicht die Überschriften.
    let metrics = metrics_until(
        &server,
        SocketAddr::from((Ipv4Addr::LOCALHOST, metrics_port)),
    );
    assert_english("/metrics", &metrics);
    assert!(
        metrics.contains("# HELP alpendns_queries_total"),
        "die HELP-Texte fehlen:\n{metrics}"
    );

    // Start-, Signal- und Shutdown-Meldungen stehen im Log, nicht auf einem
    // Kanal, den der Test sonst lesen könnte.
    let log = server.text();
    assert!(
        log.contains("AlpenDNS started"),
        "die Startmeldung fehlt:\n{log}"
    );
    assert_english("Startmeldungen", &log);

    server.shutdown();
    // Nach dem Signal: erst die eine Zeile, dann der geordnete Abgang.
    let log = server.text();
    assert!(
        log.contains("signal received"),
        "die Signal-Meldung fehlt:\n{log}"
    );
    assert_english("Signal- und Shutdown-Meldungen", &log);
}
