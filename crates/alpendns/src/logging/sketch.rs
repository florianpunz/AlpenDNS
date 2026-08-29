//! Count-Min-Sketch mit k-Anonymitätsschwelle.
//!
//! Der Zweck ist nicht Speicher zu sparen, sondern **Namen gar nicht erst zu
//! speichern**: die Struktur besteht aus Zählern, aus denen sich kein Name
//! rekonstruieren lässt. Ein Name wird erst dann irgendwo abgelegt, wenn er die
//! Schwelle überschritten hat (ADR-0004, FEATURES.md P1).
//!
//! **Die Schwelle prüft auf der unteren Schätzgrenze.** Ein Count-Min-Sketch
//! *überschätzt* — der abgelesene Wert ist nie kleiner als der wahre. Würde man
//! direkt gegen `k` prüfen, käme eine einmal gefragte Domain durch, sobald
//! andere Domains auf dieselben Zähler fallen. Genau die einmaligen Aufrufe sind
//! die verräterischen. Deshalb wird die garantierte Fehlerschranke abgezogen.
//!
//! **Der Fehler wächst mit der Zahl der Anfragen.** Das ist die eingebaute
//! Richtung: mehr Verkehr heißt größere Schranke heißt *weniger* Domains in der
//! Statistik. Die Struktur versagt zur sicheren Seite.

use std::hash::{BuildHasher as _, RandomState};

/// Zähler je Zeile. Zweierpotenz, damit die Zuordnung eine Maskierung ist.
///
/// 2^18 Zähler × 4 Zeilen × 4 Byte = 4 MiB. Bei 200 000 Anfragen am Tag — viel
/// für ein Heimnetz — liegt die Fehlerschranke bei rund 2 und damit unter dem
/// üblichen `aggregate_k` von 5.
const WIDTH: usize = 1 << 18;

/// Zeilen. Jede zusätzliche Zeile senkt die Wahrscheinlichkeit, dass alle
/// gleichzeitig kollidieren.
const DEPTH: usize = 4;

/// Aufgerundetes e aus der Fehlerschranke ε = e/w des Count-Min-Sketch.
const E_ROUNDED_UP: u64 = 3;

#[derive(Debug)]
pub struct Sketch {
    rows: Vec<Vec<u32>>,
    hashers: Vec<RandomState>,
    total: u64,
}

impl Default for Sketch {
    fn default() -> Self {
        Self::new()
    }
}

impl Sketch {
    pub fn new() -> Self {
        Self {
            rows: (0..DEPTH).map(|_| vec![0_u32; WIDTH]).collect(),
            // Je Zeile eine eigene, beim Start zufällig gezogene Hash-Funktion.
            // Ohne das könnte jemand Kollisionen gezielt herbeiführen.
            hashers: (0..DEPTH).map(|_| RandomState::new()).collect(),
            total: 0,
        }
    }

    fn slot(&self, row: usize, key: &str) -> usize {
        let Some(state) = self.hashers.get(row) else {
            return 0;
        };
        // WIDTH ist eine Zweierpotenz, deshalb Maskierung statt Modulo.
        state.hash_one(key) as usize & (WIDTH - 1)
    }

    pub fn add(&mut self, key: &str) {
        self.total = self.total.saturating_add(1);
        for row in 0..DEPTH {
            let slot = self.slot(row, key);
            if let Some(counter) = self.rows.get_mut(row).and_then(|r| r.get_mut(slot)) {
                *counter = counter.saturating_add(1);
            }
        }
    }

    /// Obergrenze des wahren Zählerstands.
    pub fn estimate(&self, key: &str) -> u32 {
        (0..DEPTH)
            .filter_map(|row| {
                let slot = self.slot(row, key);
                self.rows.get(row).and_then(|r| r.get(slot)).copied()
            })
            .min()
            .unwrap_or(0)
    }

    /// Garantierte Fehlerschranke bei der bisherigen Zahl von Einträgen.
    pub fn error_bound(&self) -> u32 {
        let bound = self
            .total
            .saturating_mul(E_ROUNDED_UP)
            .checked_div(WIDTH as u64)
            .unwrap_or(0);
        u32::try_from(bound).unwrap_or(u32::MAX)
    }

    /// Untergrenze des wahren Zählerstands. Hierauf wird die Schwelle geprüft.
    pub fn lower_bound(&self, key: &str) -> u32 {
        self.estimate(key).saturating_sub(self.error_bound())
    }

    /// Ob dieser Name die k-Schwelle sicher überschritten hat.
    pub fn reaches(&self, key: &str, k: u32) -> bool {
        k > 0 && self.lower_bound(key) >= k
    }

    pub const fn total(&self) -> u64 {
        self.total
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_name_that_was_never_seen_counts_as_zero() {
        let sketch = Sketch::new();
        assert_eq!(sketch.estimate("example.com"), 0);
        assert_eq!(sketch.lower_bound("example.com"), 0);
    }

    #[test]
    fn counting_is_at_least_the_truth() {
        let mut sketch = Sketch::new();
        for _ in 0..10 {
            sketch.add("example.com");
        }
        assert!(
            sketch.estimate("example.com") >= 10,
            "ein Count-Min-Sketch darf nie unterschätzen"
        );
        assert_eq!(sketch.total(), 10);
    }

    #[test]
    fn a_single_query_never_reaches_the_threshold() {
        // Der eigentliche Zweck: genau die einmaligen Aufrufe sind die
        // verräterischen.
        let mut sketch = Sketch::new();
        sketch.add("peinlich.example.com");
        assert!(!sketch.reaches("peinlich.example.com", 5));
    }

    #[test]
    fn a_frequent_name_reaches_the_threshold() {
        let mut sketch = Sketch::new();
        for _ in 0..50 {
            sketch.add("haeufig.example.com");
        }
        assert!(sketch.reaches("haeufig.example.com", 5));
    }

    #[test]
    fn a_rare_name_stays_hidden_among_many_others() {
        // Genau der Fall, für den die untere Schranke da ist: viel Rauschen
        // ringsum darf die seltene Domain nicht über die Schwelle heben.
        let mut sketch = Sketch::new();
        for i in 0..200_000 {
            sketch.add(&format!("rausch{i}.example"));
        }
        sketch.add("einmalig.example.com");
        assert!(
            !sketch.reaches("einmalig.example.com", 5),
            "eine einmal gefragte Domain kam über die Schwelle"
        );
    }

    #[test]
    fn the_error_bound_grows_with_the_number_of_entries() {
        let mut sketch = Sketch::new();
        assert_eq!(sketch.error_bound(), 0);
        for i in 0..500_000 {
            sketch.add(&format!("d{i}.example"));
        }
        assert!(
            sketch.error_bound() > 0,
            "die Schranke bleibt bei viel Verkehr auf null"
        );
    }

    #[test]
    fn the_lower_bound_never_exceeds_the_estimate() {
        let mut sketch = Sketch::new();
        for i in 0..1000 {
            sketch.add(&format!("d{i}.example"));
        }
        for i in 0..100 {
            let key = format!("d{i}.example");
            assert!(sketch.lower_bound(&key) <= sketch.estimate(&key));
        }
    }

    #[test]
    fn a_threshold_of_zero_never_reports_anything() {
        // Sonst wäre k = 0 ein stiller Weg, die Schwelle abzuschalten.
        let mut sketch = Sketch::new();
        for _ in 0..100 {
            sketch.add("example.com");
        }
        assert!(!sketch.reaches("example.com", 0));
    }

    #[test]
    fn two_sketches_hash_differently() {
        // Ohne zufällige Hash-Funktionen könnte jemand Kollisionen gezielt
        // herbeiführen und so eine Domain über die Schwelle drücken.
        let (mut a, mut b) = (Sketch::new(), Sketch::new());
        let mut differ = false;
        for i in 0..64 {
            let key = format!("d{i}.example");
            a.add(&key);
            b.add(&key);
            if a.slot(0, &key) != b.slot(0, &key) {
                differ = true;
            }
        }
        assert!(differ, "beide Sketches benutzen dieselbe Hash-Funktion");
    }
}
