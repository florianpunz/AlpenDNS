//! Der Decision-Trace: warum eine Anfrage so beantwortet wurde, wie sie
//! beantwortet wurde.
//!
//! Das ist das architektonisch wichtigste Detail des Projekts
//! (ARCHITECTURE.md §2). Der Trace entsteht **immer**, unabhängig vom Log-Modus;
//! erst die Logging-Schicht entscheidet, was mit ihm passiert. Deshalb darf er
//! Query-Namen enthalten — und deshalb darf ihn niemand außerhalb dieser
//! Schicht einfach ins Log schreiben (B.1 Regel 3).
//!
//! **Der Trace wird exklusiv durchgereicht** (`&mut Ctx`). Bis `fanout` entfiel,
//! lag er hinter einem `Mutex`: der Pool fragte mehrere Resolver gleichzeitig,
//! und jeder wollte eintragen. Seit immer genau ein Upstream gefragt wird, ist
//! die Pipeline eine Kette ohne Verzweigung, und der `Mutex` schützte nichts
//! mehr ([ADR-0012](../../../docs/adr/0012-fanout-entfaellt.md)).

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::{Duration, Instant};

use crate::filter::block::BlockMode;

/// Woran ein Client erkannt wurde.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MatchKind {
    /// Quell-IP oder Subnetz.
    Address,
    /// Kein Eintrag hat gepasst, es gilt die Default-Policy.
    Default,
}

/// Was ein Zeitplan angeordnet hat.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScheduleEffect {
    /// Innerhalb des Fensters: alles außer der Allowlist wird geblockt.
    BlockAllExceptAllowlist,
}

/// Ein Schritt auf dem Weg zur Antwort.
///
/// Namen sind als `Arc<str>` eingebettet statt als IDs mit Nachschlagetabelle:
/// ein Trace soll für sich allein lesbar sein, ohne die Konfiguration daneben.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Step {
    ClientMatched {
        client: Arc<str>,
        by: MatchKind,
    },
    PolicyApplied {
        policy: Arc<str>,
    },
    /// Eine befristete Freigabe hat gegriffen.
    TemporaryAllow {
        remaining: Duration,
    },
    /// Eine befristete Sperre hat gegriffen — der Gegenpart, gesetzt über die
    /// API mit einem Klick auf eine auffällige Anfrage (Phase 8, Schritt 7).
    TemporaryDeny {
        remaining: Duration,
    },
    AllowlistHit {
        list: Arc<str>,
        line: u32,
        /// Der Eintrag, der zutraf — bei einer Wildcard nicht der gefragte Name.
        matched: String,
    },
    BlocklistHit {
        list: Arc<str>,
        line: u32,
        matched: String,
    },
    RegexHit {
        policy: Arc<str>,
        pattern: Arc<str>,
    },
    ScheduleHit {
        schedule: Arc<str>,
        effect: ScheduleEffect,
    },
    CacheHit {
        ttl_left: u32,
        stale: bool,
    },
    UpstreamUsed {
        resolver: Arc<str>,
        rtt: Duration,
    },
    /// Die Signaturkette wurde selbst nachgerechnet (Phase 7, Schritt 3).
    DnssecChecked {
        verdict: crate::dnssec::Verdict,
    },
    /// Eine Heuristik hat angeschlagen (Phase 8).
    ///
    /// Steht auch dann im Trace, wenn die Aktion nur `log` oder `flag` ist —
    /// der Trace bildet ab, was passiert ist, nicht nur was entschieden wurde.
    /// Die Begründung ist Pflicht: ohne sie ist ein Fehlalarm nicht debugbar
    /// (FEATURES.md D6).
    Detected {
        detector: crate::detect::Detector,
        score: crate::detect::Permille,
        reason: Arc<str>,
        action: crate::detect::Action,
    },
    Synthesized {
        mode: BlockMode,
    },
}

impl std::fmt::Display for Step {
    /// Eine Zeile pro Schritt, so wie `alpendns policy test` sie ausgibt.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::ClientMatched { client, by } => match by {
                MatchKind::Address => write!(f, "Client '{client}' recognized by source address"),
                MatchKind::Default => write!(f, "no client entry matches, '{client}' applies"),
            },
            Self::PolicyApplied { policy } => write!(f, "Policy '{policy}'"),
            Self::TemporaryAllow { remaining } => {
                write!(f, "temporary allow, {remaining:.0?} left")
            }
            Self::TemporaryDeny { remaining } => {
                write!(f, "temporary deny, {remaining:.0?} left")
            }
            Self::AllowlistHit {
                list,
                line,
                matched,
            } => write!(f, "Allowlist '{list}' line {line}: '{matched}'"),
            Self::BlocklistHit {
                list,
                line,
                matched,
            } => write!(f, "Blocklist '{list}' line {line}: '{matched}'"),
            Self::RegexHit { policy, pattern } => {
                write!(f, "Regex rule from policy '{policy}': /{pattern}/")
            }
            Self::ScheduleHit { schedule, effect } => match effect {
                ScheduleEffect::BlockAllExceptAllowlist => write!(
                    f,
                    "Schedule '{schedule}' active: everything except the allowlist is blocked"
                ),
            },
            Self::CacheHit { ttl_left, stale } => {
                let label = if *stale { "expired" } else { "valid" };
                write!(f, "from cache ({label}, {ttl_left} s left)")
            }
            Self::UpstreamUsed { resolver, rtt } => {
                write!(f, "Upstream '{resolver}' answered in {rtt:.1?}")
            }
            Self::DnssecChecked { verdict } => write!(f, "DNSSEC verified locally: {verdict}"),
            Self::Detected {
                detector,
                score,
                reason,
                action,
            } => {
                let verb = match action {
                    crate::detect::Action::Block => "blocks",
                    crate::detect::Action::Flag => "flags",
                    crate::detect::Action::Log | crate::detect::Action::Off => "logs",
                };
                write!(
                    f,
                    "{detector} {verb} (score {}): {reason}",
                    crate::detect::format_score(*score)
                )
            }
            Self::Synthesized { mode } => write!(f, "Answer synthesized locally, mode {mode:?}"),
        }
    }
}

/// Kontext einer einzelnen Anfrage.
///
/// Wandert exklusiv durch alle Schichten der Pipeline. Jede Schicht reicht ihn
/// an die nächste weiter; nebenläufig trägt niemand ein.
#[derive(Debug)]
pub struct Ctx {
    /// Woher die Anfrage kam. Grundlage der Client-Identifikation.
    pub peer: SocketAddr,
    started: Instant,
    steps: Vec<Step>,
}

impl Ctx {
    pub fn new(peer: SocketAddr) -> Self {
        Self {
            peer,
            started: Instant::now(),
            // Acht Schritte decken den Normalfall ohne Nachallokieren ab.
            steps: Vec::with_capacity(8),
        }
    }

    /// Kontext ohne echten Client — für Hintergrundaufgaben wie Prefetch, die
    /// keine Anfrage eines Clients sind.
    pub fn internal() -> Self {
        Self::new(SocketAddr::from(([127, 0, 0, 1], 0)))
    }

    pub fn record(&mut self, step: Step) {
        self.steps.push(step);
    }

    pub fn steps(&self) -> &[Step] {
        &self.steps
    }

    pub fn elapsed(&self) -> Duration {
        self.started.elapsed()
    }

    /// Die Begründungskette als Text, ein Schritt pro Zeile.
    pub fn explain(&self) -> String {
        self.steps
            .iter()
            .enumerate()
            .map(|(index, step)| format!("  {}. {step}", index.saturating_add(1)))
            .collect::<Vec<_>>()
            .join("\n")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ctx() -> Ctx {
        Ctx::new(SocketAddr::from(([10, 0, 0, 5], 1234)))
    }

    #[test]
    fn steps_are_kept_in_order() {
        let mut ctx = ctx();
        ctx.record(Step::ClientMatched {
            client: Arc::from("laptop"),
            by: MatchKind::Address,
        });
        ctx.record(Step::PolicyApplied {
            policy: Arc::from("default"),
        });
        let steps = ctx.steps();
        assert_eq!(steps.len(), 2);
        assert!(matches!(steps.first(), Some(Step::ClientMatched { .. })));
        assert!(matches!(steps.get(1), Some(Step::PolicyApplied { .. })));
    }

    #[test]
    fn a_fresh_context_has_no_steps() {
        assert!(ctx().steps().is_empty());
        assert_eq!(ctx().explain(), "");
    }

    #[test]
    fn the_explanation_numbers_every_step() {
        let mut ctx = ctx();
        ctx.record(Step::ClientMatched {
            client: Arc::from("kids-tablet"),
            by: MatchKind::Address,
        });
        ctx.record(Step::BlocklistHit {
            list: Arc::from("stevenblack"),
            line: 42,
            matched: "ads.example.com".to_owned(),
        });
        let text = ctx.explain();
        assert!(text.contains("1. Client 'kids-tablet'"), "{text}");
        assert!(
            text.contains("2. Blocklist 'stevenblack' line 42"),
            "{text}"
        );
    }

    #[test]
    fn every_step_renders_without_panicking() {
        let steps = [
            Step::ClientMatched {
                client: Arc::from("c"),
                by: MatchKind::Default,
            },
            Step::PolicyApplied {
                policy: Arc::from("p"),
            },
            Step::TemporaryAllow {
                remaining: Duration::from_secs(42),
            },
            Step::TemporaryDeny {
                remaining: Duration::from_secs(42),
            },
            Step::AllowlistHit {
                list: Arc::from("a"),
                line: 1,
                matched: "x.example".to_owned(),
            },
            Step::BlocklistHit {
                list: Arc::from("b"),
                line: 2,
                matched: "y.example".to_owned(),
            },
            Step::RegexHit {
                policy: Arc::from("p"),
                pattern: Arc::from("^ads"),
            },
            Step::ScheduleHit {
                schedule: Arc::from("bedtime"),
                effect: ScheduleEffect::BlockAllExceptAllowlist,
            },
            Step::CacheHit {
                ttl_left: 30,
                stale: true,
            },
            Step::UpstreamUsed {
                resolver: Arc::from("quad9"),
                rtt: Duration::from_millis(12),
            },
            Step::DnssecChecked {
                verdict: crate::dnssec::Verdict::Secure,
            },
            Step::Detected {
                detector: crate::detect::Detector::Dga,
                score: 900,
                reason: Arc::from("Testbegründung"),
                action: crate::detect::Action::Flag,
            },
            Step::Synthesized {
                mode: BlockMode::Nxdomain,
            },
        ];
        for step in steps {
            assert!(!step.to_string().is_empty(), "{step:?}");
        }
    }
}
