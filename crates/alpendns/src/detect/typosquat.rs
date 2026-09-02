//! Typosquatting relativ zu *deinen* Domains.
//!
//! Der Unterschied zu allem, was es sonst gibt (FEATURES.md D4): übliche
//! Lösungen prüfen gegen globale Phishing-Listen, also gegen das, was gestern
//! schon gemeldet war. Hier läuft der Vergleich gegen die zwanzig Domains, die
//! dir wichtig sind — Bank, Behördenportal, Arbeitgeber. Dadurch fällt auch
//! eine Domain auf, die vor zehn Minuten registriert wurde und auf keiner Liste
//! steht.
//!
//! Der Rechenaufwand ist trivial, *weil* die Schutzliste klein ist. Bei
//! zwanzig Einträgen sind es zwanzig Distanzberechnungen über je ein Dutzend
//! Zeichen. Gegen eine Million Domains wäre derselbe Ansatz unbrauchbar — das
//! ist keine Schwäche der Umsetzung, sondern der Grund, warum das Feature so
//! zugeschnitten ist.
//!
//! # Drei Arten von Treffer
//!
//! | Art | Beispiel gegen `sparkasse.at` | Score |
//! |---|---|---|
//! | Homograph | `spаrkasse.at` (kyrillisches `а`) | 1.000 |
//! | Fremder Name trägt sie | `sparkasse.at.com`, `sparkasse.at.login.example` | 0.950 |
//! | Tippfehler, Abstand 1 | `sparkase.at`, `sparkassse.at` | 0.950 |
//! | Tippfehler, Abstand 2 | `sparkkasee.at` | 0.850 |
//! | Andere Endung | `sparkasse.com` | 0.900 |
//!
//! Der Homograph steht oben, weil er kein Tippfehler ist: niemand vertippt
//! sich in ein kyrillisches Zeichen. Ein Treffer dort ist immer Absicht.

use std::collections::BTreeMap;

use super::{Detector, Finding, NameDetector, Observation};

/// Wie weit ein Name von einer geschützten Domain entfernt sein darf, um noch
/// als Verwechslung zu gelten.
///
/// Zwei ist die Obergrenze aus FEATURES.md D4. Drei wäre bei kurzen Namen
/// schon fast jede andere Domain: von `orf.at` nach `ard.at` sind es zwei.
const MAX_DISTANCE: usize = 2;

/// Kürzere Namen als dieser werden nicht verglichen.
///
/// Bei drei Zeichen ist ein Abstand von zwei die halbe Domain. `t.co` gegen
/// `x.co` wäre ein Treffer und nichts weiter als zwei verschiedene Firmen.
const MIN_LENGTH: usize = 5;

/// Prüft Namen gegen eine kleine Liste geschützter Domains.
#[derive(Debug)]
pub struct Typosquat {
    /// Geschützte Domains, klein und ohne abschließenden Punkt.
    protect: Vec<Protected>,
}

#[derive(Debug)]
struct Protected {
    /// Wie sie konfiguriert wurde, für die Begründung.
    domain: String,
    /// Der Teil vor der Endung, etwa `sparkasse` aus `sparkasse.at`.
    label: String,
    /// Die Endung, etwa `at`.
    suffix: String,
    /// Der Name mit allen Verwechselbaren auf ihr lateinisches Gegenstück
    /// abgebildet. Zwei Namen mit demselben Skelett sehen gleich aus.
    skeleton: String,
}

impl Typosquat {
    pub fn new(protect: &[String]) -> Self {
        Self {
            protect: protect
                .iter()
                .filter_map(|entry| {
                    let domain = normalize(entry);
                    let (label, suffix) = domain.split_once('.')?;
                    Some(Protected {
                        label: label.to_owned(),
                        suffix: suffix.to_owned(),
                        skeleton: skeleton(&domain),
                        domain,
                    })
                })
                .collect(),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.protect.is_empty()
    }
}

/// Klein, ohne abschließenden Punkt, Punycode aufgelöst.
///
/// Das Auflösen ist der Kern der Homographen-Erkennung: ein Name mit einem
/// kyrillischen Zeichen erreicht uns als `xn--sprkasse-...`, und in dieser Form
/// sieht er `sparkasse` nicht im Geringsten ähnlich. Erst nach dem Dekodieren
/// steht das verwechselbare Zeichen da, wo es hingehört.
fn normalize(name: &str) -> String {
    let trimmed = name.trim().trim_end_matches('.').to_lowercase();
    if !trimmed.contains("xn--") {
        return trimmed;
    }
    // `to_unicode` gibt bei kaputter Kodierung eine Ersatzform zurück statt zu
    // scheitern; das ist hier genau richtig — ein Name, den wir nicht
    // dekodieren können, soll den Detektor nicht aus dem Tritt bringen.
    idna::domain_to_unicode(&trimmed).0
}

/// Bildet verwechselbare Zeichen auf ihr lateinisches Gegenstück ab.
///
/// Die Tabelle ist absichtlich klein und handverlesen statt vollständig aus
/// Unicodes `confusables.txt` erzeugt: gebraucht werden die Zeichen, die in
/// echten Angriffen vorkommen, und das sind die kyrillischen und griechischen
/// Buchstaben, die im Schriftbild identisch sind. Eine vollständige Tabelle
/// wäre 6000 Einträge groß und würde Zeichen zusammenwerfen, die nur
/// *ähnlich* sind — jeder davon ein möglicher Fehlalarm.
fn confusable(character: char) -> Option<char> {
    Some(match character {
        // Kyrillisch
        'а' => 'a',
        'в' => 'b',
        'с' => 'c',
        'ԁ' => 'd',
        'е' => 'e',
        'ѕ' => 's',
        'һ' => 'h',
        'і' => 'i',
        'ј' => 'j',
        'к' => 'k',
        'м' => 'm',
        'о' => 'o',
        'р' => 'p',
        'т' => 't',
        'у' => 'y',
        'х' => 'x',
        // Griechisch
        'α' => 'a',
        'β' => 'b',
        'ε' => 'e',
        'ι' => 'i',
        'κ' => 'k',
        'ν' => 'v',
        'ο' => 'o',
        'ρ' => 'p',
        'τ' => 't',
        'υ' => 'u',
        'χ' => 'x',
        // Lateinische Sonderformen, die im Schriftbild kaum auffallen
        'ı' => 'i',
        'ǀ' => 'l',
        'ɡ' => 'g',
        'ł' => 'l',
        'ø' => 'o',
        _ => return None,
    })
}

/// Der Name mit allen Verwechselbaren auf Latein abgebildet.
pub fn skeleton(name: &str) -> String {
    name.chars()
        .map(|character| confusable(character).unwrap_or(character))
        .collect()
}

/// Ob der Name Zeichen außerhalb des lateinischen Alphabets enthält.
fn has_non_ascii(name: &str) -> bool {
    !name.is_ascii()
}

/// Ob die geschützte Domain im Namen steckt, ohne sein Ende zu sein.
///
/// `sparkasse.at.com` und `sparkasse.at.login.example` gehören jemand anderem,
/// sehen aber in einer Adresszeile aus wie die echte Seite — auf einem
/// Telefondisplay ist vom Rest ohnehin nichts mehr zu sehen. Der Fall fällt
/// durch den Abstandsvergleich, weil die registrierbare Domain hier `at.com`
/// bzw. `login.example` ist und mit `sparkasse.at` nichts zu tun hat.
///
/// Abgegrenzt wird an Labelgrenzen: `www.sparkasse.at` endet auf der
/// geschützten Domain und ist damit die echte Seite, `sparkasse.attacke.example`
/// enthält sie nicht als Labelfolge.
fn embeds(name: &str, protected: &str) -> bool {
    if name == protected || name.ends_with(&format!(".{protected}")) {
        return false;
    }
    name.starts_with(&format!("{protected}.")) || name.contains(&format!(".{protected}."))
}

/// Damerau-Levenshtein in der Variante "optimal string alignment".
///
/// Der Unterschied zur vollen Damerau-Distanz: eine Vertauschung darf nicht
/// nachträglich noch einmal bearbeitet werden. Für Tippfehler in Domainnamen
/// ist das ohne Belang und spart die halbe Tabelle.
///
/// Bricht ab, sobald die kleinste Zahl in einer Zeile über `limit` liegt —
/// zwei Namen, die weit auseinander sind, sollen nicht die ganze Matrix kosten.
pub fn distance(left: &str, right: &str, limit: usize) -> Option<usize> {
    let left: Vec<char> = left.chars().collect();
    let right: Vec<char> = right.chars().collect();
    if left.len().abs_diff(right.len()) > limit {
        return None;
    }

    let width = right.len().saturating_add(1);
    // Drei Zeilen genügen: die aktuelle und die zwei davor (für Vertauschungen).
    let mut prev2: Vec<usize> = vec![0; width];
    let mut prev: Vec<usize> = (0..width).collect();
    let mut current: Vec<usize> = vec![0; width];

    for (i, &lc) in left.iter().enumerate() {
        let row = i.saturating_add(1);
        if let Some(first) = current.first_mut() {
            *first = row;
        }
        let mut best = row;

        for (j, &rc) in right.iter().enumerate() {
            let col = j.saturating_add(1);
            let cost = usize::from(lc != rc);
            let deletion = prev.get(col).copied().unwrap_or(usize::MAX);
            let insertion = current.get(j).copied().unwrap_or(usize::MAX);
            let substitution = prev.get(j).copied().unwrap_or(usize::MAX);
            let mut value = deletion
                .saturating_add(1)
                .min(insertion.saturating_add(1))
                .min(substitution.saturating_add(cost));

            // Vertauschung zweier benachbarter Zeichen: `sparkasse` → `sparkasse`
            // mit vertauschtem `ss` kostet eins, nicht zwei.
            if i > 0
                && j > 0
                && lc == right.get(j.wrapping_sub(1)).copied().unwrap_or_default()
                && rc == left.get(i.wrapping_sub(1)).copied().unwrap_or_default()
            {
                let swap = prev2
                    .get(j.wrapping_sub(1))
                    .copied()
                    .unwrap_or(usize::MAX)
                    .saturating_add(1);
                value = value.min(swap);
            }

            if let Some(slot) = current.get_mut(col) {
                *slot = value;
            }
            best = best.min(value);
        }

        if best > limit {
            return None;
        }
        std::mem::swap(&mut prev2, &mut prev);
        std::mem::swap(&mut prev, &mut current);
    }

    prev.last().copied().filter(|&value| value <= limit)
}

impl NameDetector for Typosquat {
    fn detector(&self) -> Detector {
        Detector::Typosquat
    }

    fn inspect(&self, observation: &Observation<'_>) -> Option<Finding> {
        let name = normalize(observation.name);
        // Verglichen wird die registrierbare Domain, nicht der volle Name:
        // `www.sparkasse.at` ist die echte Seite, und `login.sparkase.at` ist
        // eine Fälschung von `sparkase.at` und nicht von `www.sparkasse.at`.
        let candidate = registrable(&name);
        let candidate_skeleton = skeleton(candidate);
        let split = candidate.split_once('.');

        // Bester Treffer über die ganze Liste: sonst entschiede die
        // Reihenfolge in der Konfiguration, welche Begründung erscheint.
        let mut best: Option<(u16, String)> = None;
        for protected in &self.protect {
            // Das Original selbst wird nie gemeldet. Das ist die zweite Hälfte
            // des Abnahmekriteriums aus Schritt 5.
            if candidate == protected.domain {
                return None;
            }

            // Steht die geschützte Domain mitten im Namen, ist die Länge des
            // registrierbaren Teils ohne Belang — geprüft wird der ganze Name.
            if embeds(&name, &protected.domain) {
                let reason = format!(
                    "'{name}' carries '{}' in the name but belongs to '{candidate}'",
                    protected.domain
                );
                if best.as_ref().is_none_or(|(current, _)| 950 > *current) {
                    best = Some((950, reason));
                }
                continue;
            }

            if candidate.len() < MIN_LENGTH {
                continue;
            }
            let Some((candidate_label, candidate_suffix)) = split else {
                continue;
            };

            let hit = if candidate_skeleton == protected.skeleton && has_non_ascii(&name) {
                Some((
                    1000_u16,
                    format!(
                        "'{}' looks like '{}' but uses characters from a different \
                         script",
                        candidate, protected.domain
                    ),
                ))
            } else if candidate_label == protected.label && candidate_suffix != protected.suffix {
                Some((
                    900,
                    format!(
                        "'{}' is '{}' with a different ending",
                        candidate, protected.domain
                    ),
                ))
            } else {
                distance(candidate, &protected.domain, MAX_DISTANCE)
                    .filter(|&d| d > 0)
                    .map(|d| {
                        let score = if d == 1 { 950 } else { 850 };
                        (
                            score,
                            format!(
                                "'{}' differs by {d} characters from '{}'",
                                candidate, protected.domain
                            ),
                        )
                    })
            };

            if let Some((score, reason)) = hit
                && best.as_ref().is_none_or(|(current, _)| score > *current)
            {
                best = Some((score, reason));
            }
        }

        let (score, reason) = best?;
        Some(Finding::new(Detector::Typosquat, score, reason))
    }
}

/// Die letzten beiden Labels — die Näherung genügt hier.
///
/// Anders als bei `split_by_zone` (dort kam dafür die Public Suffix List, siehe
/// ADR-0018) ist die Folge einer zu groben Näherung hier harmlos: Bei
/// `sparkasse.co.uk` würde `co.uk` verglichen, das ist kürzer als
/// [`MIN_LENGTH`]… und fällt heraus. Ein Fehlalarm entsteht dadurch nicht, nur
/// ein blinder Fleck — und der ist über einen zusätzlichen Eintrag in
/// `protect` zu schließen.
fn registrable(name: &str) -> &str {
    let mut parts = name.rmatch_indices('.');
    let _last = parts.next();
    match parts.next() {
        Some((index, _)) => name.get(index.saturating_add(1)..).unwrap_or(name),
        None => name,
    }
}

/// Die Schutzliste, sortiert und ohne Dopplungen — für die API.
pub fn protected_domains(protect: &[String]) -> Vec<String> {
    protect
        .iter()
        .map(|entry| normalize(entry))
        .map(|domain| (domain.clone(), domain))
        .collect::<BTreeMap<_, _>>()
        .into_values()
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use hickory_proto::rr::RecordType;

    fn detector() -> Typosquat {
        Typosquat::new(&[
            "sparkasse.at".to_owned(),
            "id.austria.gv.at".to_owned(),
            "hale.at".to_owned(),
        ])
    }

    fn inspect(name: &str) -> Option<Finding> {
        detector().inspect(&Observation {
            name,
            query_type: RecordType::A,
        })
    }

    #[test]
    fn the_originals_are_never_flagged() {
        // Die zweite Hälfte des Abnahmekriteriums aus Schritt 5, und die
        // wichtigere: ein Detektor, der die echte Bank meldet, wird
        // abgeschaltet, bevor er je einen Angriff sieht.
        for original in [
            "sparkasse.at",
            "www.sparkasse.at",
            "login.sparkasse.at",
            "hale.at",
            "www.hale.at",
        ] {
            assert!(inspect(original).is_none(), "{original} wurde gemeldet");
        }
    }

    #[test]
    fn constructed_variants_of_two_protected_domains_are_found() {
        // Die erste Hälfte des Abnahmekriteriums.
        let variants = [
            // gegen sparkasse.at
            "sparkase.at",
            "sparkassse.at",
            "sparkasse.com",
            "sparkasse.co",
            "spakrasse.at",
            "sparkasse.at.com",
            // gegen hale.at
            "haie.at",
            "hale.com",
            "hal3.at",
        ];
        for variant in variants {
            let finding = inspect(variant).unwrap_or_else(|| panic!("{variant} nicht erkannt"));
            assert!(finding.score >= 850, "{variant}: {}", finding.score);
            assert!(!finding.reason.is_empty());
        }
    }

    #[test]
    fn a_cyrillic_homograph_is_the_strongest_hit() {
        // Kyrillisches 'а' in sparkasse.at. Niemand vertippt sich dorthin.
        let finding = inspect("spаrkasse.at").expect("Homograph nicht erkannt");
        assert_eq!(finding.score, 1000);
        assert!(
            finding.reason.contains("different script"),
            "{}",
            finding.reason
        );
    }

    #[test]
    fn a_homograph_arriving_as_punycode_is_found() {
        // So kommt er tatsächlich an: der Client schickt Punycode.
        let punycode = idna::domain_to_ascii("spаrkasse.at").expect("kodierbar");
        assert!(punycode.starts_with("xn--"), "{punycode}");
        let finding = inspect(&punycode).expect("Punycode-Homograph nicht erkannt");
        assert_eq!(finding.score, 1000);
    }

    #[test]
    fn unrelated_domains_are_left_alone() {
        for name in [
            "example.com",
            "wikipedia.org",
            "orf.at",
            "github.com",
            "raiffeisen.at",
            "bank99.at",
        ] {
            assert!(inspect(name).is_none(), "{name} fälschlich gemeldet");
        }
    }

    #[test]
    fn very_short_names_are_not_compared() {
        // Bei drei Zeichen wäre Abstand zwei die halbe Domain.
        let short = Typosquat::new(&["t.co".to_owned()]);
        for name in ["x.co", "t.io", "a.co"] {
            assert!(
                short
                    .inspect(&Observation {
                        name,
                        query_type: RecordType::A
                    })
                    .is_none(),
                "{name}"
            );
        }
    }

    #[test]
    fn the_strongest_match_wins_over_the_first_in_the_list() {
        // `hale.com` ist Abstand 2 zu `hale.at` — aber als andere Endung ein
        // stärkerer Treffer. Es soll die stärkere Begründung erscheinen.
        let finding = inspect("hale.com").expect("nicht erkannt");
        assert_eq!(finding.score, 900);
        assert!(
            finding.reason.contains("different ending"),
            "{}",
            finding.reason
        );
    }

    #[test]
    fn distance_counts_the_operations_it_says_it_does() {
        assert_eq!(distance("abc", "abc", 2), Some(0));
        assert_eq!(distance("abc", "abd", 2), Some(1)); // Ersetzen
        assert_eq!(distance("abc", "ab", 2), Some(1)); // Löschen
        assert_eq!(distance("abc", "abcd", 2), Some(1)); // Einfügen
        assert_eq!(distance("abc", "acb", 2), Some(1)); // Vertauschen
        assert_eq!(distance("abcd", "badc", 2), Some(2)); // zwei Vertauschungen
        assert_eq!(distance("abc", "xyz", 2), None); // über der Grenze
        assert_eq!(distance("", "", 2), Some(0));
        assert_eq!(distance("a", "", 2), Some(1));
    }

    #[test]
    fn distance_is_symmetric() {
        for (left, right) in [
            ("sparkasse", "sparkase"),
            ("hale", "haie"),
            ("abcdef", "abcdef"),
            ("orf", "ard"),
        ] {
            assert_eq!(
                distance(left, right, 3),
                distance(right, left, 3),
                "{left} / {right}"
            );
        }
    }

    #[test]
    fn the_skeleton_maps_confusables_to_latin() {
        assert_eq!(skeleton("spаrkasse"), "sparkasse"); // kyrillisches а
        assert_eq!(skeleton("sparkasse"), "sparkasse"); // schon lateinisch
        assert_eq!(skeleton("ραypal"), "paypal"); // griechisch
    }

    #[test]
    fn an_empty_protect_list_finds_nothing() {
        let empty = Typosquat::new(&[]);
        assert!(empty.is_empty());
        assert!(
            empty
                .inspect(&Observation {
                    name: "sparkase.at",
                    query_type: RecordType::A
                })
                .is_none()
        );
    }

    #[test]
    fn hostile_input_does_not_panic() {
        // B.1 Regel 1: jedes Byte vom Netzwerk ist feindlich. Der Name kommt
        // aus einer Anfrage.
        let long = "a".repeat(4000);
        for name in [
            "",
            ".",
            "..",
            "...",
            "xn--",
            "xn--a",
            "xn--0000000000",
            "\u{0}\u{0}\u{0}",
            &long,
            &format!("{long}.at"),
            "………",
        ] {
            let _ = inspect(name);
        }
    }
}
