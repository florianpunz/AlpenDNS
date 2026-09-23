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

/// Der Hinweis, den eine DNSSEC-Prüfung ohne Zeitsynchronisation verdient.
///
/// `None`, wenn DNSSEC aus ist — dann hängt keine Entscheidung an der Uhr — oder
/// wenn das Merkmal der Zeitsynchronisation liegt. Beide Eingaben sind
/// Parameter, damit der Test erfundene hineingeben kann; `check` gibt die echten
/// hinein.
///
/// **Der Grund, warum das überhaupt zählt:** die Signaturkette wird gegen die
/// Systemuhr gerechnet (ADR-0016), und zwar nicht in diesem Programm, sondern in
/// `hickory-net` (`dnssec::mod`, `ExpiredRrsig`): dort steht ein `SystemTime::now()`
/// gegen das Gültigkeitsfenster jeder Signatur. Steht die Uhr auf dem
/// Epoch-Datum, ist `current_time >= sig_inception` für **jede** signierte Zone
/// falsch — jede Antwort wird bogus, jede Anfrage SERVFAIL, und der Upstream
/// zählt dabei keinen einzigen Fehlversuch (so gewollt, siehe `pool.rs`). Die
/// Oberfläche zeigt also gesunde Upstreams und kein einziges Ergebnis.
///
/// **Ein Hinweis, kein Fehler.** `alpendns check` läuft als `ExecStartPre`. Ob
/// die Uhr falsch steht, weiß dieses Programm nicht — es weiß nur, dass niemand
/// sie gestellt hat, und das kann bei `chrony` oder `ntpd` auch ohne diese Datei
/// in Ordnung sein. Ein Resolver, der deswegen nicht startet, wäre die
/// schlechtere Antwort.
pub fn time_sync_hint(dnssec: bool, marker: &std::path::Path) -> Option<String> {
    if !dnssec || marker.exists() {
        return None;
    }
    // Umbrochen auf die Länge des echten Pfades (34 Zeichen), damit die Zeilen
    // samt "Hinweis: " auf einem 80er-Terminal stehen bleiben.
    Some(format!(
        "DNSSEC ist an (privacy.dnssec), aber die Systemuhr ist nicht \n\
         synchronisiert: {} existiert nicht. \n\
         Die Signaturkette wird gegen die Systemuhr gerechnet (ADR-0016) — steht \n\
         sie falsch, ist jede signierte Zone bogus und jede Anfrage SERVFAIL. \n\
         Läuft hier chrony oder ntpd statt systemd-timesyncd, kann der Hinweis \n\
         falsch sein; timedatectl zeigt, wie es steht.",
        marker.display()
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_time_sync_hint_appears_only_with_dnssec_and_no_marker() {
        let dir = std::env::temp_dir().join(format!("alpendns-clock-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("Testverzeichnis");
        let missing = dir.join("gibt-es-nicht");
        let missing_path = missing.display().to_string();
        let _ = std::fs::remove_file(&missing);

        // Ohne DNSSEC hängt nichts an der Uhr: kein Hinweis, auch ohne Merkmal.
        assert!(time_sync_hint(false, &missing).is_none());

        // Mit DNSSEC und ohne Merkmal: der Hinweis nennt den gesuchten Pfad.
        let hint = time_sync_hint(true, &missing).expect("Hinweis");
        assert!(
            hint.contains(&missing_path),
            "der Hinweis nennt {missing_path} nicht: {hint}"
        );

        // Mit Merkmal: die Uhr wurde gestellt, es gibt nichts zu sagen.
        let present = dir.join("synchronized");
        std::fs::write(&present, b"").expect("Merkmal");
        assert!(time_sync_hint(true, &present).is_none());

        let _ = std::fs::remove_dir_all(&dir);
    }

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
