//! Nachschlagen, ob ein Name auf einer Liste steht.
//!
//! Der Matcher liefert bei einem Treffer eine [`RuleRef`] und nicht `true`.
//! Das ist keine Bequemlichkeit: ohne die Herkunft einer Regel ist "warum wurde
//! das geblockt?" nicht beantwortbar, und genau diese Frage ist der Grund für
//! dieses Projekt (CLAUDE.md B.3).
//!
//! **Datenstruktur:** eine `HashMap` mit Suffix-Nachschlag, wie in der Roadmap
//! für v1 vorgesehen. Für `a.b.example.com` werden bis zu vier Schlüssel
//! probiert. Bloom-Filter und invertierter Trie (ARCHITECTURE.md §3) kommen
//! erst, wenn der Benchmark zeigt, dass es nötig ist — erst messen, dann
//! optimieren.

use std::collections::HashMap;

use super::parser::{Entry, Parsed, Scope};

/// Welche Liste eine Regel geliefert hat.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ListId(pub u16);

/// Verweis auf die Regel, die zugeschlagen hat.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RuleRef {
    pub list: ListId,
    /// Zeilennummer in der Quelldatei, 1-basiert.
    pub line: u32,
}

/// Ein Treffer, mit allem, was für eine Begründung nötig ist.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Match {
    pub rule: RuleRef,
    /// Der Eintrag, der zugeschlagen hat — nicht der angefragte Name.
    /// Bei `ads.example.com` gegen die Wildcard `example.com` steht hier
    /// `example.com`.
    pub matched: String,
    pub scope: Scope,
}

#[derive(Debug, Clone, Copy)]
struct Rule {
    reference: RuleRef,
    scope: Scope,
}

/// Zusammengefasste Listen, bereit zum Nachschlagen.
#[derive(Debug, Default)]
pub struct Matcher {
    entries: HashMap<Box<str>, Rule>,
    names: Vec<String>,
}

/// Baut einen [`Matcher`] aus mehreren geparsten Listen.
#[derive(Debug, Default)]
pub struct Builder {
    entries: HashMap<Box<str>, Rule>,
    names: Vec<String>,
}

impl Builder {
    pub fn new() -> Self {
        Self::default()
    }

    /// Nimmt eine geparste Liste auf und gibt ihre ID zurück.
    ///
    /// Kommt eine Domain mehrfach vor, gewinnt der **breitere** Geltungsbereich:
    /// eine Wildcard schlägt einen exakten Eintrag, weil sie ohnehin alles
    /// abdeckt, was der exakte abdeckt. Bei gleichem Geltungsbereich gewinnt das
    /// erste Vorkommen — so zeigt die Begründung auf die Zeile, die tatsächlich
    /// zuerst da war.
    pub fn add(&mut self, list_name: &str, parsed: &Parsed) -> ListId {
        let id = ListId(u16::try_from(self.names.len()).unwrap_or(u16::MAX));
        self.names.push(list_name.to_owned());

        for Entry {
            domain,
            scope,
            line,
        } in &parsed.entries
        {
            let rule = Rule {
                reference: RuleRef {
                    list: id,
                    line: *line,
                },
                scope: *scope,
            };
            match self.entries.get(domain.as_str()) {
                Some(existing) if existing.scope == Scope::Suffix || *scope == Scope::Exact => {
                    // Vorhandener ist schon so breit wie möglich, oder der neue
                    // ist enger: nichts zu tun.
                }
                _ => {
                    self.entries.insert(domain.as_str().into(), rule);
                }
            }
        }
        id
    }

    pub fn build(self) -> Matcher {
        Matcher {
            entries: self.entries,
            names: self.names,
        }
    }
}

impl Matcher {
    /// Anzahl eindeutiger Domains.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Name der Liste hinter einer Regel.
    pub fn list_name(&self, id: ListId) -> Option<&str> {
        self.names.get(usize::from(id.0)).map(String::as_str)
    }

    /// Sucht den Namen und alle seine übergeordneten Zonen.
    ///
    /// `name` muss normalisiert sein (klein, ohne Punkt am Ende) — dafür gibt es
    /// [`super::parser::normalize`].
    ///
    /// Der spezifischste Treffer gewinnt: steht `ads.example.com` exakt auf einer
    /// Liste und `example.com` als Wildcard auf einer anderen, wird der exakte
    /// gemeldet, weil er die genauere Begründung ist.
    pub fn lookup(&self, name: &str) -> Option<Match> {
        let mut rest = name;
        let mut depth = 0_usize;
        loop {
            if let Some(rule) = self.entries.get(rest) {
                // Auf der ersten Ebene zählt jeder Eintrag, darüber nur
                // Wildcards: `notexample.com` darf nie wegen `example.com`
                // treffen, und `example.com` exakt gilt nicht für Subdomains.
                if depth == 0 || rule.scope == Scope::Suffix {
                    return Some(Match {
                        rule: rule.reference,
                        matched: rest.to_owned(),
                        scope: rule.scope,
                    });
                }
            }
            let (_, parent) = rest.split_once('.')?;
            rest = parent;
            depth = depth.saturating_add(1);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::filter::parser::{Format, parse};

    fn matcher_from(entries: &[(&str, Format)]) -> Matcher {
        let mut builder = Builder::new();
        for (index, (text, format)) in entries.iter().enumerate() {
            builder.add(&format!("liste{index}"), &parse(text, *format));
        }
        builder.build()
    }

    #[test]
    fn an_exact_entry_matches_only_itself() {
        let matcher = matcher_from(&[("ads.example.com\n", Format::Domains)]);
        assert!(matcher.lookup("ads.example.com").is_some());
        assert!(matcher.lookup("sub.ads.example.com").is_none());
        assert!(matcher.lookup("example.com").is_none());
    }

    #[test]
    fn a_wildcard_entry_matches_the_domain_and_everything_below() {
        let matcher = matcher_from(&[("example.com\n", Format::Wildcard)]);
        for name in ["example.com", "www.example.com", "a.b.c.example.com"] {
            assert!(matcher.lookup(name).is_some(), "{name} traf nicht");
        }
    }

    #[test]
    fn a_wildcard_never_matches_a_name_that_merely_ends_in_the_same_letters() {
        // Der klassische Fehler: `notexample.com` als Teilstring-Treffer von
        // `example.com`.
        let matcher = matcher_from(&[("example.com\n", Format::Wildcard)]);
        for name in ["notexample.com", "badexample.com", "example.com.evil.net"] {
            assert!(matcher.lookup(name).is_none(), "{name} traf fälschlich");
        }
    }

    #[test]
    fn a_hit_names_its_list_and_line() {
        let mut builder = Builder::new();
        let id = builder.add(
            "stevenblack",
            &parse("# Kopf\n0.0.0.0 ads.example.com\n", Format::Hosts),
        );
        let matcher = builder.build();

        let hit = matcher.lookup("ads.example.com").expect("Treffer");
        assert_eq!(hit.rule.list, id);
        assert_eq!(hit.rule.line, 2);
        assert_eq!(hit.matched, "ads.example.com");
        assert_eq!(matcher.list_name(id), Some("stevenblack"));
    }

    #[test]
    fn a_wildcard_hit_reports_the_entry_not_the_query() {
        let matcher = matcher_from(&[("example.com\n", Format::Wildcard)]);
        let hit = matcher
            .lookup("tief.verschachtelt.example.com")
            .expect("Treffer");
        assert_eq!(hit.matched, "example.com");
        assert_eq!(hit.scope, Scope::Suffix);
    }

    #[test]
    fn the_more_specific_entry_wins() {
        let matcher = matcher_from(&[
            ("example.com\n", Format::Wildcard),
            ("ads.example.com\n", Format::Domains),
        ]);
        let hit = matcher.lookup("ads.example.com").expect("Treffer");
        assert_eq!(
            hit.matched, "ads.example.com",
            "der breitere Eintrag gewann"
        );
    }

    #[test]
    fn a_wildcard_beats_an_exact_entry_for_the_same_domain() {
        // Sonst würde ein exakter Eintrag eine Wildcard derselben Domain
        // verdecken und Subdomains kämen durch.
        let matcher = matcher_from(&[
            ("example.com\n", Format::Domains),
            ("example.com\n", Format::Wildcard),
        ]);
        assert!(matcher.lookup("www.example.com").is_some());
    }

    #[test]
    fn order_does_not_matter_for_scope_precedence() {
        let matcher = matcher_from(&[
            ("example.com\n", Format::Wildcard),
            ("example.com\n", Format::Domains),
        ]);
        assert!(matcher.lookup("www.example.com").is_some());
    }

    #[test]
    fn the_first_occurrence_wins_within_the_same_scope() {
        let mut builder = Builder::new();
        builder.add("erste", &parse("example.com\n", Format::Domains));
        builder.add("zweite", &parse("\n\n\nexample.com\n", Format::Domains));
        let matcher = builder.build();
        assert_eq!(matcher.lookup("example.com").expect("Treffer").rule.line, 1);
    }

    #[test]
    fn an_empty_matcher_matches_nothing() {
        let matcher = Matcher::default();
        assert!(matcher.is_empty());
        assert!(matcher.lookup("example.com").is_none());
    }

    #[test]
    fn a_single_label_name_is_handled() {
        let matcher = matcher_from(&[("localhost\n", Format::Wildcard)]);
        assert!(matcher.lookup("localhost").is_some());
        assert!(matcher.lookup("etwas.anderes").is_none());
    }

    proptest::proptest! {
        /// Die Invariante aus docs/TESTING.md §2, in ihrer genauen Form: eine
        /// Wildcard trifft einen Namen **genau dann**, wenn er der Eintrag
        /// selbst ist oder auf `.eintrag` endet.
        ///
        /// Die naheliegende Formulierung "`{eintrag}.{suffix}` trifft nie" wäre
        /// falsch: bei Eintrag `r` und Suffix `r` ergibt das `r.r`, und das
        /// *ist* eine Subdomain von `r`. Proptest hat genau diesen Fall gefunden.
        #[test]
        fn wildcard_matching_is_exactly_label_suffix_matching(
            labels in proptest::collection::vec("[a-z]{1,10}", 1..5),
            other in proptest::collection::vec("[a-z]{1,10}", 1..5),
        ) {
            let blocked = labels.join(".");
            let matcher = matcher_from(&[(&format!("{blocked}\n"), Format::Wildcard)]);
            let suffix = other.join(".");

            let candidates = [
                blocked.clone(),
                format!("{suffix}.{blocked}"),
                format!("{suffix}{blocked}"),
                format!("{blocked}.{suffix}"),
                format!("{blocked}{suffix}"),
                suffix.clone(),
            ];
            for candidate in candidates {
                let expected = candidate == blocked
                    || candidate.ends_with(&format!(".{blocked}"));
                proptest::prop_assert_eq!(
                    matcher.lookup(&candidate).is_some(),
                    expected,
                    "{} sollte {}treffen",
                    candidate,
                    if expected { "" } else { "nicht " }
                );
            }
        }
    }
}
