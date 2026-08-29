//! Ringpuffer für den Log-Modus `ring`.
//!
//! Die letzten Sekunden im RAM, nie auf Platte. Das ist der Kompromiss aus
//! ADR-0004: "warum geht diese Seite nicht mehr" ist damit beantwortbar, ohne
//! dass eine Datei entsteht, die einen Haushalt beschreibt.

use std::collections::VecDeque;
use std::time::{Duration, Instant};

/// Obergrenze unabhängig vom Zeitfenster.
///
/// Ohne sie könnte ein Lastspitze den Speicher füllen, bevor das Zeitfenster
/// greift: bei 100 000 Anfragen pro Sekunde wären fünf Minuten dreißig Millionen
/// Einträge.
const MAX_ENTRIES: usize = 20_000;

/// Ein Eintrag mit dem Zeitpunkt, zu dem er entstand.
#[derive(Debug, Clone)]
struct Slot<T> {
    at: Instant,
    value: T,
}

/// Hält Einträge für ein Zeitfenster.
#[derive(Debug)]
pub struct Ring<T> {
    entries: VecDeque<Slot<T>>,
    window: Duration,
}

impl<T: Clone> Ring<T> {
    pub fn new(window: Duration) -> Self {
        Self {
            entries: VecDeque::new(),
            window,
        }
    }

    pub fn push(&mut self, now: Instant, value: T) {
        self.expire(now);
        if self.entries.len() >= MAX_ENTRIES {
            self.entries.pop_front();
        }
        self.entries.push_back(Slot { at: now, value });
    }

    /// Wirft alles weg, was älter als das Fenster ist.
    fn expire(&mut self, now: Instant) {
        while self
            .entries
            .front()
            .is_some_and(|slot| now.saturating_duration_since(slot.at) > self.window)
        {
            self.entries.pop_front();
        }
    }

    /// Die jüngsten `limit` Einträge, neueste zuerst.
    pub fn recent(&mut self, now: Instant, limit: usize) -> Vec<T> {
        self.expire(now);
        self.entries
            .iter()
            .rev()
            .take(limit)
            .map(|slot| slot.value.clone())
            .collect()
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ring() -> Ring<u32> {
        Ring::new(Duration::from_secs(300))
    }

    #[test]
    fn entries_come_back_newest_first() {
        let mut ring = ring();
        let now = Instant::now();
        for i in 0..5 {
            ring.push(now, i);
        }
        assert_eq!(ring.recent(now, 3), vec![4, 3, 2]);
    }

    #[test]
    fn entries_older_than_the_window_are_gone() {
        let mut ring = ring();
        let start = Instant::now();
        ring.push(start, 1);

        let later = start
            .checked_add(Duration::from_secs(301))
            .expect("kein Überlauf");
        assert!(
            ring.recent(later, 10).is_empty(),
            "alter Eintrag blieb liegen"
        );
        assert_eq!(ring.len(), 0);
    }

    #[test]
    fn entries_inside_the_window_survive() {
        let mut ring = ring();
        let start = Instant::now();
        ring.push(start, 1);
        let later = start
            .checked_add(Duration::from_secs(299))
            .expect("kein Überlauf");
        assert_eq!(ring.recent(later, 10), vec![1]);
    }

    #[test]
    fn memory_does_not_grow_without_bound() {
        // Auch wenn alles innerhalb des Fensters liegt.
        let mut ring = ring();
        let now = Instant::now();
        for i in 0..(MAX_ENTRIES * 2) {
            ring.push(now, u32::try_from(i).unwrap_or(0));
        }
        assert_eq!(ring.len(), MAX_ENTRIES);
    }

    #[test]
    fn the_oldest_entries_are_dropped_first_when_full() {
        let mut ring = ring();
        let now = Instant::now();
        for i in 0..(MAX_ENTRIES + 10) {
            ring.push(now, u32::try_from(i).unwrap_or(0));
        }
        let newest = ring.recent(now, 1);
        assert_eq!(newest.first().copied(), u32::try_from(MAX_ENTRIES + 9).ok());
    }

    #[test]
    fn an_empty_ring_reports_nothing() {
        let mut ring = ring();
        assert!(ring.is_empty());
        assert!(ring.recent(Instant::now(), 10).is_empty());
    }
}
