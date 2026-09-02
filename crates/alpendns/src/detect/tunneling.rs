//! DNS-Tunneling: Daten, die als Namen aus dem Netz getragen werden.
//!
//! `iodine`, `dnscat2` und Verwandte kodieren Nutzdaten in Subdomains und
//! lassen sich die Antwort in TXT- oder NULL-Records zurückgeben. Für den
//! Resolver sieht das aus wie ganz normale Auflösung — deshalb kommt DNS auch
//! durch Firewalls, die alles andere dichtmachen.
//!
//! # Der entscheidende Punkt: bewertet wird die Zone, nicht die Anfrage
//!
//! Eine einzelne lange Subdomain mit hoher Entropie ist völlig normal. So
//! sehen die Hostnamen jedes CDN aus, jeder Cloud-Storage-Bucket und jeder
//! Mailversender mit Tracking. **Tausend** davon unter derselben Zone,
//! innerhalb weniger Minuten, jede genau einmal — das ist nicht normal
//! (FEATURES.md D2).
//!
//! Deshalb hält dieser Detektor als einziger Zustand: je Zone ein Zeitfenster
//! mit ein paar Zählern. Ohne diesen Zustand wäre die Erkennung entweder blind
//! oder ein Fehlalarm-Automat.
//!
//! # Fünf Signale, gewichtet
//!
//! | Signal | wogegen es hilft | Gewicht |
//! |---|---|---:|
//! | einmalige Subdomains je Zone | der Kern: Tunnel brauchen je Paket einen neuen Namen | 0,50 |
//! | Entropie im ersten Label | kodierte Nutzdaten sehen zufällig aus | 0,25 |
//! | Länge des ersten Labels | pro Anfrage soll möglichst viel durchgehen | 0,15 |
//! | Anteil TXT/NULL | der Rückkanal | 0,10 |
//!
//! Die Rate je Zone steckt implizit in der Zahl der einmaligen Subdomains je
//! Fenster: ein Tunnel mit zehn Anfragen pro Stunde ist kein Tunnel, sondern
//! ein langsames Leck, und der fällt hier bewusst nicht auf.
//!
//! # Grenzen
//!
//! Manche CDNs und Antivirus-Produkte sehen genauso aus — Reputationsdienste
//! fragen Hashes als Subdomain ab, und das ist per Konstruktion ein Tunnel mit
//! guten Absichten. Deshalb `flag` als Default und eine Ausnahmeliste
//! (`allow_zones`).

use std::collections::{HashMap, HashSet};
use std::hash::{Hash as _, Hasher as _};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use hickory_proto::rr::RecordType;

use super::{Detector, Finding, NameDetector, Observation, Permille};
use crate::clock::Clock;

/// So viele einmalige Subdomains müssen im Fenster stehen, bevor überhaupt
/// bewertet wird.
///
/// Darunter ist jede Aussage Rauschen: ein Haushalt fragt unter einer Zone
/// selten mehr als eine Handvoll verschiedener Hosts, und ein Tunnel erzeugt
/// hunderte. Der Schwellwert liegt näher am unteren Ende, damit ein langsamer
/// Tunnel nicht durchrutscht.
const MIN_UNIQUE: u32 = 12;

/// Bei so vielen einmaligen Subdomains ist das Signal voll ausgeschlagen.
const FULL_UNIQUE: f32 = 60.0;

/// Entropie in Bit je Zeichen, zwischen denen das Signal ansteigt.
///
/// Normale Hostnamen liegen bei 2,0 bis 3,0 Bit — sie bestehen aus Wörtern.
/// Die Obergrenze ist bewusst 4,2 und nicht das theoretische Maximum: `dnscat2`
/// kodiert hexadezimal und kommt damit über 4,0 Bit gar nicht hinaus. Eine
/// Decke bei 5,0 hieße, dass ausgerechnet der verbreitetste Tunnel das Signal
/// nie voll auslösen kann.
const ENTROPY_FLOOR: f32 = 3.2;
const ENTROPY_CEILING: f32 = 4.2;

/// Labellängen, zwischen denen das Längensignal ansteigt.
///
/// Ebenfalls nach unten gezogen: 15 Zeichen sind für einen Hostnamen schon
/// lang, und ein Tunnel mit 36 Zeichen soll nicht bei der Hälfte des Signals
/// hängenbleiben.
const LENGTH_FLOOR: f32 = 15.0;
const LENGTH_CEILING: f32 = 45.0;

/// Ab diesem Score gilt eine Zone als Tunnel, wenn die Konfiguration nichts
/// anderes sagt.
///
/// Die Zahl gehört hierher und nicht in die Konfiguration: sie ergibt nur
/// zusammen mit den Gewichten oben einen Sinn. Gemessen (BENCHMARKS.md):
/// gewöhnlicher Verkehr unter einer Zone bleibt bei 0,17, ein `dnscat2`-artiger
/// Strom liegt bei 0,81, ein `iodine`-artiger bei 1,0.
///
/// Zwischen 0,2 und 0,8 liegt leeres Feld, und die Schwelle steht am **unteren**
/// Rand der Treffer statt in der Mitte der Lücke: `dnscat2` kodiert
/// hexadezimal und kommt damit knapp über 0,8 — bei 0,8 als Schwelle entschiede
/// die zweite Nachkommastelle darüber, ob der verbreitetste Tunnel auffällt.
pub const DEFAULT_THRESHOLD: f32 = 0.75;

/// So viele Zonen werden höchstens gleichzeitig beobachtet.
///
/// Die Tabelle liegt im Anfragepfad; sie darf nicht mit dem Verkehr wachsen.
/// Ist sie voll, wird beim nächsten Aufräumen die Zone mit dem ältesten
/// Fenster verdrängt. Ein Angreifer, der die Tabelle mit Zonen flutet, drängt
/// damit seinen eigenen Tunnel heraus — das ist zwar denkbar, kostet ihn aber
/// mehr als es ihm bringt.
const MAX_ZONES: usize = 4096;

/// So viele einmalige Subdomains werden je Zone gemerkt.
///
/// Darüber ist der Score ohnehin voll ausgeschlagen; weiterzuzählen würde nur
/// Speicher kosten. Gespeichert werden **Hashes**, nicht die Namen selbst.
const MAX_UNIQUE_TRACKED: usize = 256;

/// Was in einem Zeitfenster unter einer Zone passiert ist.
#[derive(Debug)]
struct Window {
    opened: Instant,
    queries: u32,
    /// Gesalzene Hashes der Subdomain-Teile. Nicht die Namen: dieser Zustand
    /// liegt über Minuten im Speicher, und was hier nicht steht, kann auch
    /// nicht versehentlich irgendwo ausgegeben werden (B.1 Regel 3).
    unique: HashSet<u64>,
    /// Wie viele davon nicht mehr in `unique` passten.
    unique_overflow: u32,
    txt_or_null: u32,
    entropy_sum: f32,
    length_sum: u32,
}

impl Window {
    fn new(now: Instant) -> Self {
        Self {
            opened: now,
            queries: 0,
            unique: HashSet::new(),
            unique_overflow: 0,
            txt_or_null: 0,
            entropy_sum: 0.0,
            length_sum: 0,
        }
    }

    fn unique_count(&self) -> u32 {
        u32::try_from(self.unique.len())
            .unwrap_or(u32::MAX)
            .saturating_add(self.unique_overflow)
    }
}

/// Beobachtet Zonen über ein Zeitfenster.
pub struct Tunneling<C> {
    zones: Mutex<HashMap<String, Window>>,
    window: Duration,
    /// Zonen, für die nicht bewertet wird.
    allowed: Vec<String>,
    /// Salz für die Subdomain-Hashes, beim Start gezogen.
    salt: u64,
    clock: C,
}

// Von Hand: die Tabelle enthält Zonennamen, und die haben in einer
// Debug-Ausgabe nichts verloren.
impl<C> std::fmt::Debug for Tunneling<C> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Tunneling")
            .field("window", &self.window)
            .field(
                "zones",
                &self
                    .zones
                    .lock()
                    .map(|zones| zones.len())
                    .unwrap_or_default(),
            )
            .finish_non_exhaustive()
    }
}

impl<C: Clock> Tunneling<C> {
    pub fn new(window: Duration, allow_zones: &[String], clock: C) -> Self {
        Self {
            zones: Mutex::new(HashMap::new()),
            window,
            allowed: allow_zones
                .iter()
                .map(|zone| zone.trim().trim_end_matches('.').to_lowercase())
                .collect(),
            salt: rand::random(),
            clock,
        }
    }

    /// Wie viele Zonen gerade beobachtet werden. Für Metrik und Tests.
    pub fn tracked_zones(&self) -> usize {
        self.zones
            .lock()
            .map(|zones| zones.len())
            .unwrap_or_default()
    }

    fn is_allowed(&self, zone: &str) -> bool {
        self.allowed
            .iter()
            .any(|allowed| zone == allowed || zone.ends_with(&format!(".{allowed}")))
    }

    fn hash_subdomain(&self, subdomain: &str) -> u64 {
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        self.salt.hash(&mut hasher);
        subdomain.hash(&mut hasher);
        hasher.finish()
    }
}

/// Shannon-Entropie in Bit je Zeichen.
///
/// Über das erste Label, nicht über den ganzen Namen: die Zone ist bei jeder
/// Anfrage dieselbe und würde die Zahl nach unten ziehen, je länger sie ist.
pub fn entropy(label: &str) -> f32 {
    let mut counts = [0_u32; 256];
    let mut total = 0_u32;
    for byte in label.bytes() {
        if let Some(slot) = counts.get_mut(usize::from(byte)) {
            *slot = slot.saturating_add(1);
            total = total.saturating_add(1);
        }
    }
    if total == 0 {
        return 0.0;
    }
    #[expect(
        clippy::cast_precision_loss,
        reason = "Labellängen liegen unter 64, weit unter der Genauigkeitsgrenze von f32"
    )]
    let total_f = total as f32;
    counts
        .iter()
        .filter(|&&count| count > 0)
        .map(|&count| {
            #[expect(clippy::cast_precision_loss, reason = "siehe oben")]
            let p = count as f32 / total_f;
            -p * p.log2()
        })
        .sum()
}

/// Teilt einen Namen in Zone und den Teil darunter.
///
/// Die Zone sind die letzten beiden Labels — hier genügt die Näherung, anders
/// als bei `split_by_zone` (ADR-0018). Eine zu grob geschnittene Zone wirft
/// mehrere echte Zonen in einen Topf und macht den Detektor damit *empfindlicher*,
/// nicht blinder; die Folge wäre ein Fehlalarm, und den fängt die
/// Mindestanzahl einmaliger Subdomains ab.
fn split_zone(name: &str) -> Option<(&str, &str)> {
    let mut boundaries = name.rmatch_indices('.');
    let _tld = boundaries.next()?;
    let (index, _) = boundaries.next()?;
    let zone = name.get(index.saturating_add(1)..)?;
    let subdomain = name.get(..index)?;
    (!subdomain.is_empty()).then_some((zone, subdomain))
}

fn clamp01(value: f32) -> f32 {
    value.clamp(0.0, 1.0)
}

/// Rechnet die Signale eines Fensters in einen Score um.
fn score_window(window: &Window) -> (Permille, f32, f32, f32) {
    let unique = window.unique_count();
    #[expect(
        clippy::cast_precision_loss,
        reason = "Zählwerte eines Fensters liegen weit unter der Genauigkeitsgrenze"
    )]
    let queries = window.queries.max(1) as f32;

    #[expect(clippy::cast_precision_loss, reason = "siehe oben")]
    let volume = clamp01(unique as f32 / FULL_UNIQUE);
    let mean_entropy = window.entropy_sum / queries;
    #[expect(clippy::cast_precision_loss, reason = "siehe oben")]
    let mean_length = window.length_sum as f32 / queries;

    let entropy_signal =
        clamp01((mean_entropy - ENTROPY_FLOOR) / (ENTROPY_CEILING - ENTROPY_FLOOR));
    let length_signal = clamp01((mean_length - LENGTH_FLOOR) / (LENGTH_CEILING - LENGTH_FLOOR));
    #[expect(clippy::cast_precision_loss, reason = "siehe oben")]
    let txt_signal = clamp01(window.txt_or_null as f32 / queries);

    // Die einmaligen Subdomains wiegen am schwersten, weil sie das
    // spezifischste Signal sind: die Entropie hängt stark am Kodierverfahren
    // (hex kommt über 4 Bit nicht hinaus, base64 über 6), die Zahl der
    // einmaligen Namen dagegen an der Sache selbst — ein Tunnel braucht je
    // Paket einen neuen Namen, egal wie er ihn schreibt.
    let score = 0.50 * volume + 0.25 * entropy_signal + 0.15 * length_signal + 0.10 * txt_signal;
    (super::permille(score), mean_entropy, mean_length, volume)
}

impl<C: Clock> NameDetector for Tunneling<C> {
    fn detector(&self) -> Detector {
        Detector::Tunneling
    }

    fn inspect(&self, observation: &Observation<'_>) -> Option<Finding> {
        let (zone, subdomain) = split_zone(observation.name)?;
        if self.is_allowed(zone) {
            return None;
        }
        // Das erste Label trägt bei jedem bekannten Tunnel die Nutzdaten.
        let first = subdomain.rsplit('.').next_back().unwrap_or(subdomain);
        let now = self.clock.now();

        let mut zones = self
            .zones
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);

        // Aufräumen, bevor etwas dazukommt: abgelaufene Fenster fliegen raus,
        // und erst wenn danach immer noch kein Platz ist, wird verdrängt.
        zones.retain(|_, window| now.saturating_duration_since(window.opened) < self.window);
        if zones.len() >= MAX_ZONES && !zones.contains_key(zone) {
            let oldest = zones
                .iter()
                .min_by_key(|(_, window)| window.opened)
                .map(|(name, _)| name.clone());
            if let Some(oldest) = oldest {
                zones.remove(&oldest);
            }
        }

        let window = zones
            .entry(zone.to_owned())
            .or_insert_with(|| Window::new(now));

        window.queries = window.queries.saturating_add(1);
        window.entropy_sum += entropy(first);
        window.length_sum = window
            .length_sum
            .saturating_add(u32::try_from(first.len()).unwrap_or(u32::MAX));
        if matches!(observation.query_type, RecordType::TXT | RecordType::NULL) {
            window.txt_or_null = window.txt_or_null.saturating_add(1);
        }
        if window.unique.len() < MAX_UNIQUE_TRACKED {
            window.unique.insert(self.hash_subdomain(subdomain));
        } else {
            window.unique_overflow = window.unique_overflow.saturating_add(1);
        }

        let unique = window.unique_count();
        if unique < MIN_UNIQUE {
            return None;
        }
        let (score, mean_entropy, mean_length, _) = score_window(window);
        if score == 0 {
            return None;
        }
        let queries = window.queries;
        let txt = window.txt_or_null;
        // Der Lock endet hier; das Formatieren darunter braucht ihn nicht.
        drop(zones);

        Some(Finding::new(
            Detector::Tunneling,
            score,
            format!(
                "Zone '{zone}': {unique} unique subdomains in {queries} queries, \
                 label entropy {mean_entropy:.1} bits/character, mean label length \
                 {mean_length:.0}, {txt} of them TXT/NULL"
            ),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::clock::TestClock;
    use std::sync::Arc;

    const WINDOW: Duration = Duration::from_secs(300);

    fn detector(clock: Arc<TestClock>) -> Tunneling<Arc<TestClock>> {
        Tunneling::new(WINDOW, &[], clock)
    }

    fn ask(detector: &Tunneling<Arc<TestClock>>, name: &str, kind: RecordType) -> Option<Finding> {
        detector.inspect(&Observation {
            name,
            query_type: kind,
        })
    }

    /// Ein iodine-artiger Strom: base32-kodierte Nutzdaten in langen Labels.
    ///
    /// Nachgebaut nach dem dokumentierten Kodierschema, nicht aus einem
    /// Mitschnitt — was zählt, sind die Merkmale: ein neues Label je Paket,
    /// nahe an der Längengrenze, aus einem Alphabet ohne Wortstruktur.
    fn iodine_like(index: usize) -> String {
        const ALPHABET: &[u8] = b"abcdefghijklmnopqrstuvwxyz0123456789";
        let mut state = (index as u64).wrapping_mul(0x9e37_79b9_7f4a_7c15);
        let mut label = String::with_capacity(60);
        for _ in 0..58 {
            state = state
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            let pick = usize::try_from(state >> 33).unwrap_or(0) % ALPHABET.len();
            label.push(char::from(ALPHABET.get(pick).copied().unwrap_or(b'a')));
        }
        format!("{label}.t.example.com")
    }

    /// Ein dnscat2-artiger Strom: hex-kodiert, mehrere kürzere Labels.
    fn dnscat_like(index: usize) -> String {
        let mut state = (index as u64)
            .wrapping_add(1)
            .wrapping_mul(0x2545_f491_4f6c_dd1d);
        let mut label = String::with_capacity(40);
        for _ in 0..36 {
            state = state
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1);
            let nibble = u32::try_from((state >> 40) & 0xf).unwrap_or(0);
            label.push(char::from_digit(nibble, 16).unwrap_or('0'));
        }
        format!("{label}.tunnel.example.net")
    }

    #[test]
    fn an_iodine_like_stream_is_detected() {
        // Abnahmekriterium Schritt 3, erste Hälfte.
        let clock = Arc::new(TestClock::new());
        let detector = detector(Arc::clone(&clock));
        let mut best = 0;
        for i in 0..80 {
            if let Some(finding) = ask(&detector, &iodine_like(i), RecordType::TXT) {
                best = best.max(finding.score);
            }
        }
        assert!(best >= 950, "Score nur {best}");
    }

    #[test]
    fn a_dnscat_like_stream_is_detected() {
        let clock = Arc::new(TestClock::new());
        let detector = detector(Arc::clone(&clock));
        let mut best = 0;
        for i in 0..80 {
            if let Some(finding) = ask(&detector, &dnscat_like(i), RecordType::TXT) {
                best = best.max(finding.score);
            }
        }
        assert!(best >= 780, "Score nur {best}");
    }

    #[test]
    fn the_reason_names_the_features_that_led_to_it() {
        // FEATURES.md D6: ohne die Merkmale ist ein Falsch-Positiv nicht
        // debugbar, und dann wird der Detektor abgeschaltet statt verbessert.
        let clock = Arc::new(TestClock::new());
        let detector = detector(Arc::clone(&clock));
        let mut last = None;
        for i in 0..80 {
            last = ask(&detector, &iodine_like(i), RecordType::TXT).or(last);
        }
        let reason = last.expect("ein Fund").reason;
        for expected in ["unique subdomains", "entropy", "label length", "TXT/NULL"] {
            assert!(reason.contains(expected), "'{expected}' fehlt in: {reason}");
        }
    }

    #[test]
    fn ordinary_traffic_under_one_zone_stays_far_below_the_threshold() {
        // Der Fall, der den Detektor unbrauchbar machen würde: ein Haushalt
        // fragt unter einer Zone ein paar Dutzend Hosts. Der Detektor liefert
        // dafür durchaus einen Score — die Schwelle auszuwerten ist Sache von
        // `Detectors`. Geprüft wird deshalb der Abstand zur Schwelle, und der
        // soll groß sein und nicht knapp.
        let clock = Arc::new(TestClock::new());
        let detector = detector(Arc::clone(&clock));
        let hosts = [
            "www", "mail", "imap", "smtp", "cdn", "static", "images", "api", "login", "shop",
            "blog", "forum", "media", "assets", "video", "download", "support", "docs", "status",
            "admin",
        ];
        let mut worst = 0;
        for host in hosts {
            for _ in 0..5 {
                if let Some(finding) = ask(&detector, &format!("{host}.example.com"), RecordType::A)
                {
                    worst = worst.max(finding.score);
                }
            }
        }
        assert!(
            worst.saturating_mul(2) < super::super::permille(DEFAULT_THRESHOLD),
            "gewöhnlicher Verkehr kam auf {worst}"
        );
    }

    #[test]
    fn the_two_sample_streams_clear_the_default_threshold() {
        // Abnahmekriterium Schritt 3: der Beispielkorpus wird erkannt. Geprüft
        // wird gegen genau die Schwelle, die im Betrieb gilt.
        let threshold = super::super::permille(DEFAULT_THRESHOLD);
        for (label, generator) in [
            ("iodine", iodine_like as fn(usize) -> String),
            ("dnscat", dnscat_like as fn(usize) -> String),
        ] {
            let clock = Arc::new(TestClock::new());
            let detector = detector(Arc::clone(&clock));
            let mut best = 0;
            for i in 0..80 {
                if let Some(finding) = ask(&detector, &generator(i), RecordType::TXT) {
                    best = best.max(finding.score);
                }
            }
            assert!(
                best >= threshold,
                "{label} erreichte nur {best}, Schwelle ist {threshold}"
            );
        }
    }

    #[test]
    fn a_single_long_random_hostname_is_not_a_tunnel() {
        // So sieht jeder CDN-Hostname aus. Ohne die Mindestanzahl einmaliger
        // Subdomains wäre das ein Dauer-Fehlalarm.
        let clock = Arc::new(TestClock::new());
        let detector = detector(Arc::clone(&clock));
        for _ in 0..50 {
            assert!(ask(&detector, &iodine_like(1), RecordType::A).is_none());
        }
    }

    #[test]
    fn the_window_expires() {
        // Ein Tunnel über Wochen mit zehn Anfragen pro Tag fällt hier
        // absichtlich nicht auf — dafür ist der Zustand zu teuer.
        let clock = Arc::new(TestClock::new());
        let detector = detector(Arc::clone(&clock));
        for i in 0..11 {
            assert!(ask(&detector, &iodine_like(i), RecordType::TXT).is_none());
        }
        clock.advance(WINDOW + Duration::from_secs(1));
        for i in 11..22 {
            assert!(
                ask(&detector, &iodine_like(i), RecordType::TXT).is_none(),
                "das Fenster ist nicht abgelaufen"
            );
        }
        assert_eq!(detector.tracked_zones(), 1);
    }

    #[test]
    fn the_allow_list_exempts_a_zone_and_its_subzones() {
        let clock = Arc::new(TestClock::new());
        let detector = Tunneling::new(WINDOW, &["example.com".to_owned()], Arc::clone(&clock));
        for i in 0..80 {
            assert!(ask(&detector, &iodine_like(i), RecordType::TXT).is_none());
        }
        assert_eq!(
            detector.tracked_zones(),
            0,
            "die Zone wird trotzdem verfolgt"
        );
    }

    #[test]
    fn the_table_does_not_grow_with_traffic() {
        // Die Tabelle liegt im Anfragepfad. Ein Angreifer darf sie nicht
        // beliebig füllen können.
        let clock = Arc::new(TestClock::new());
        let detector = detector(Arc::clone(&clock));
        for i in 0..(MAX_ZONES * 2) {
            let _ = ask(&detector, &format!("host.zone{i}.example"), RecordType::A);
        }
        assert!(
            detector.tracked_zones() <= MAX_ZONES,
            "{} Zonen",
            detector.tracked_zones()
        );
    }

    #[test]
    fn the_unique_set_per_zone_is_bounded() {
        let clock = Arc::new(TestClock::new());
        let detector = detector(Arc::clone(&clock));
        for i in 0..(MAX_UNIQUE_TRACKED * 3) {
            let _ = ask(&detector, &iodine_like(i), RecordType::TXT);
        }
        let zones = detector
            .zones
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let window = zones.values().next().expect("eine Zone");
        assert!(window.unique.len() <= MAX_UNIQUE_TRACKED);
        // Trotzdem stimmt die Zahl, die in der Begründung steht.
        assert!(window.unique_count() >= u32::try_from(MAX_UNIQUE_TRACKED).unwrap_or(0));
    }

    #[test]
    fn entropy_separates_words_from_random_data() {
        assert!(entropy("www") < 2.0);
        assert!(entropy("login") < 2.5);
        // Base32-artige Nutzdaten liegen nahe am Maximum von 5 Bit.
        assert!(entropy("mfrggzdfmztwq2lknnwg23tp") > 3.5);
        assert_eq!(entropy(""), 0.0);
    }

    #[test]
    fn names_without_a_subdomain_are_ignored() {
        let clock = Arc::new(TestClock::new());
        let detector = detector(Arc::clone(&clock));
        for name in ["example.com", "com", "", ".", "..", "a"] {
            assert!(ask(&detector, name, RecordType::A).is_none(), "{name}");
        }
    }

    #[test]
    fn hostile_names_do_not_panic() {
        let clock = Arc::new(TestClock::new());
        let detector = detector(Arc::clone(&clock));
        let long_label = "a".repeat(300);
        let many_labels = "a.".repeat(2000);
        for name in [
            &long_label,
            &many_labels,
            "\u{0}.\u{0}.\u{0}",
            "....",
            &format!("{long_label}.{long_label}.example"),
        ] {
            let _ = ask(&detector, name, RecordType::TXT);
        }
    }
}
