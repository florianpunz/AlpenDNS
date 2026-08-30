//! Befristete Freigaben — und seit Phase 8 auch befristete Sperren.
//!
//! Der praktische Fall: eine Seite ist geblockt, jemand braucht sie *jetzt*, und
//! niemand will dafür eine Liste bearbeiten und den Dienst neu laden. Eine
//! Freigabe gilt für eine Weile und verschwindet dann von selbst — das ist der
//! Unterschied zu einer Allowlist, die man anlegt und nie wieder aufräumt.
//!
//! Der Gegenpart kam mit den Heuristiken dazu: neben einer auffälligen Anfrage
//! steht in der Oberfläche ein Knopf zum Sperren. Beides ist dieselbe Struktur
//! mit derselben Frist — sie zweimal zu schreiben wäre die Sorte Verdopplung,
//! bei der die eine Hälfte später anders aufräumt als die andere.
//!
//! Die Frist läuft über [`Clock`] und damit über monotone Zeit: eine
//! verstellte Systemuhr verlängert keine Freigabe.

use std::collections::HashMap;
use std::sync::{Mutex, PoisonError};
use std::time::{Duration, Instant};

use crate::clock::Clock;

/// Einträge, die von selbst ablaufen. Zweimal benutzt: für Freigaben und für
/// Sperren.
#[derive(Debug)]
pub struct Temporary<C> {
    entries: Mutex<HashMap<String, Instant>>,
    clock: C,
}

impl<C: Clock> Temporary<C> {
    pub fn new(clock: C) -> Self {
        Self {
            entries: Mutex::new(HashMap::new()),
            clock,
        }
    }

    /// Trägt eine Domain und alles darunter für `ttl` ein.
    ///
    /// Subdomains gelten mit: wer eine Seite freigibt, meint auch die Adressen,
    /// von denen sie ihre Bilder lädt. Eine Freigabe nur für den exakten Namen
    /// wäre in der Praxis nutzlos.
    pub fn grant(&self, domain: &str, ttl: Duration) {
        let Some(until) = self.clock.now().checked_add(ttl) else {
            return;
        };
        let mut entries = self.lock();
        entries.insert(normalize(domain), until);
        // Beim Eintragen gleich aufräumen, damit die Tabelle nicht wächst.
        let now = self.clock.now();
        entries.retain(|_, expiry| *expiry > now);
    }

    pub fn revoke(&self, domain: &str) {
        self.lock().remove(&normalize(domain));
    }

    /// Prüft den Namen und alle übergeordneten Zonen.
    ///
    /// Gibt die Restlaufzeit zurück, damit der Trace sie nennen kann.
    pub fn check(&self, name: &str) -> Option<Duration> {
        let now = self.clock.now();
        let entries = self.lock();
        let normalized = normalize(name);
        let mut rest = normalized.as_str();
        loop {
            if let Some(until) = entries.get(rest)
                && *until > now
            {
                return Some(until.saturating_duration_since(now));
            }
            let (_, parent) = rest.split_once('.')?;
            rest = parent;
        }
    }

    /// Alle noch gültigen Freigaben mit ihrer Restlaufzeit.
    pub fn active(&self) -> Vec<(String, Duration)> {
        let now = self.clock.now();
        let mut found: Vec<(String, Duration)> = self
            .lock()
            .iter()
            .filter(|(_, until)| **until > now)
            .map(|(domain, until)| (domain.clone(), until.saturating_duration_since(now)))
            .collect();
        found.sort_by(|a, b| a.0.cmp(&b.0));
        found
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<String, Instant>> {
        self.entries.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

fn normalize(domain: &str) -> String {
    domain.trim().trim_matches('.').to_ascii_lowercase()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::clock::TestClock;
    use std::sync::Arc;

    fn allows() -> (Arc<TestClock>, Temporary<Arc<TestClock>>) {
        let clock = Arc::new(TestClock::new());
        let allows = Temporary::new(Arc::clone(&clock));
        (clock, allows)
    }

    #[test]
    fn a_grant_expires_after_its_ttl() {
        // Der Nachweis aus der Roadmap, Schritt 5.
        let (clock, allows) = allows();
        allows.grant("example.com", Duration::from_secs(60));
        assert!(
            allows.check("example.com").is_some(),
            "sofort danach erlaubt"
        );

        clock.advance(Duration::from_secs(59));
        assert!(
            allows.check("example.com").is_some(),
            "kurz vor Ablauf noch erlaubt"
        );

        clock.advance(Duration::from_secs(2));
        assert!(
            allows.check("example.com").is_none(),
            "nach Ablauf wieder geblockt"
        );
    }

    #[test]
    fn the_remaining_time_counts_down() {
        let (clock, allows) = allows();
        allows.grant("example.com", Duration::from_secs(60));
        clock.advance(Duration::from_secs(20));
        let rest = allows.check("example.com").expect("noch gültig");
        assert_eq!(rest, Duration::from_secs(40));
    }

    #[test]
    fn subdomains_are_covered() {
        let (_clock, allows) = allows();
        allows.grant("example.com", Duration::from_secs(60));
        assert!(allows.check("cdn.example.com").is_some());
        assert!(allows.check("a.b.example.com").is_some());
    }

    #[test]
    fn a_grant_never_covers_a_name_that_merely_ends_the_same() {
        let (_clock, allows) = allows();
        allows.grant("example.com", Duration::from_secs(60));
        assert!(allows.check("notexample.com").is_none());
        assert!(allows.check("example.com.evil.net").is_none());
    }

    #[test]
    fn grants_are_case_insensitive_and_dot_tolerant() {
        let (_clock, allows) = allows();
        allows.grant("  Example.COM.  ", Duration::from_secs(60));
        assert!(allows.check("EXAMPLE.com.").is_some());
    }

    #[test]
    fn a_grant_can_be_revoked_early() {
        let (_clock, allows) = allows();
        allows.grant("example.com", Duration::from_secs(600));
        allows.revoke("example.com");
        assert!(allows.check("example.com").is_none());
    }

    #[test]
    fn granting_again_extends_the_deadline() {
        let (clock, allows) = allows();
        allows.grant("example.com", Duration::from_secs(60));
        clock.advance(Duration::from_secs(50));
        allows.grant("example.com", Duration::from_secs(60));
        clock.advance(Duration::from_secs(20));
        assert!(
            allows.check("example.com").is_some(),
            "die Verlängerung griff nicht"
        );
    }

    #[test]
    fn expired_grants_do_not_pile_up() {
        let (clock, allows) = allows();
        for i in 0..100 {
            allows.grant(&format!("d{i}.example"), Duration::from_secs(10));
        }
        clock.advance(Duration::from_secs(11));
        // Der Eintrag räumt beim Setzen auf.
        allows.grant("noch.example", Duration::from_secs(10));
        assert_eq!(allows.active().len(), 1);
    }

    #[test]
    fn active_lists_what_is_still_valid() {
        let (clock, allows) = allows();
        allows.grant("b.example", Duration::from_secs(60));
        allows.grant("a.example", Duration::from_secs(60));
        clock.advance(Duration::from_secs(10));
        let active = allows.active();
        assert_eq!(active.len(), 2);
        assert_eq!(active.first().map(|(d, _)| d.as_str()), Some("a.example"));
        assert_eq!(
            active.first().map(|(_, r)| *r),
            Some(Duration::from_secs(50))
        );
    }
}
