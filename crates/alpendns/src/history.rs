//! Zeitreihe der letzten 24 Stunden — ausschließlich aus Zählern.
//!
//! **Was hier nicht steht, ist der Punkt:** kein Name, kein Client, keine
//! einzelne Anfrage. Die Struktur kann gar nichts anderes aufnehmen als
//! Summen, weil sie nur [`Sample`] entgegennimmt — und `Sample` hat keine
//! Felder für Namen. Ein Diagramm über 24 Stunden ist damit nicht der Anfang
//! eines Query-Logs, sondern dessen Gegenteil: die Kurve zeigt *wie viel*,
//! niemals *was* (ADR-0004).
//!
//! **Keine neue Persistenz.** Der Ring liegt im RAM und ist nach einem Neustart
//! weg. Das ist keine fehlende Funktion, sondern die Zusicherung: was der
//! Prozess nicht überlebt, kann auch niemand beschlagnahmen. Ein Prometheus
//! daneben darf die Zahlen gerne behalten — dann ist das eine Entscheidung des
//! Betreibers und steht in dessen Konfiguration, nicht in unserer.
//!
//! Gespeichert werden **Differenzen je Eimer**, nicht die Zählerstände selbst:
//! die Kurve soll den Verkehr eines Zeitfensters zeigen, nicht eine monoton
//! steigende Treppe.

use std::collections::VecDeque;
use std::time::{Duration, Instant};

/// Breite eines Eimers. Fünf Minuten mal 288 ergibt genau 24 Stunden.
pub const BUCKET: Duration = Duration::from_secs(300);

/// So viele Eimer werden behalten.
pub const BUCKETS: usize = 288;

/// Ein Zählerstand zu einem Zeitpunkt. Absolut, nicht als Differenz.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Sample {
    pub queries: u64,
    pub blocked: u64,
    pub cache_hits: u64,
    pub cache_misses: u64,
    /// Erfolgreiche Anfragen je Upstream, in der Reihenfolge von
    /// [`History::upstreams`].
    pub upstreams: Vec<u64>,
}

/// Der Verkehr eines Zeitfensters.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize)]
pub struct Bucket {
    pub queries: u64,
    pub blocked: u64,
    pub cache_hits: u64,
    pub cache_misses: u64,
    pub upstreams: Vec<u64>,
}

impl Bucket {
    fn add(&mut self, delta: &Sample) {
        self.queries = self.queries.saturating_add(delta.queries);
        self.blocked = self.blocked.saturating_add(delta.blocked);
        self.cache_hits = self.cache_hits.saturating_add(delta.cache_hits);
        self.cache_misses = self.cache_misses.saturating_add(delta.cache_misses);
        if self.upstreams.len() < delta.upstreams.len() {
            self.upstreams.resize(delta.upstreams.len(), 0);
        }
        for (slot, value) in self.upstreams.iter_mut().zip(&delta.upstreams) {
            *slot = slot.saturating_add(*value);
        }
    }
}

/// Der Ringpuffer über 24 Stunden.
///
/// Die Zeit kommt als Parameter herein statt aus einer Uhr im Inneren: die
/// Struktur bleibt damit ohne `Clock`-Generik testbar, und der Aufrufer, der
/// ohnehin schon eine Uhr hat, gibt sie einfach weiter (B.3 Regel 4).
#[derive(Debug)]
pub struct History {
    buckets: VecDeque<Bucket>,
    /// Wann der jüngste Eimer begonnen hat.
    current_started: Instant,
    /// Der letzte gesehene Zählerstand, um die Differenz zu bilden.
    last: Option<Sample>,
    upstreams: Vec<String>,
}

impl History {
    pub fn new(now: Instant) -> Self {
        let mut buckets = VecDeque::with_capacity(BUCKETS);
        buckets.push_back(Bucket::default());
        Self {
            buckets,
            current_started: now,
            last: None,
            upstreams: Vec::new(),
        }
    }

    /// Die Namen der Upstreams, passend zur Reihenfolge in [`Bucket::upstreams`].
    pub fn upstreams(&self) -> &[String] {
        &self.upstreams
    }

    pub const fn bucket_seconds(&self) -> u64 {
        BUCKET.as_secs()
    }

    /// Nimmt einen Zählerstand auf.
    ///
    /// Der erste Aufruf legt nur den Bezugspunkt fest — ohne Vorgänger gibt es
    /// keine Differenz, und den Zählerstand beim Start als Verkehr der ersten
    /// fünf Minuten zu buchen wäre schlicht falsch.
    pub fn observe(&mut self, sample: Sample, names: &[String], now: Instant) {
        if self.upstreams.len() != names.len() {
            self.upstreams = names.to_vec();
        }
        self.roll(now);

        let Some(previous) = self.last.replace(sample.clone()) else {
            return;
        };
        // Zähler laufen nur vorwärts; eine negative Differenz kann es nicht
        // geben. `saturating_sub` fängt trotzdem den Fall ab, dass ein Upstream
        // verschwindet und die Spalten sich verschieben.
        let delta = Sample {
            queries: sample.queries.saturating_sub(previous.queries),
            blocked: sample.blocked.saturating_sub(previous.blocked),
            cache_hits: sample.cache_hits.saturating_sub(previous.cache_hits),
            cache_misses: sample.cache_misses.saturating_sub(previous.cache_misses),
            upstreams: sample
                .upstreams
                .iter()
                .zip(previous.upstreams.iter().chain(std::iter::repeat(&0)))
                .map(|(now, before)| now.saturating_sub(*before))
                .collect(),
        };

        if let Some(current) = self.buckets.back_mut() {
            current.add(&delta);
        }
    }

    /// Schiebt so viele Eimer weiter, wie seit dem letzten Mal vergangen sind.
    fn roll(&mut self, now: Instant) {
        let elapsed = now.saturating_duration_since(self.current_started);
        // Bei einer sehr langen Pause bleibt kein alter Eimer übrig; mehr als
        // BUCKETS Schritte wären verschwendete Arbeit.
        #[expect(
            clippy::integer_division,
            reason = "ganze Eimer sind hier die Frage, der Rest läuft im aktuellen weiter"
        )]
        let steps = usize::try_from(elapsed.as_secs() / BUCKET.as_secs())
            .unwrap_or(BUCKETS)
            .min(BUCKETS);
        for _ in 0..steps {
            self.buckets.push_back(Bucket::default());
            if self.buckets.len() > BUCKETS {
                self.buckets.pop_front();
            }
        }
        if steps > 0 {
            self.current_started = self
                .current_started
                .checked_add(BUCKET.saturating_mul(u32::try_from(steps).unwrap_or(1)))
                .unwrap_or(now);
        }
    }

    /// Die Eimer, ältester zuerst.
    pub fn buckets(&self) -> Vec<Bucket> {
        self.buckets.iter().cloned().collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample(queries: u64, blocked: u64) -> Sample {
        Sample {
            queries,
            blocked,
            cache_hits: 0,
            cache_misses: 0,
            upstreams: vec![queries],
        }
    }

    const NAMES: [&str; 1] = ["quad9"];

    fn names() -> Vec<String> {
        NAMES.iter().map(|n| (*n).to_owned()).collect()
    }

    #[test]
    fn the_first_sample_only_sets_the_reference_point() {
        // Der Zählerstand beim ersten Blick ist kein Verkehr dieses Fensters.
        let start = Instant::now();
        let mut history = History::new(start);
        history.observe(sample(5_000, 100), &names(), start);
        assert_eq!(history.buckets().first().map(|b| b.queries), Some(0));
    }

    #[test]
    fn differences_land_in_the_current_bucket() {
        let start = Instant::now();
        let mut history = History::new(start);
        history.observe(sample(100, 10), &names(), start);
        history.observe(sample(180, 12), &names(), start + Duration::from_secs(30));
        let buckets = history.buckets();
        assert_eq!(buckets.len(), 1);
        assert_eq!(
            buckets.first().map(|b| (b.queries, b.blocked)),
            Some((80, 2))
        );
    }

    #[test]
    fn a_new_bucket_starts_after_five_minutes() {
        let start = Instant::now();
        let mut history = History::new(start);
        history.observe(sample(0, 0), &names(), start);
        history.observe(sample(10, 0), &names(), start + Duration::from_secs(60));
        history.observe(
            sample(25, 0),
            &names(),
            start + BUCKET + Duration::from_secs(60),
        );
        let buckets = history.buckets();
        assert_eq!(buckets.len(), 2);
        assert_eq!(buckets.first().map(|b| b.queries), Some(10));
        assert_eq!(buckets.get(1).map(|b| b.queries), Some(15));
    }

    #[test]
    fn the_ring_never_grows_past_a_day() {
        let start = Instant::now();
        let mut history = History::new(start);
        history.observe(sample(0, 0), &names(), start);
        for step in 1..=(BUCKETS + 50) {
            let at = start + BUCKET * u32::try_from(step).unwrap_or(1);
            history.observe(sample(step as u64 * 10, 0), &names(), at);
        }
        assert_eq!(history.buckets().len(), BUCKETS);
    }

    #[test]
    fn a_long_pause_does_not_walk_the_ring_more_than_once() {
        // Ein schlafender Rechner darf beim Aufwachen keine Millionen Eimer
        // erzeugen.
        let start = Instant::now();
        let mut history = History::new(start);
        history.observe(sample(0, 0), &names(), start);
        history.observe(sample(1, 0), &names(), start + Duration::from_secs(400_000));
        assert_eq!(history.buckets().len(), BUCKETS);
    }

    #[test]
    fn upstream_columns_follow_the_names() {
        let start = Instant::now();
        let mut history = History::new(start);
        let names = vec!["a".to_owned(), "b".to_owned()];
        let first = Sample {
            queries: 10,
            blocked: 0,
            cache_hits: 0,
            cache_misses: 0,
            upstreams: vec![6, 4],
        };
        let second = Sample {
            queries: 30,
            blocked: 0,
            cache_hits: 0,
            cache_misses: 0,
            upstreams: vec![20, 10],
        };
        history.observe(first, &names, start);
        history.observe(second, &names, start + Duration::from_secs(10));
        assert_eq!(history.upstreams(), names.as_slice());
        assert_eq!(
            history.buckets().first().map(|b| b.upstreams.clone()),
            Some(vec![14, 6])
        );
    }
}
