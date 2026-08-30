//! Was mit einer Anfrage passiert, bevor sie den Rechner verlässt.
//!
//! Alles hier sind reine Funktionen auf einer Nachricht; wer sie anwendet, ist
//! der jeweilige Transport. Die Zuordnung ist nicht willkürlich:
//!
//! | Mechanismus | wo angewendet | warum |
//! |---|---|---|
//! | ECS entfernen | überall | das Subnetz des Clients geht keinen Upstream etwas an |
//! | Padding | nur verschlüsselt | im Klartext verrät die Länge ohnehin nichts, was der Name nicht schon verrät |
//! | 0x20 | nur Klartext | schützt gegen Off-Path-Spoofing, das es auf einer TLS-Verbindung nicht gibt |
//! | Cookies | nur Klartext | dito (RFC 7873) |
//! | DNSSEC | nur verschlüsselt | der Klartextweg geht nur in interne Zonen, und die sind nicht signiert |

use std::sync::atomic::{AtomicU64, Ordering};

use hickory_proto::op::{Edns, Message};
use hickory_proto::rr::Name;
use hickory_proto::rr::rdata::opt::{EdnsCode, EdnsOption};

/// Welche Mechanismen aktiv sind. Kommt aus `[privacy]` in der Konfiguration.
#[derive(Debug, Clone, Copy)]
pub struct Settings {
    pub strip_ecs: bool,
    pub padding: bool,
    pub cookies: bool,
    pub dns0x20: bool,
    /// Signaturkette selbst nachrechnen. Wirkt nur auf verschlüsselten
    /// Transporten: der Klartextweg führt ausschließlich in interne Zonen, und
    /// die sind nicht signiert.
    pub dnssec: bool,
}

impl From<&crate::config::PrivacyConfig> for Settings {
    fn from(config: &crate::config::PrivacyConfig) -> Self {
        Self {
            strip_ecs: config.strip_ecs,
            padding: config.padding,
            cookies: config.cookies,
            dns0x20: config.dns0x20,
            dnssec: config.dnssec,
        }
    }
}

impl Default for Settings {
    fn default() -> Self {
        Self::from(&crate::config::PrivacyConfig::default())
    }
}

/// Blockgröße für EDNS-Padding. RFC 8467 §4.1 empfiehlt Anfragen auf ein
/// Vielfaches von 128 Byte aufzufüllen.
pub const PADDING_BLOCK: usize = 128;

/// Länge eines Client-Cookies nach RFC 7873 §4.
pub const CLIENT_COOKIE_LEN: usize = 8;

/// Wie oft ein Mechanismus tatsächlich gegriffen hat.
///
/// Die Zähler stehen prozessweit und nicht in [`Settings`]: `Settings` ist
/// `Copy` und liegt als Kopie in jedem Transport, ein Zähler darin würde also
/// je Transport getrennt zählen. Ein `Arc` durch alle Kopien zu fädeln wäre
/// Aufwand für eine Handvoll Additionen. Atomare Zähler sind kein Mutex im
/// Anfragepfad (B.3 Regel 5) und blockieren niemanden.
///
/// Gezählt wird **die Wirkung, nicht die Einstellung**: `strip_ecs` zählt nur,
/// wenn wirklich eine ECS-Option entfernt wurde. Sonst zeigte die Zahl bloß,
/// dass ein Schalter an ist — das steht schon in der Konfiguration.
#[derive(Debug, Default)]
pub struct Counters {
    /// Anfragen, aus denen eine ECS-Option entfernt wurde.
    pub ecs_stripped: AtomicU64,
    /// Anfragen, die auf Blockgröße aufgefüllt wurden.
    pub padded: AtomicU64,
    /// Anfragen mit gewürfelter Groß-/Kleinschreibung (0x20).
    pub randomized: AtomicU64,
    /// Anfragen mit gesetztem DNS-Cookie.
    pub cookies: AtomicU64,
}

/// Momentaufnahme der Zähler, für Metriken und API.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct CounterSnapshot {
    pub ecs_stripped: u64,
    pub padded: u64,
    pub randomized: u64,
    pub cookies: u64,
}

static COUNTERS: Counters = Counters {
    ecs_stripped: AtomicU64::new(0),
    padded: AtomicU64::new(0),
    randomized: AtomicU64::new(0),
    cookies: AtomicU64::new(0),
};

/// Der aktuelle Stand aller Privacy-Zähler.
pub fn counters() -> CounterSnapshot {
    CounterSnapshot {
        ecs_stripped: COUNTERS.ecs_stripped.load(Ordering::Relaxed),
        padded: COUNTERS.padded.load(Ordering::Relaxed),
        randomized: COUNTERS.randomized.load(Ordering::Relaxed),
        cookies: COUNTERS.cookies.load(Ordering::Relaxed),
    }
}

/// Entfernt EDNS Client Subnet aus einer Anfrage.
///
/// ECS verrät dem Upstream, aus welchem Subnetz der Client kommt, damit er
/// geografisch passende Antworten geben kann. Für einen Heimanschluss ist der
/// Nutzen gering und der Preis hoch: das Subnetz identifiziert den Haushalt.
pub fn strip_ecs(message: &mut Message) {
    if let Some(edns) = message.edns.as_mut()
        && edns.option(EdnsCode::Subnet).is_some()
    {
        edns.options_mut().remove(EdnsCode::Subnet);
        COUNTERS.ecs_stripped.fetch_add(1, Ordering::Relaxed);
    }
}

/// Füllt eine Anfrage auf ein Vielfaches von `block` Byte auf.
///
/// Auf einer verschlüsselten Verbindung ist die Nachrichtenlänge das, was vom
/// Namen übrig bleibt: `a.de` und `sehr-langer-name.example.org` sehen
/// unterschiedlich aus, auch wenn niemand mitlesen kann.
pub fn pad_to_block(message: &mut Message, block: usize) -> Result<(), hickory_proto::ProtoError> {
    if block == 0 {
        return Ok(());
    }
    if message.edns.is_none() {
        message.set_edns(Edns::new());
    }
    // Vorhandenes Padding zuerst weg, sonst wächst die Nachricht bei jedem Aufruf.
    if let Some(edns) = message.edns.as_mut() {
        edns.options_mut().remove(EdnsCode::Padding);
    }

    let current = message.to_vec()?.len();
    // Die Option selbst kostet vier Byte Kopf (Code und Länge).
    let with_header = current.saturating_add(4);
    let target = with_header.div_ceil(block).saturating_mul(block);
    let fill = target.saturating_sub(with_header);

    if let Some(edns) = message.edns.as_mut() {
        edns.options_mut().insert(EdnsOption::Unknown(
            u16::from(EdnsCode::Padding),
            vec![0; fill],
        ));
        COUNTERS.padded.fetch_add(1, Ordering::Relaxed);
    }
    Ok(())
}

/// Würfelt die Groß-/Kleinschreibung eines Namens neu (0x20-Encoding).
///
/// Ein Angreifer, der eine Antwort unterschieben will, muss neben Query-ID und
/// Quellport auch die Schreibweise treffen. Bei einem Namen mit 15 Buchstaben
/// sind das 15 zusätzliche Bits.
pub fn randomize_case(name: &Name) -> Name {
    let labels: Vec<Vec<u8>> = name
        .iter()
        .map(|label| {
            label
                .iter()
                .map(|&byte| {
                    if byte.is_ascii_alphabetic() && rand::random::<bool>() {
                        byte ^ 0x20
                    } else {
                        byte
                    }
                })
                .collect()
        })
        .collect();

    // Schlägt das Zusammensetzen fehl, bleibt der Name wie er war — 0x20 ist
    // eine Härtung, kein Grund, eine Anfrage scheitern zu lassen.
    Name::from_labels(labels).map_or_else(
        |_| name.clone(),
        |mut built| {
            built.set_fqdn(name.is_fqdn());
            COUNTERS.randomized.fetch_add(1, Ordering::Relaxed);
            built
        },
    )
}

/// Ersetzt den Namen der ersten Frage. Gibt den vorherigen zurück.
pub fn replace_question_name(message: &mut Message, name: Name) -> Option<Name> {
    let query = message.queries.first_mut()?;
    let previous = query.name().clone();
    query.set_name(name);
    Some(previous)
}

/// Ob die Antwort die Schreibweise der Frage exakt gespiegelt hat.
///
/// Anders als der normale Vergleich ist dieser case-*sensitiv* — genau darin
/// besteht der Schutz.
pub fn echoed_same_case(sent: &Name, echoed: &Name) -> bool {
    sent.eq_case(echoed)
}

/// Ein neues zufälliges Client-Cookie.
pub fn new_client_cookie() -> [u8; CLIENT_COOKIE_LEN] {
    rand::random()
}

/// Setzt das DNS-Cookie (RFC 7873) in eine Anfrage.
///
/// Ohne Server-Cookie sind es acht Byte; kennt man eines vom letzten Mal, wird
/// es angehängt. Der Server erkennt uns daran wieder und antwortet, ohne auf
/// TCP zu verweisen.
pub fn set_cookie(message: &mut Message, client: &[u8; CLIENT_COOKIE_LEN], server: Option<&[u8]>) {
    if message.edns.is_none() {
        message.set_edns(Edns::new());
    }
    let mut value = client.to_vec();
    if let Some(server) = server {
        value.extend_from_slice(server);
    }
    if let Some(edns) = message.edns.as_mut() {
        edns.options_mut()
            .insert(EdnsOption::Unknown(u16::from(EdnsCode::Cookie), value));
        COUNTERS.cookies.fetch_add(1, Ordering::Relaxed);
    }
}

/// Liest das Server-Cookie aus einer Antwort, falls eines dabei war.
///
/// Die ersten acht Byte sind das Cookie, das wir geschickt haben; alles danach
/// gehört dem Server.
pub fn server_cookie(message: &Message) -> Option<Vec<u8>> {
    let edns = message.edns.as_ref()?;
    let EdnsOption::Unknown(_, value) = edns.option(EdnsCode::Cookie)? else {
        return None;
    };
    let server = value.get(CLIENT_COOKIE_LEN..)?;
    (!server.is_empty()).then(|| server.to_vec())
}

#[cfg(test)]
mod tests {
    use super::*;
    use hickory_proto::op::{MessageType, OpCode, Query};
    use hickory_proto::rr::RecordType;
    use hickory_proto::rr::rdata::opt::ClientSubnet;
    use std::net::{IpAddr, Ipv4Addr};

    fn name(text: &str) -> Name {
        Name::from_ascii(text).expect("gültiger Name")
    }

    fn query_for(text: &str) -> Message {
        let mut message = Message::new(1, MessageType::Query, OpCode::Query);
        message.add_query(Query::query(name(text), RecordType::A));
        message
    }

    #[test]
    fn ecs_is_removed_from_the_request() {
        let mut message = query_for("example.com.");
        let mut edns = Edns::new();
        edns.options_mut()
            .insert(EdnsOption::Subnet(ClientSubnet::new(
                IpAddr::V4(Ipv4Addr::new(192, 0, 2, 0)),
                24,
                0,
            )));
        message.set_edns(edns);
        assert!(
            message
                .edns
                .as_ref()
                .and_then(|e| e.option(EdnsCode::Subnet))
                .is_some(),
            "Aufbau des Tests stimmt nicht"
        );

        strip_ecs(&mut message);
        assert!(
            message
                .edns
                .as_ref()
                .and_then(|e| e.option(EdnsCode::Subnet))
                .is_none(),
            "ECS ging trotzdem raus"
        );
    }

    #[test]
    fn stripping_ecs_without_edns_does_nothing() {
        let mut message = query_for("example.com.");
        strip_ecs(&mut message);
        assert!(message.edns.is_none());
    }

    #[test]
    fn padding_rounds_the_message_up_to_a_full_block() {
        for host in [
            "a.de.",
            "example.com.",
            &format!("{}.example.org.", "x".repeat(50)),
        ] {
            let mut message = query_for(host);
            pad_to_block(&mut message, PADDING_BLOCK).expect("kodierbar");
            let len = message.to_vec().expect("kodierbar").len();
            assert_eq!(
                len % PADDING_BLOCK,
                0,
                "{host} ergibt {len} Byte, kein Vielfaches von {PADDING_BLOCK}"
            );
        }
    }

    #[test]
    fn padding_makes_different_names_the_same_length() {
        // Das ist der eigentliche Zweck: die Länge darf den Namen nicht verraten.
        let mut short = query_for("a.de.");
        let mut long = query_for("sehr-viel-laengerer-name.example.org.");
        pad_to_block(&mut short, PADDING_BLOCK).expect("kodierbar");
        pad_to_block(&mut long, PADDING_BLOCK).expect("kodierbar");
        assert_eq!(
            short.to_vec().expect("kodierbar").len(),
            long.to_vec().expect("kodierbar").len()
        );
    }

    #[test]
    fn padding_twice_does_not_grow_the_message() {
        let mut message = query_for("example.com.");
        pad_to_block(&mut message, PADDING_BLOCK).expect("kodierbar");
        let once = message.to_vec().expect("kodierbar").len();
        pad_to_block(&mut message, PADDING_BLOCK).expect("kodierbar");
        assert_eq!(message.to_vec().expect("kodierbar").len(), once);
    }

    #[test]
    fn case_randomization_is_a_round_trip() {
        // Die Invariante aus docs/TESTING.md §2.
        let original = name("www.Example.com.");
        for _ in 0..200 {
            let randomized = randomize_case(&original);
            assert_eq!(randomized, original, "der Name selbst hat sich geändert");
            assert_eq!(
                randomized.to_ascii().to_lowercase(),
                original.to_ascii().to_lowercase()
            );
        }
    }

    #[test]
    fn case_randomization_actually_changes_the_case_sometimes() {
        let original = name("averyverylongnamewithmanyletters.example.com.");
        let changed = (0..50).any(|_| !randomize_case(&original).eq_case(&original));
        assert!(changed, "die Schreibweise wurde nie verändert");
    }

    #[test]
    fn case_comparison_is_case_sensitive() {
        // Der normale Vergleich ist es nicht — genau deshalb braucht 0x20 einen
        // eigenen.
        let sent = name("ExAmPlE.com.");
        let echoed_wrong = name("example.com.");
        assert_eq!(
            sent, echoed_wrong,
            "der normale Vergleich ist unempfindlich"
        );
        assert!(!echoed_same_case(&sent, &echoed_wrong));
        assert!(echoed_same_case(&sent, &name("ExAmPlE.com.")));
    }

    #[test]
    fn replacing_the_question_name_returns_the_previous_one() {
        let mut message = query_for("example.com.");
        let previous = replace_question_name(&mut message, name("EXAMPLE.com."));
        assert_eq!(previous, Some(name("example.com.")));
        assert!(
            message
                .queries
                .first()
                .expect("eine Frage")
                .name()
                .eq_case(&name("EXAMPLE.com."))
        );
    }

    #[test]
    fn cookie_without_server_part_is_eight_bytes() {
        let mut message = query_for("example.com.");
        let client = [1, 2, 3, 4, 5, 6, 7, 8];
        set_cookie(&mut message, &client, None);

        let option = message
            .edns
            .as_ref()
            .and_then(|e| e.option(EdnsCode::Cookie))
            .expect("Cookie gesetzt");
        let EdnsOption::Unknown(_, value) = option else {
            panic!("unerwarteter Optionstyp");
        };
        assert_eq!(value.as_slice(), &client);
        assert_eq!(
            server_cookie(&message),
            None,
            "es gibt noch kein Server-Cookie"
        );
    }

    #[test]
    fn server_cookie_is_read_back_and_sent_again() {
        let mut response = query_for("example.com.");
        let client = new_client_cookie();
        set_cookie(&mut response, &client, Some(&[0xaa; 16]));

        let server = server_cookie(&response).expect("Server-Cookie");
        assert_eq!(server, vec![0xaa; 16]);

        let mut next = query_for("example.com.");
        set_cookie(&mut next, &client, Some(&server));
        assert_eq!(server_cookie(&next), Some(server));
    }

    #[test]
    fn client_cookies_differ_between_calls() {
        let first = new_client_cookie();
        let repeated = (0..20).any(|_| new_client_cookie() != first);
        assert!(repeated, "das Cookie ist nicht zufällig");
    }
}
