//! Neu registrierte Domains (NRD).
//!
//! Domains, die vor weniger als einem Monat registriert wurden, sind
//! überproportional oft bösartig — Phishing-Kampagnen brauchen frische Namen,
//! weil die alten auf Listen stehen. Das ist kein Beweis für irgendetwas: die
//! meisten frisch registrierten Domains sind harmlos, und eine Firma, die diese
//! Woche gegründet wurde, ist es auch. Deshalb steht dieser Detektor nie allein,
//! sondern liefert ein Signal neben anderen (FEATURES.md D5).
//!
//! **Die Schnittstelle ist eine Datei, kein API-Aufruf.** Das ist bewusst so
//! (Roadmap, Hinweis zu Phase 8): die Datei kommt aus dem AlpenShield-Projekt,
//! und der Resolver darf nicht davon abhängen, dass ein zweiter Dienst läuft.
//! Fehlt sie, läuft der Detektor leer statt den Start zu verhindern — Verfügbar-
//! keit geht vor Vollständigkeit (B.1 Regel 6).
//!
//! # Format
//!
//! Eine Zeile je Domain, Trennzeichen ist Leerraum. `#` beginnt einen
//! Kommentar. Alles nach dem Datum wird ignoriert, damit ein Feed weitere
//! Spalten mitführen kann, ohne dass hier etwas bricht.
//!
//! ```text
//! # erzeugt am 2026-08-30
//! kqxvbnzmrt.com      2026-08-28
//! frische-domain.at   2026-08-15   ns1.example.
//! ```

use std::collections::HashMap;
use std::path::Path;

use jiff::civil::Date;

use super::{Detector, Finding, NameDetector, Observation, Permille};

/// Sekunden je Tag. Die NRD-Datei führt Datumsangaben, die Konfiguration eine
/// Dauer — irgendwo muss umgerechnet werden.
#[expect(
    clippy::integer_division,
    reason = "aus einer Dauer werden ganze Tage; ein halber Tag hat hier keine Bedeutung"
)]
const fn as_days(duration: std::time::Duration) -> i64 {
    match i64::from_ne_bytes((duration.as_secs() / 86_400).to_ne_bytes()) {
        days if days >= 0 => days,
        _ => i64::MAX,
    }
}

/// Ein Verzeichnis frisch registrierter Domains.
pub struct Nrd<W> {
    /// Domain → Registrierungsdatum. Leer, wenn die Datei fehlte.
    registered: HashMap<String, Date>,
    /// Ab diesem Alter gilt eine Domain nicht mehr als neu.
    max_age_days: i64,
    wall: W,
}

// Von Hand, weil die Uhr kein `Debug` mitbringen muss — und weil die Tabelle
// Domainnamen enthält, die in keiner Debug-Ausgabe etwas verloren haben
// (B.1 Regel 3). Ausgegeben wird nur, wie viele es sind.
impl<W> std::fmt::Debug for Nrd<W> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Nrd")
            .field("entries", &self.registered.len())
            .field("max_age_days", &self.max_age_days)
            .finish_non_exhaustive()
    }
}

/// Was beim Einlesen schiefgegangen ist.
#[derive(Debug, thiserror::Error)]
pub enum LoadError {
    #[error("NRD-Datei {path} konnte nicht gelesen werden")]
    Read {
        path: String,
        #[source]
        source: std::io::Error,
    },
}

impl<W: crate::clock::WallClock> Nrd<W> {
    /// Liest die Datei ein. Eine fehlende Datei ist **kein** Fehler.
    ///
    /// Der Unterschied zu den Blocklisten (B.1 Regel 6, "fail closed bei
    /// Policy") ist Absicht: eine fehlende Blockliste hieße, ungefiltert im
    /// Internet zu hängen. Eine fehlende NRD-Datei heißt nur, dass ein Signal
    /// neben anderen fehlt — und dieser Detektor blockt per Default ohnehin
    /// nichts.
    pub fn load(path: &Path, max_age: std::time::Duration, wall: W) -> Result<Self, LoadError> {
        let max_age_days = as_days(max_age);
        let text = match std::fs::read_to_string(path) {
            Ok(text) => text,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                tracing::warn!(
                    path = %path.display(),
                    "NRD-Datei nicht vorhanden; der Detektor läuft leer mit"
                );
                String::new()
            }
            Err(source) => {
                return Err(LoadError::Read {
                    path: path.display().to_string(),
                    source,
                });
            }
        };

        let registered = parse(&text);
        tracing::info!(
            entries = registered.len(),
            max_age_days,
            "NRD-Liste geladen"
        );
        Ok(Self {
            registered,
            max_age_days,
            wall,
        })
    }

    /// Für Tests und für den Aufbau ohne Datei.
    pub fn from_entries(
        entries: impl IntoIterator<Item = (String, Date)>,
        max_age: std::time::Duration,
        wall: W,
    ) -> Self {
        Self {
            registered: entries
                .into_iter()
                .map(|(domain, date)| (normalize(&domain), date))
                .collect(),
            max_age_days: as_days(max_age),
            wall,
        }
    }

    pub fn len(&self) -> usize {
        self.registered.len()
    }

    pub fn is_empty(&self) -> bool {
        self.registered.is_empty()
    }

    /// Sucht den Namen und alle übergeordneten Zonen.
    ///
    /// Der Feed listet registrierbare Domains; gefragt wird nach
    /// `www.frische-domain.at`. Ohne den Aufstieg fände der Detektor nie etwas.
    fn lookup(&self, name: &str) -> Option<(&str, Date)> {
        let mut rest = name;
        loop {
            if let Some((key, date)) = self.registered.get_key_value(rest) {
                return Some((key.as_str(), *date));
            }
            rest = rest.split_once('.')?.1;
            if rest.is_empty() {
                return None;
            }
        }
    }
}

/// Liest das Dateiformat. Kaputte Zeilen werden übersprungen, nicht gemeldet.
///
/// Ein Feed mit einer Million Zeilen hat irgendwann eine kaputte; den Start
/// daran scheitern zu lassen wäre der falsche Kompromiss. Was durchkommt, ist
/// gültig — das ist die Zusicherung, nicht "die Datei war fehlerfrei".
fn parse(text: &str) -> HashMap<String, Date> {
    text.lines()
        .filter_map(|line| {
            let line = line.split('#').next().unwrap_or("").trim();
            if line.is_empty() {
                return None;
            }
            let mut fields = line.split_whitespace();
            let domain = fields.next()?;
            let date: Date = fields.next()?.parse().ok()?;
            Some((normalize(domain), date))
        })
        .collect()
}

fn normalize(domain: &str) -> String {
    domain.trim().trim_end_matches('.').to_lowercase()
}

/// Wie stark ein Alter ins Gewicht fällt.
///
/// Linear: heute registriert ergibt 1.000, genau am Rand des Fensters 0. Eine
/// Domain von gestern ist verdächtiger als eine von vor drei Wochen, und die
/// Abstufung soll das zeigen, statt alles im Fenster gleich zu behandeln.
#[expect(
    clippy::integer_division,
    reason = "die Kurve ist auf Promille gerundet; feiner wäre die Zahl ohnehin nicht"
)]
fn score_for(age_days: i64, max_age_days: i64) -> Permille {
    if max_age_days <= 0 || age_days >= max_age_days {
        return 0;
    }
    let remaining = max_age_days.saturating_sub(age_days.max(0));
    let permille = remaining.saturating_mul(1000) / max_age_days;
    Permille::try_from(permille).unwrap_or(1000)
}

impl<W: crate::clock::WallClock> NameDetector for Nrd<W> {
    fn detector(&self) -> Detector {
        Detector::Nrd
    }

    fn inspect(&self, observation: &Observation<'_>) -> Option<Finding> {
        if self.registered.is_empty() {
            return None;
        }
        let name = normalize(observation.name);
        let (domain, registered) = self.lookup(&name)?;

        let today = self.wall.now().date();
        let age_days = registered.until(today).ok()?.get_days().into();
        let score = score_for(age_days, self.max_age_days);
        if score == 0 {
            return None;
        }

        Some(Finding::new(
            Detector::Nrd,
            score,
            format!(
                "'{domain}' wurde am {registered} registriert, vor {age_days} Tagen \
                 (Schwelle: {} Tage)",
                self.max_age_days
            ),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::clock::FixedWallClock;
    use hickory_proto::rr::RecordType;
    use std::time::Duration;

    const MAX_AGE: Duration = Duration::from_secs(30 * 86_400);

    fn date(text: &str) -> Date {
        text.parse().expect("gültiges Datum")
    }

    fn today(text: &str) -> FixedWallClock {
        FixedWallClock::new(
            format!("{text}T12:00:00+02:00[Europe/Vienna]")
                .parse()
                .expect("gültiger Zeitpunkt"),
        )
    }

    fn nrd_with(entries: &[(&str, &str)], now: &str) -> Nrd<FixedWallClock> {
        Nrd::from_entries(
            entries
                .iter()
                .map(|(domain, when)| ((*domain).to_owned(), date(when))),
            MAX_AGE,
            today(now),
        )
    }

    fn inspect(detector: &Nrd<FixedWallClock>, name: &str) -> Option<Finding> {
        detector.inspect(&Observation {
            name,
            query_type: RecordType::A,
        })
    }

    #[test]
    fn a_domain_below_max_age_is_flagged() {
        // Das Abnahmekriterium aus Schritt 6.
        let detector = nrd_with(&[("frisch.example", "2026-08-15")], "2026-08-30");
        let finding = inspect(&detector, "frisch.example").expect("nicht geflaggt");
        assert!(finding.reason.contains("2026-08-15"), "{}", finding.reason);
        assert!(finding.reason.contains("15 Tagen"), "{}", finding.reason);
        assert_eq!(finding.detector, Detector::Nrd);
    }

    #[test]
    fn a_domain_above_max_age_is_not_flagged() {
        let detector = nrd_with(&[("alt.example", "2026-01-01")], "2026-08-30");
        assert!(inspect(&detector, "alt.example").is_none());
    }

    #[test]
    fn the_boundary_belongs_to_the_old_ones() {
        // Genau 30 Tage alt: außerhalb. Sonst wandert die Grenze mit der Uhrzeit.
        let detector = nrd_with(&[("rand.example", "2026-07-31")], "2026-08-30");
        assert!(inspect(&detector, "rand.example").is_none());
        let detector = nrd_with(&[("rand.example", "2026-08-01")], "2026-08-30");
        assert!(inspect(&detector, "rand.example").is_some());
    }

    #[test]
    fn fresher_scores_higher() {
        let gestern = nrd_with(&[("a.example", "2026-08-29")], "2026-08-30");
        let vorwochen = nrd_with(&[("a.example", "2026-08-05")], "2026-08-30");
        let young = inspect(&gestern, "a.example").expect("geflaggt");
        let older = inspect(&vorwochen, "a.example").expect("geflaggt");
        assert!(
            young.score > older.score,
            "{} vs {}",
            young.score,
            older.score
        );
    }

    #[test]
    fn subdomains_inherit_the_registration_date() {
        // Der Feed listet registrierbare Domains, gefragt wird nach Hosts.
        let detector = nrd_with(&[("frisch.example", "2026-08-28")], "2026-08-30");
        for name in [
            "frisch.example",
            "www.frisch.example",
            "a.b.c.frisch.example",
        ] {
            assert!(inspect(&detector, name).is_some(), "{name}");
        }
        assert!(inspect(&detector, "frisch.example.com").is_none());
        assert!(inspect(&detector, "nichtfrisch.example").is_none());
    }

    #[test]
    fn an_empty_list_finds_nothing() {
        let detector = nrd_with(&[], "2026-08-30");
        assert!(detector.is_empty());
        assert!(inspect(&detector, "frisch.example").is_none());
    }

    #[test]
    fn a_date_in_the_future_is_treated_as_brand_new() {
        // Kommt vor, wenn der Feed in einer anderen Zeitzone erzeugt wurde.
        // Ein negatives Alter darf keinen Überlauf und keinen Score über 1.000
        // ergeben.
        let detector = nrd_with(&[("morgen.example", "2026-09-05")], "2026-08-30");
        let finding = inspect(&detector, "morgen.example").expect("geflaggt");
        assert_eq!(finding.score, 1000);
    }

    #[test]
    fn the_file_format_survives_comments_and_junk() {
        let text = "\
# erzeugt am 2026-08-30
frisch.example      2026-08-28
mit-spalten.example 2026-08-27  ns1.example.  irgendwas

   eingerueckt.example   2026-08-26
kaputt.example      nicht-ein-datum
ohne-datum.example
GROSS.EXAMPLE       2026-08-25
mit-punkt.example.  2026-08-24
kommentar.example   2026-08-23  # dahinter
";
        let parsed = parse(text);
        assert_eq!(parsed.len(), 6, "{parsed:?}");
        assert_eq!(parsed.get("frisch.example"), Some(&date("2026-08-28")));
        assert_eq!(parsed.get("gross.example"), Some(&date("2026-08-25")));
        assert_eq!(parsed.get("mit-punkt.example"), Some(&date("2026-08-24")));
        assert_eq!(parsed.get("kommentar.example"), Some(&date("2026-08-23")));
        assert!(!parsed.contains_key("kaputt.example"));
        assert!(!parsed.contains_key("ohne-datum.example"));
    }

    #[test]
    fn a_missing_file_is_not_a_startup_error() {
        // Der Resolver hängt nicht davon ab, dass AlpenShield gelaufen ist.
        let loaded = Nrd::load(
            Path::new("/nonexistent/pfad/nrd.txt"),
            MAX_AGE,
            today("2026-08-30"),
        )
        .expect("eine fehlende Datei ist kein Fehler");
        assert!(loaded.is_empty());
    }

    #[test]
    fn a_real_file_is_read() {
        let dir = std::env::temp_dir().join(format!("alpendns-nrd-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("Verzeichnis");
        let path = dir.join("nrd.txt");
        std::fs::write(&path, "frisch.example 2026-08-28\n").expect("schreiben");

        let loaded = Nrd::load(&path, MAX_AGE, today("2026-08-30")).expect("lesbar");
        assert_eq!(loaded.len(), 1);
        assert!(inspect(&loaded, "www.frisch.example").is_some());

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn the_score_curve_stays_inside_its_bounds() {
        for age in [-5_i64, 0, 1, 15, 29, 30, 31, 10_000, i64::MAX] {
            let score = score_for(age, 30);
            assert!(score <= 1000, "Alter {age} ergab {score}");
        }
        assert_eq!(score_for(0, 30), 1000);
        assert_eq!(score_for(30, 30), 0);
        assert_eq!(score_for(0, 0), 0, "eine Schwelle von null flaggt nichts");
    }

    #[test]
    fn hostile_names_do_not_panic() {
        let detector = nrd_with(&[("frisch.example", "2026-08-28")], "2026-08-30");
        let long = "a.".repeat(2000);
        for name in ["", ".", "..", "...", &long, "\u{0}"] {
            let _ = inspect(&detector, name);
        }
    }
}
