//! Regex-Regeln pro Policy.
//!
//! **Zur Laufzeitbegrenzung:** die Regex-Engine dieses Projekts arbeitet mit
//! endlichen Automaten und ohne Backtracking. Ein Ausdruck wie `(a+)+$`, der
//! eine Backtracking-Engine exponentiell beschäftigt, läuft hier linear zur
//! Eingabelänge. Das ist keine Vorsichtsmaßnahme, sondern eine Eigenschaft der
//! Engine — die Alternative wäre gewesen, Laufzeiten von außen abzuschneiden und
//! Anfragen mitten in der Auswertung zu verwerfen.
//!
//! Begrenzt wird trotzdem, was sich begrenzen lässt: die Größe des übersetzten
//! Ausdrucks und die Länge des geprüften Namens.

use std::sync::Arc;

use regex::{Regex, RegexBuilder};

/// Obergrenze für den übersetzten Ausdruck.
///
/// Ein Muster darf nicht beliebig viel Speicher belegen, nur weil es beliebig
/// viele Zeichenklassen aufzählt. 1 MB reicht für alles, was ein Mensch von Hand
/// schreibt.
const COMPILED_SIZE_LIMIT: usize = 1024 * 1024;

/// Längster Name, der geprüft wird. Ein DNS-Name ist nie länger (RFC 1035).
const MAX_NAME_LEN: usize = 253;

#[derive(Debug, thiserror::Error)]
#[error("Regex '{pattern}' in Policy '{policy}' ist ungültig: {reason}")]
pub struct RegexError {
    pub policy: String,
    pub pattern: String,
    pub reason: String,
}

#[derive(Debug)]
pub struct Rule {
    pub pattern: Arc<str>,
    regex: Regex,
}

/// Die Regex-Regeln einer Policy.
#[derive(Debug, Default)]
pub struct RegexRules {
    rules: Vec<Rule>,
}

impl RegexRules {
    /// Übersetzt die Muster. Ein ungültiges Muster ist ein Konfigurationsfehler
    /// und verhindert den Start — nicht etwas, das zur Laufzeit auffällt.
    pub fn compile(policy: &str, patterns: &[String]) -> Result<Self, RegexError> {
        let mut rules = Vec::with_capacity(patterns.len());
        for pattern in patterns {
            let regex = RegexBuilder::new(pattern)
                .size_limit(COMPILED_SIZE_LIMIT)
                // Domainnamen sind case-insensitiv; alles andere wäre eine Falle.
                .case_insensitive(true)
                .build()
                .map_err(|error| RegexError {
                    policy: policy.to_owned(),
                    pattern: pattern.clone(),
                    reason: error.to_string(),
                })?;
            rules.push(Rule {
                pattern: Arc::from(pattern.as_str()),
                regex,
            });
        }
        Ok(Self { rules })
    }

    pub fn is_empty(&self) -> bool {
        self.rules.is_empty()
    }

    /// Erste zutreffende Regel, falls es eine gibt.
    pub fn matches(&self, name: &str) -> Option<&Rule> {
        if name.len() > MAX_NAME_LEN {
            return None;
        }
        self.rules.iter().find(|rule| rule.regex.is_match(name))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    fn rules(patterns: &[&str]) -> RegexRules {
        let owned: Vec<String> = patterns.iter().map(|p| (*p).to_owned()).collect();
        RegexRules::compile("test", &owned).expect("gültige Muster")
    }

    #[test]
    fn a_matching_name_is_found_with_its_pattern() {
        let rules = rules(&[r"^ads\.", r"tracking"]);
        let hit = rules.matches("ads.example.com").expect("Treffer");
        assert_eq!(&*hit.pattern, r"^ads\.");
        assert_eq!(
            rules.matches("x.tracking.example").map(|r| &*r.pattern),
            Some("tracking")
        );
    }

    #[test]
    fn a_name_without_a_match_returns_nothing() {
        assert!(rules(&[r"^ads\."]).matches("example.com").is_none());
    }

    #[test]
    fn matching_ignores_case() {
        assert!(rules(&[r"^ads\."]).matches("ADS.example.com").is_some());
    }

    #[test]
    fn an_invalid_pattern_is_a_configuration_error() {
        let error = RegexRules::compile("kids", &["(unbalanciert".to_owned()])
            .expect_err("muss abgelehnt werden");
        assert_eq!(error.policy, "kids");
        assert!(error.reason.contains("regex"), "{}", error.reason);
    }

    #[test]
    fn an_absurdly_large_pattern_is_rejected() {
        // Statt beim ersten Treffer den Speicher zu füllen.
        let huge = format!("(?:{})", vec!["[a-z0-9]{100}"; 2000].join("|"));
        assert!(RegexRules::compile("test", &[huge]).is_err());
    }

    #[test]
    fn a_pattern_that_kills_backtracking_engines_runs_in_no_time() {
        // Der Nachweis aus der Roadmap, Schritt 6. In einer Backtracking-Engine
        // läuft das exponentiell; hier linear.
        let rules = rules(&[r"^(a+)+$"]);
        let input = format!("{}b", "a".repeat(10_000));

        let started = Instant::now();
        let found = rules.matches(&input);
        let elapsed = started.elapsed();

        assert!(found.is_none());
        assert!(
            elapsed < Duration::from_millis(50),
            "Auswertung brauchte {elapsed:?}"
        );
    }

    #[test]
    fn several_nested_quantifiers_stay_harmless() {
        let rules = rules(&[r"^(x+x+)+y$", r"(a|aa)+$", r"^(\w+\s?)*$"]);
        let input = "x".repeat(5_000);
        let started = Instant::now();
        let _ = rules.matches(&input);
        assert!(started.elapsed() < Duration::from_millis(50));
    }

    #[test]
    fn an_overlong_name_is_not_matched_at_all() {
        let rules = rules(&[r"a"]);
        assert!(rules.matches(&"a".repeat(MAX_NAME_LEN + 1)).is_none());
        assert!(rules.matches(&"a".repeat(MAX_NAME_LEN)).is_some());
    }

    #[test]
    fn no_patterns_means_no_matches() {
        let rules = RegexRules::default();
        assert!(rules.is_empty());
        assert!(rules.matches("example.com").is_none());
    }
}
