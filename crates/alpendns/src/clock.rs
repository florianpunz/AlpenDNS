//! Zeitquelle als Trait, damit sie in Tests kontrollierbar ist.
//!
//! Ohne das wären alle TTL- und Zeitplan-Tests `sleep`-basiert: langsam, und bei
//! Last auf dem Testrechner unzuverlässig. Deshalb ruft im Cache und später in
//! der Policy niemand direkt `Instant::now()` auf.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

/// Liefert die aktuelle monotone Zeit.
pub trait Clock: Send + Sync + 'static {
    fn now(&self) -> Instant;
}

/// Die echte Uhr. Im Betrieb die einzige Implementierung.
#[derive(Debug, Clone, Copy, Default)]
pub struct SystemClock;

impl Clock for SystemClock {
    fn now(&self) -> Instant {
        Instant::now()
    }
}

/// Damit ein Test dieselbe Uhr behalten kann, die er in den Cache gibt.
impl<C: Clock> Clock for Arc<C> {
    fn now(&self) -> Instant {
        (**self).now()
    }
}

/// Liefert die aktuelle Ortszeit mit Datum und Wochentag.
///
/// Getrennt von [`Clock`], weil beide verschiedene Dinge sind: `Clock` liefert
/// monotone Zeit für Fristen (TTL, Timeouts) und darf nie rückwärts laufen;
/// hier geht es um Kalenderzeit für Zeitpläne, die sehr wohl springt — bei
/// Sommerzeitwechseln und wenn jemand die Systemuhr stellt.
pub trait WallClock: Send + Sync + 'static {
    fn now(&self) -> jiff::Zoned;
}

/// Die Ortszeit des Systems.
#[derive(Debug, Clone, Copy, Default)]
pub struct SystemWallClock;

impl WallClock for SystemWallClock {
    fn now(&self) -> jiff::Zoned {
        jiff::Zoned::now()
    }
}

impl<C: WallClock> WallClock for Arc<C> {
    fn now(&self) -> jiff::Zoned {
        (**self).now()
    }
}

/// Kalenderuhr für Tests: steht auf einem gesetzten Zeitpunkt.
#[derive(Debug)]
pub struct FixedWallClock(std::sync::Mutex<jiff::Zoned>);

impl FixedWallClock {
    pub fn new(at: jiff::Zoned) -> Self {
        Self(std::sync::Mutex::new(at))
    }

    /// Stellt die Uhr auf einen anderen Zeitpunkt.
    pub fn set(&self, at: jiff::Zoned) {
        *self
            .0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = at;
    }
}

impl WallClock for FixedWallClock {
    fn now(&self) -> jiff::Zoned {
        self.0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }
}

/// Uhr, die stillsteht, bis sie von Hand vorgestellt wird.
///
/// Öffentlich, weil Integrationstests ein eigenes Crate sind und nicht auf
/// `#[cfg(test)]`-Typen zugreifen können.
#[derive(Debug)]
pub struct TestClock {
    base: Instant,
    offset_nanos: AtomicU64,
}

impl TestClock {
    pub fn new() -> Self {
        Self {
            base: Instant::now(),
            offset_nanos: AtomicU64::new(0),
        }
    }

    /// Stellt die Uhr vor.
    pub fn advance(&self, by: Duration) {
        let nanos = u64::try_from(by.as_nanos()).unwrap_or(u64::MAX);
        self.offset_nanos.fetch_add(nanos, Ordering::SeqCst);
    }
}

impl Default for TestClock {
    fn default() -> Self {
        Self::new()
    }
}

impl Clock for TestClock {
    fn now(&self) -> Instant {
        let offset = Duration::from_nanos(self.offset_nanos.load(Ordering::SeqCst));
        // Ein Überlauf ist nur mit absurden Testwerten erreichbar; dann bleibt
        // die Uhr stehen, statt zu panicken.
        self.base.checked_add(offset).unwrap_or(self.base)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_clock_stands_still_until_advanced() {
        let clock = TestClock::new();
        let first = clock.now();
        assert_eq!(clock.now(), first, "Uhr läuft von allein");

        clock.advance(Duration::from_secs(30));
        assert_eq!(clock.now().duration_since(first), Duration::from_secs(30));
    }

    #[test]
    fn advances_are_cumulative() {
        let clock = TestClock::new();
        let start = clock.now();
        clock.advance(Duration::from_secs(10));
        clock.advance(Duration::from_secs(5));
        assert_eq!(clock.now().duration_since(start), Duration::from_secs(15));
    }

    #[test]
    fn system_clock_moves_forward() {
        let clock = SystemClock;
        assert!(clock.now() <= clock.now());
    }
}
