//! Exakte Häufigkeiten hinter einer k-Anonymitätsschwelle.
//!
//! Der Zweck ist nicht, Speicher zu sparen, sondern **Namen gar nicht erst zu
//! speichern**: die Tabelle hält Zähler unter einem gesalzenen Hash, aus dem
//! sich kein Name zurückrechnen lässt. Der Name selbst wird erst abgelegt, wenn
//! er die Schwelle überschritten hat (ADR-0004, FEATURES.md P1).
//!
//! **Gezählt wird exakt.** Der Vorgänger war ein Count-Min-Sketch, der
//! überschätzt: er brauchte eine Fehlerschranke, die mit dem Verkehr wächst, und
//! die Schwelle musste auf der unteren Schätzgrenze prüfen, damit eine einmal
//! gefragte Domain nicht durch fremde Kollisionen über die Schwelle rutscht. Bei
//! der Kardinalität eines Haushalts-Resolvers ist eine exakte Tabelle nicht
//! teurer und hat das Problem nicht ([ADR-0015](../../../../docs/adr/0015-exakte-zaehlung-statt-sketch.md)).
//!
//! **Die Tabelle ist gedeckelt.** Ein Sketch hat eine feste Größe; eine
//! HashMap wächst mit der Zahl der verschiedenen Namen, und die ist von außen
//! steuerbar — ein Gerät, das zufällige Subdomains abfragt (DNS-Tunneling, aber
//! auch bloß ein Browser mit NXDOMAIN-Proben), triebe sie sonst beliebig weit.
//! Ist [`MAX_TRACKED`] erreicht, bekommen neue Namen keinen Zähler mehr und
//! erreichen damit auch die Schwelle nicht: die Struktur versagt zur sicheren
//! Seite.

use std::collections::HashMap;
use std::hash::{BuildHasher as _, RandomState};

/// So viele verschiedene Namen werden gleichzeitig gezählt.
///
/// 200 000 ist reichlich für einen Haushalt — die Messung in BENCHMARKS.md
/// kostet dabei rund 5 MB, dieselbe Größenordnung wie die 4 MiB, die der
/// Count-Min-Sketch fest belegte. Kein Konfigurationsschlüssel: wer diese Zahl
/// erreicht, hat kein Einstellungs-, sondern ein Missbrauchsproblem.
const MAX_TRACKED: usize = 200_000;

/// Zähler je Name, ohne die Namen.
#[derive(Debug)]
pub struct Counts {
    /// Schlüssel ist der gesalzene Hash des Namens, nicht der Name.
    counts: HashMap<u64, u32>,
    /// Beim Start zufällig gezogen. Ohne das wäre der Hash über alle
    /// Installationen derselbe und aus einem Speicherabbild per Wörterbuch
    /// umkehrbar.
    hasher: RandomState,
    total: u64,
    /// Namen, die keinen Zähler mehr bekamen, weil die Tabelle voll war.
    dropped: u64,
}

impl Default for Counts {
    fn default() -> Self {
        Self::new()
    }
}

impl Counts {
    pub fn new() -> Self {
        Self {
            counts: HashMap::new(),
            hasher: RandomState::new(),
            total: 0,
            dropped: 0,
        }
    }

    fn slot(&self, key: &str) -> u64 {
        self.hasher.hash_one(key)
    }

    /// Zählt einen Namen und gibt seinen neuen Stand zurück.
    ///
    /// Gibt 0 zurück, wenn die Tabelle voll ist und dieser Name neu wäre — dann
    /// erreicht er die Schwelle nie, und das ist die gewollte Richtung.
    pub fn add(&mut self, key: &str) -> u32 {
        self.total = self.total.saturating_add(1);
        let slot = self.slot(key);
        if let Some(counter) = self.counts.get_mut(&slot) {
            *counter = counter.saturating_add(1);
            return *counter;
        }
        if self.counts.len() >= MAX_TRACKED {
            self.dropped = self.dropped.saturating_add(1);
            return 0;
        }
        self.counts.insert(slot, 1);
        1
    }

    /// Wie oft dieser Name gezählt wurde.
    pub fn count(&self, key: &str) -> u32 {
        self.counts.get(&self.slot(key)).copied().unwrap_or(0)
    }

    /// Ob dieser Name die k-Schwelle überschritten hat.
    pub fn reaches(&self, key: &str, k: u32) -> bool {
        k > 0 && self.count(key) >= k
    }

    pub const fn total(&self) -> u64 {
        self.total
    }

    /// Anfragen, für die kein Zähler mehr angelegt wurde. Für Metriken.
    pub const fn dropped(&self) -> u64 {
        self.dropped
    }

    /// Wie viele verschiedene Namen gerade gezählt werden.
    pub fn tracked(&self) -> usize {
        self.counts.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_name_that_was_never_seen_counts_as_zero() {
        let counts = Counts::new();
        assert_eq!(counts.count("example.com"), 0);
        assert!(!counts.reaches("example.com", 5));
    }

    #[test]
    fn counting_is_exact() {
        // Der eigentliche Unterschied zum Sketch: keine Schätzung, keine
        // Fehlerschranke, kein Überschätzen.
        let mut counts = Counts::new();
        for expected in 1..=10 {
            assert_eq!(counts.add("example.com"), expected);
        }
        assert_eq!(counts.count("example.com"), 10);
        assert_eq!(counts.total(), 10);
    }

    #[test]
    fn a_single_query_never_reaches_the_threshold() {
        // Genau die einmaligen Aufrufe sind die verräterischen.
        let mut counts = Counts::new();
        counts.add("peinlich.example.com");
        assert!(!counts.reaches("peinlich.example.com", 5));
    }

    #[test]
    fn a_frequent_name_reaches_the_threshold_exactly_at_k() {
        let mut counts = Counts::new();
        for _ in 0..4 {
            counts.add("haeufig.example.com");
        }
        assert!(!counts.reaches("haeufig.example.com", 5), "einer zu früh");
        counts.add("haeufig.example.com");
        assert!(counts.reaches("haeufig.example.com", 5));
    }

    #[test]
    fn a_rare_name_stays_hidden_among_many_others() {
        // Beim Sketch war das der Fall, für den es die untere Schranke brauchte:
        // fremdes Rauschen hob den seltenen Namen über die Schwelle. Exakt
        // gezählt kann das nicht passieren — der Test bleibt trotzdem stehen,
        // weil er die Zusicherung prüft, nicht die Implementierung.
        let mut counts = Counts::new();
        for i in 0..100_000 {
            counts.add(&format!("rausch{i}.example"));
        }
        counts.add("einmalig.example.com");
        assert_eq!(counts.count("einmalig.example.com"), 1);
        assert!(!counts.reaches("einmalig.example.com", 5));
    }

    #[test]
    fn a_threshold_of_zero_never_reports_anything() {
        // Sonst wäre k = 0 ein stiller Weg, die Schwelle abzuschalten.
        let mut counts = Counts::new();
        for _ in 0..100 {
            counts.add("example.com");
        }
        assert!(!counts.reaches("example.com", 0));
    }

    #[test]
    fn two_tables_hash_differently() {
        // Ohne zufälliges Salz wäre der Schlüssel über alle Installationen
        // derselbe und aus einem Speicherabbild per Wörterbuch umkehrbar.
        let (a, b) = (Counts::new(), Counts::new());
        let differ = (0..64).any(|i| {
            let key = format!("d{i}.example");
            a.slot(&key) != b.slot(&key)
        });
        assert!(differ, "beide Tabellen benutzen dasselbe Salz");
    }

    #[test]
    fn the_table_stops_growing_and_says_so() {
        // Ein Gerät mit zufälligen Subdomains darf den Speicher nicht treiben.
        let mut counts = Counts::new();
        for i in 0..MAX_TRACKED.saturating_add(1000) {
            counts.add(&format!("d{i}.example"));
        }
        assert_eq!(counts.tracked(), MAX_TRACKED);
        assert!(counts.dropped() >= 1000, "{}", counts.dropped());
    }

    #[test]
    fn a_name_beyond_the_cap_never_reaches_the_threshold() {
        // Fail closed: kein Zähler heißt keine Schwelle, nicht "irgendein Wert".
        let mut counts = Counts::new();
        for i in 0..MAX_TRACKED {
            counts.add(&format!("d{i}.example"));
        }
        for _ in 0..50 {
            assert_eq!(counts.add("spaet.example.com"), 0);
        }
        assert!(!counts.reaches("spaet.example.com", 5));
    }

    #[test]
    fn names_already_counted_keep_counting_when_the_table_is_full() {
        let mut counts = Counts::new();
        counts.add("frueh.example.com");
        for i in 0..MAX_TRACKED {
            counts.add(&format!("d{i}.example"));
        }
        assert_eq!(counts.add("frueh.example.com"), 2, "der Zähler steht still");
    }
}
