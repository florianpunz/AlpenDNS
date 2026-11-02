//! Antwortzeiten als Perzentile, aus einem festen Histogramm.
//!
//! Ein Ring der letzten Messwerte wäre genauer, aber er müsste bei jeder
//! Anfrage hinter einer Sperre wachsen und beim Auslesen sortiert werden. Das
//! Histogramm kostet stattdessen einen atomaren Zähler und behält nur Summen —
//! dieselbe Zusicherung wie [`crate::history`]: es kann keine Namen enthalten,
//! weil es keine Felder dafür hat.
//!
//! Die Eimer sind logarithmisch: 16 je Verdopplung, also 1 µs bis 67 s mit
//! höchstens 6,25 % Überschätzung. Für eine Anzeige in Millisekunden ist das
//! genauer als die Stelle, an der sie steht; für eine Messung wäre es zu grob.

use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

/// Eimer je Verdopplung. 16 ergibt eine Schrittweite von 1/16 des unteren Randes.
const SHIFT: u32 = 4;
const PER_OCTAVE: usize = 1 << SHIFT;

/// Verdopplungen von 1 µs bis 67 s. Darüber wird in den letzten Eimer gelegt.
const OCTAVES: usize = 26;

const BUCKETS: usize = PER_OCTAVE * OCTAVES;

/// Ein festes Histogramm über Antwortzeiten.
#[derive(Debug)]
pub struct Histogram {
    buckets: [AtomicU64; BUCKETS],
}

impl Default for Histogram {
    fn default() -> Self {
        Self::new()
    }
}

/// Die beiden Werte, die eine Anzeige braucht.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Percentiles {
    /// Obergrenze des Eimers, in dem die Median-Messung liegt.
    pub p50: Duration,
    /// Obergrenze des Eimers, in dem die 95-%-Messung liegt.
    pub p95: Duration,
}

impl Histogram {
    pub const fn new() -> Self {
        Self {
            buckets: [const { AtomicU64::new(0) }; BUCKETS],
        }
    }

    /// Trägt eine Messung ein.
    pub fn observe(&self, elapsed: Duration) {
        let micros = u64::try_from(elapsed.as_micros()).unwrap_or(u64::MAX);
        if let Some(bucket) = self.buckets.get(bucket_of(micros)) {
            bucket.fetch_add(1, Ordering::Relaxed);
        }
    }

    /// Median und 95-%-Wert, oder `None`, solange nichts gemessen wurde.
    ///
    /// Zwischen dem Summieren und dem Durchlaufen kann eine parallele Messung
    /// den Zähler erhöhen. Dann kann `seen` hinter `total` zurückbleiben; die
    /// Rückfallwerte am Ende halten das Ergebnis trotzdem gültig, statt hier
    /// einen `unwrap` zu brauchen.
    pub fn percentiles(&self) -> Option<Percentiles> {
        let total = self
            .buckets
            .iter()
            .map(|bucket| bucket.load(Ordering::Relaxed))
            .sum::<u64>();
        if total == 0 {
            return None;
        }

        let mut seen = 0_u64;
        let mut p50 = None;
        let mut p95 = None;
        for (index, bucket) in self.buckets.iter().enumerate() {
            seen = seen.saturating_add(bucket.load(Ordering::Relaxed));
            if p50.is_none() && seen.saturating_mul(2) >= total {
                p50 = Some(upper_micros(index));
            }
            if p95.is_none() && seen.saturating_mul(20) >= total.saturating_mul(19) {
                p95 = Some(upper_micros(index));
            }
        }

        let last = upper_micros(BUCKETS.saturating_sub(1));
        Some(Percentiles {
            p50: Duration::from_micros(p50.unwrap_or(last)),
            p95: Duration::from_micros(p95.unwrap_or(last)),
        })
    }
}

/// In welchen Eimer eine Messung fällt.
fn bucket_of(micros: u64) -> usize {
    let value = micros.max(1);
    let exponent = usize::try_from(u64::BITS - 1 - value.leading_zeros()).unwrap_or(0);
    let base = 1_u64 << exponent;
    let step_shift = u32::try_from(exponent.saturating_sub(SHIFT as usize)).unwrap_or(0);
    let offset = usize::try_from((value - base) >> step_shift).unwrap_or(PER_OCTAVE - 1);
    exponent
        .saturating_mul(PER_OCTAVE)
        .saturating_add(offset.min(PER_OCTAVE))
        .min(BUCKETS.saturating_sub(1))
}

/// Die obere Grenze eines Eimers in Mikrosekunden.
fn upper_micros(index: usize) -> u64 {
    let exponent = u32::try_from(index >> SHIFT).unwrap_or(0);
    let offset = u64::try_from(index & (PER_OCTAVE - 1)).unwrap_or(0);
    let base = 1_u64.checked_shl(exponent).unwrap_or(u64::MAX);
    let step = 1_u64
        .checked_shl(exponent.saturating_sub(SHIFT))
        .unwrap_or(u64::MAX);
    base.saturating_add(step.saturating_mul(offset.saturating_add(1)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_empty_histogram_has_no_percentiles() {
        assert_eq!(Histogram::new().percentiles(), None);
    }

    #[test]
    fn one_measurement_is_both_values() {
        let histogram = Histogram::new();
        histogram.observe(Duration::from_millis(10));
        let percentiles = histogram.percentiles().expect("Messung");
        assert_eq!(percentiles.p50, percentiles.p95);
        assert!(percentiles.p50 >= Duration::from_millis(10));
        assert!(percentiles.p50 < Duration::from_millis(11));
    }

    #[test]
    fn the_ninety_fifth_percentile_ignores_the_bulk() {
        let histogram = Histogram::new();
        for _ in 0..90 {
            histogram.observe(Duration::from_micros(100));
        }
        for _ in 0..10 {
            histogram.observe(Duration::from_millis(800));
        }
        let percentiles = histogram.percentiles().expect("Messungen");
        assert!(percentiles.p50 < Duration::from_millis(1));
        assert!(percentiles.p95 >= Duration::from_millis(800));
    }

    #[test]
    fn every_measurement_lands_within_its_reported_bound() {
        for micros in [1, 2, 7, 15, 16, 99, 1_000, 12_345, 999_999, 4_000_000] {
            let histogram = Histogram::new();
            histogram.observe(Duration::from_micros(micros));
            let percentiles = histogram.percentiles().expect("Messung");
            assert!(
                percentiles.p50 >= Duration::from_micros(micros),
                "{micros} µs liegt über seiner Obergrenze"
            );
            let bound = u128::from(micros) + (u128::from(micros) >> SHIFT).max(1);
            assert!(
                percentiles.p50.as_micros() <= bound,
                "{micros} µs wird zu grob überschätzt"
            );
        }
    }

    #[test]
    fn a_measurement_beyond_the_last_bucket_does_not_overflow() {
        let histogram = Histogram::new();
        histogram.observe(Duration::from_secs(60 * 60));
        let percentiles = histogram.percentiles().expect("Messung");
        assert!(percentiles.p95 <= Duration::from_secs(90));
    }
}
