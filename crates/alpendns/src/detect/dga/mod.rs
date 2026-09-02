//! DGA-Erkennung: algorithmisch erzeugte Domainnamen.
//!
//! Malware, die ihren Steuerserver nicht in den Code schreiben will, erzeugt
//! Namen aus einem Algorithmus mit dem Datum als Saat: `kqxvbnzmrt.com`,
//! morgen ein anderer. Wer die Liste blocken will, müsste den Algorithmus
//! kennen. Wer *erkennen* will, dass ein Name so entstanden ist, braucht das
//! nicht — solche Namen sehen anders aus als gewachsene.
//!
//! # Wie
//!
//! Ein Zeichen-3-Gramm-Modell über einer Popularitätsliste (FEATURES.md D3).
//! Für jedes Zeichentripel steht in der Tabelle, wie überraschend das dritte
//! Zeichen nach den beiden davor ist. `-ing` ist wenig überraschend, `-qxv`
//! sehr. Der Score ist die mittlere Überraschung über den ganzen Namen,
//! normalisiert auf seine Länge, plus zwei Hilfsmerkmale: Konsonantenhäufungen
//! und Ziffernanteil.
//!
//! Das Modell liegt als Tabelle im Binary — 38³ Einträge zu je zwei Byte,
//! rund 107 KiB. Kein Nachladen, keine GPU, keine Laufzeit über ein paar
//! Mikrosekunden. Woher es kommt und wie es erzeugt wurde, steht in
//! `model.bin.md` daneben; erzeugen lässt es sich mit
//! `cargo test --release --test detect_corpus -- --ignored train`.
//!
//! # Grenzen, die dokumentiert gehören
//!
//! * **Kurze Namen sind nicht unterscheidbar.** `bit.ly`, `t.co`, `vk.com` —
//!   bei fünf Zeichen gibt es zu wenig Text für eine Statistik. Namen unter
//!   [`MIN_LENGTH`] werden gar nicht erst bewertet.
//! * **Wörterbuch-DGAs erkennt das Modell nicht.** Familien wie `suppobox`
//!   setzen zwei echte Wörter aneinander (`sandwichfriend.net`); für ein
//!   Zeichenmodell ist das ein gewachsener Name. Das ist keine Lücke in der
//!   Umsetzung, sondern die Grenze des Verfahrens.
//! * **CDN-Hostnamen sehen aus wie DGA.** Deshalb wird nur die registrierbare
//!   Domain bewertet und nicht der ganze Name: `d1a2b3c4.cloudfront.net` ist
//!   `cloudfront.net`, und das ist ein gewachsener Name.
//! * **Über Namen außerhalb des lateinischen Alphabets sagt das Modell
//!   nichts.** Ein IDN erreicht uns als Punycode — `xn--vhqrb498dfmcffp24.cn` —
//!   und das ist eine base32-artige Zeichenfolge, die jedem Zeichenmodell wie
//!   eine DGA aussieht. Beim ersten Messlauf waren vier der zwanzig
//!   auffälligsten Namen Punycode. Sie werden jetzt gar nicht erst bewertet:
//!   lieber ein blinder Fleck, den man benennen kann, als ein Fehlalarm bei
//!   jedem chinesischen oder russischen Domainnamen.
//! * **Pinyin-Kürzel bleiben ein Fehlalarm.** `hnqxdzkj.com` oder `lzdsxxb.com`
//!   sind gewachsene Namen aus Anfangsbuchstaben chinesischer Silben, und für
//!   ein Modell über lateinischem Text sind sie nicht von Zufall zu
//!   unterscheiden. Sie machen den größten Teil der verbleibenden
//!   Falsch-Positiven aus.
//!
//! Das Qualitätsmaß ist deshalb die **Falsch-Positiv-Rate**, nicht die
//! Trefferquote (FEATURES.md D3). Die Zahlen stehen in BENCHMARKS.md.

use super::{Detector, Finding, NameDetector, Observation};

/// Zeichen, die das Modell kennt: Rand/Sonstiges, a–z, 0–9, Bindestrich.
pub const ALPHABET: usize = 38;

/// So viele Einträge hat die Tabelle.
pub const TABLE_SIZE: usize = ALPHABET * ALPHABET * ALPHABET;

/// Der Faktor, mit dem die Überraschung in der Tabelle festgehalten ist.
///
/// Gespeichert wird `-log2(p) * 512` als `u16`. Das reicht für Werte bis 128
/// Bit bei einer Auflösung von 1/512 Bit — deutlich feiner, als das Modell
/// selbst unterscheiden kann, und ohne Gleitkomma in der Datei.
pub const SCALE: f32 = 512.0;

/// Kürzere Namen als dieser werden nicht bewertet.
///
/// Bei sechs Zeichen bleiben nach den Randmarken sechs Tripel — zu wenig, um
/// einen Mittelwert daraus abzuleiten, der nicht vom Zufall lebt.
pub const MIN_LENGTH: usize = 7;

/// Mittlere Überraschung, zwischen der das Hauptsignal ansteigt.
///
/// Aus der Messung (siehe `model.bin.md`): gewachsene Namen liegen im Median
/// bei 3,7 Bit je Tripel, zufällig erzeugte je nach Alphabet bei 6,0 bis 7,9.
/// Die beiden Verteilungen überlappen — das ist die Eigenschaft des Verfahrens
/// und der Grund, warum die Trefferquote je Familie so unterschiedlich ausfällt.
const NLL_FLOOR: f32 = 5.0;
const NLL_CEILING: f32 = 6.5;

/// Ab diesem Score gilt ein Name als algorithmisch erzeugt.
///
/// Die Zahl gehört neben das Modell, weil sie nur mit ihm zusammen einen Sinn
/// ergibt. Gemessen auf der Top-100k, die nicht im Training steckt:
/// **0,077 % Falsch-Positive** bei dieser Schwelle, gegen eine Zusage von
/// 0,1 %.
///
/// Warum nicht höher: bei 0,80 fällt die Trefferquote der alphanumerischen
/// Familie von 92,8 % auf 38,8 %, während die Fehlalarme nur von 77 auf 39
/// zurückgehen. 0,75 liegt unmittelbar vor dieser Kante. Warum nicht
/// niedriger: bei 0,70 sind es 0,098 % Fehlalarme, und damit bliebe keine
/// Reserve für einen anderen Verkehrsmix.
pub const DEFAULT_THRESHOLD: f32 = 0.75;

/// Die eingebaute Tabelle.
///
/// `include_bytes!` statt Einlesen: das Modell ist Teil des Programms, nicht
/// seiner Konfiguration. Ein Modell auf der Platte wäre eine zweite Sorte
/// Zustand, die auseinanderlaufen kann — und eine Datei, die jemand austauschen
/// könnte, um die Erkennung stillzulegen.
static MODEL: &[u8; TABLE_SIZE * 2] = include_bytes!("model.bin");

/// Der Platz eines Zeichens im Alphabet des Modells.
const fn index_of(byte: u8) -> usize {
    match byte {
        b'a'..=b'z' => (byte - b'a') as usize + 1,
        b'0'..=b'9' => (byte - b'0') as usize + 27,
        b'-' => 37,
        // Alles andere fällt auf die Randmarke. Ein Name mit Zeichen außerhalb
        // dieses Alphabets ist entweder Punycode (dann steht dort `xn--` und
        // damit lauter bekannte Zeichen) oder kaputt.
        _ => 0,
    }
}

/// Die Überraschung des dritten Zeichens nach den beiden davor, in Bit.
fn surprise(first: usize, second: usize, third: usize) -> f32 {
    let offset = first
        .saturating_mul(ALPHABET)
        .saturating_add(second)
        .saturating_mul(ALPHABET)
        .saturating_add(third)
        .saturating_mul(2);
    let Some(bytes) = MODEL.get(offset..offset.saturating_add(2)) else {
        return 0.0;
    };
    let raw = u16::from_le_bytes([
        bytes.first().copied().unwrap_or(0),
        bytes.get(1).copied().unwrap_or(0),
    ]);
    f32::from(raw) / SCALE
}

/// Zerlegt ein Label in die Symbolfolge, über die das Modell rechnet.
///
/// Vorn zwei Randmarken, hinten eine: so zählen Anfang und Ende mit. `xq` am
/// Anfang ist auffällig, `er` am Ende nicht.
///
/// Öffentlich, weil der Trainer (`tests/detect_corpus.rs`) dieselbe Zerlegung
/// braucht. Zwei Kopien davon wären die Sorte Fehler, die man erst Monate
/// später bemerkt: das Modell wäre über einer anderen Folge trainiert als der,
/// gegen die es dann bewertet.
pub fn symbols(label: &str) -> Vec<usize> {
    let mut symbols = Vec::with_capacity(label.len().saturating_add(3));
    symbols.push(0);
    symbols.push(0);
    symbols.extend(label.bytes().map(index_of));
    symbols.push(0);
    symbols
}

/// Die mittlere Überraschung über einen Namen, in Bit je Tripel.
pub fn mean_surprise(label: &str) -> f32 {
    let symbols = symbols(label);
    let mut total = 0.0_f32;
    let mut count = 0_u32;
    for window in symbols.windows(3) {
        let (Some(&a), Some(&b), Some(&c)) = (window.first(), window.get(1), window.get(2)) else {
            continue;
        };
        total += surprise(a, b, c);
        count = count.saturating_add(1);
    }
    if count == 0 {
        return 0.0;
    }
    #[expect(
        clippy::cast_precision_loss,
        reason = "Labellängen liegen unter 64; die Zahl der Tripel erst recht"
    )]
    {
        total / count as f32
    }
}

/// Anteil der Ziffern am Namen.
fn digit_ratio(label: &str) -> f32 {
    let digits = label.bytes().filter(u8::is_ascii_digit).count();
    if label.is_empty() {
        return 0.0;
    }
    #[expect(clippy::cast_precision_loss, reason = "Labellängen liegen unter 64")]
    {
        digits as f32 / label.len() as f32
    }
}

/// Die längste Kette von Konsonanten hintereinander.
///
/// Gewachsene Namen haben selten mehr als vier (`strand`, `schwarz`); zufällige
/// Buchstabenfolgen kommen regelmäßig auf sechs und mehr.
fn longest_consonant_run(label: &str) -> usize {
    let mut longest = 0;
    let mut current = 0;
    for byte in label.bytes() {
        if byte.is_ascii_alphabetic() && !matches!(byte, b'a' | b'e' | b'i' | b'o' | b'u' | b'y') {
            current += 1;
            longest = longest.max(current);
        } else {
            current = 0;
        }
    }
    longest
}

fn clamp01(value: f32) -> f32 {
    value.clamp(0.0, 1.0)
}

/// Der Score eines einzelnen Labels, ohne Rücksicht auf die Schwelle.
///
/// Öffentlich, weil die Messläufe gegen Korpora ihn brauchen (siehe
/// `tests/detect_corpus.rs`).
pub fn score_label(label: &str) -> super::Permille {
    if label.len() < MIN_LENGTH {
        return 0;
    }
    let nll = mean_surprise(label);
    let statistical = clamp01((nll - NLL_FLOOR) / (NLL_CEILING - NLL_FLOOR));

    #[expect(clippy::cast_precision_loss, reason = "Labellängen liegen unter 64")]
    let consonants = clamp01((longest_consonant_run(label) as f32 - 4.0) / 3.0);
    let digits = clamp01((digit_ratio(label) - 0.2) / 0.4);

    super::permille(0.75 * statistical + 0.15 * consonants + 0.10 * digits)
}

/// Erkennt algorithmisch erzeugte Namen.
#[derive(Debug, Default)]
pub struct Dga {
    _private: (),
}

impl Dga {
    pub const fn new() -> Self {
        Self { _private: () }
    }
}

/// Der Teil, der bewertet wird: das erste Label der registrierbaren Domain.
///
/// Über die Public Suffix List, wie bei `split_by_zone` (ADR-0018) — hier aus
/// einem anderen Grund: ohne sie wäre für `x.co.uk` das bewertete Label `co`,
/// und für `d1a2b3.cloudfront.net` wäre es `d1a2b3` statt `cloudfront`. Das
/// zweite ist der teure Fehler, denn so sieht jeder CDN-Hostname aus.
pub fn scored_label(name: &str) -> Option<&str> {
    let trimmed = name.trim_end_matches('.');

    // Unterhalb eines *privaten* Suffixes vergibt ein Anbieter die Namen:
    // `cloudfront.net`, `s3.amazonaws.com`, `github.io` stehen selbst in der
    // Public Suffix List. Dort ist ein zufällig aussehender Name der Normalfall
    // und kein Signal — jede CloudFront-Verteilung heißt so. Bewerten hieße,
    // eine Bauweise für einen Angriff zu halten.
    if psl::suffix(trimmed.as_bytes()).and_then(|suffix| suffix.typ()) == Some(psl::Type::Private) {
        return None;
    }

    let registrable = psl::domain_str(trimmed).unwrap_or(trimmed);
    let label = registrable.split('.').next()?;
    if label.is_empty() || label.starts_with("xn--") {
        // Punycode: siehe die Grenzen im Modulkopf. Dekodieren hülfe nicht —
        // dann stünden dort chinesische Zeichen, über die das Modell erst
        // recht nichts weiß.
        return None;
    }
    Some(label)
}

impl NameDetector for Dga {
    fn detector(&self) -> Detector {
        Detector::Dga
    }

    fn inspect(&self, observation: &Observation<'_>) -> Option<Finding> {
        let label = scored_label(observation.name)?;
        let score = score_label(label);
        if score == 0 {
            return None;
        }
        Some(Finding::new(
            Detector::Dga,
            score,
            format!(
                "'{label}' does not fit naturally grown names: {:.1} bits of surprise per \
                 character triple, longest consonant run {}, digit share {:.0} %",
                mean_surprise(label),
                longest_consonant_run(label),
                digit_ratio(label) * 100.0
            ),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use hickory_proto::rr::RecordType;

    fn inspect(name: &str) -> Option<Finding> {
        Dga::new().inspect(&Observation {
            name,
            query_type: RecordType::A,
        })
    }

    fn score(name: &str) -> super::super::Permille {
        inspect(name).map_or(0, |finding| finding.score)
    }

    #[test]
    fn the_model_has_the_size_it_claims() {
        assert_eq!(MODEL.len(), TABLE_SIZE * 2);
        // Eine Tabelle aus lauter Nullen wäre ein leeres Modell, das alles für
        // unauffällig hielte — und der Test darunter würde es nicht merken.
        assert!(
            MODEL.iter().filter(|&&byte| byte != 0).count() > TABLE_SIZE,
            "das Modell sieht leer aus"
        );
    }

    #[test]
    fn grown_names_score_low() {
        for name in [
            "google.com",
            "wikipedia.org",
            "sparkasse.at",
            "cloudflare.com",
            "github.com",
            "stackoverflow.com",
            "raiffeisen.at",
            "bundeskanzleramt.gv.at",
            "sueddeutsche.de",
            "amazonaws.com",
            "gravatar.com",
            "wordpress.org",
        ] {
            let score = score(name);
            assert!(
                score < super::super::permille(DEFAULT_THRESHOLD),
                "{name} kam auf {score}"
            );
        }
    }

    #[test]
    fn a_cdn_hostname_is_not_mistaken_for_a_generated_domain() {
        // Der teure Fehler: `d1a2b3c4e5.cloudfront.net` sieht wie eine DGA aus
        // und ist die normale Form eines CDN-Namens. Zwei verschiedene Gründe,
        // warum nichts gemeldet wird — beide sollen halten.
        for name in [
            // Privates Suffix: der Anbieter vergibt die Namen.
            "d1a2b3c4e5f6g7.cloudfront.net",
            "k8x2mq9v.s3.amazonaws.com",
            "zufaelligername.github.io",
            // Kein privates Suffix, aber bewertet wird die registrierbare
            // Domain und nicht der Host darunter.
            "xyzzy123abcdef.akamaiedge.net",
            "a8f3d9c2b1e7.example.com",
        ] {
            assert!(
                score(name) < super::super::permille(DEFAULT_THRESHOLD),
                "{name} kam auf {}",
                score(name)
            );
        }
    }

    #[test]
    fn internationalised_names_are_not_judged_at_all() {
        // Punycode sieht für ein lateinisches Zeichenmodell wie base32 aus.
        // Beim ersten Messlauf waren vier der zwanzig auffälligsten Namen
        // solche — ein systematischer Fehlalarm für ganze Sprachräume.
        for name in [
            "xn--vhqrb498dfmcffp24qfocl09dqkh.cn",
            "xn--80aeeqaabljrdbg6a3ahhcl4ay9hsa.xn--p1ai",
            "www.xn--xhqv9jbtpfww.com",
        ] {
            assert_eq!(score(name), 0, "{name}");
            assert!(scored_label(name).is_none(), "{name}");
        }
    }

    #[test]
    fn short_names_are_not_judged_at_all() {
        // FEATURES.md D3: bei fünf Zeichen gibt es keine Statistik.
        for name in ["t.co", "bit.ly", "vk.com", "x.com", "orf.at"] {
            assert_eq!(score(name), 0, "{name}");
        }
    }

    #[test]
    fn the_reason_names_the_features() {
        let finding = inspect("kqxvbnzmrtwp.com").expect("ein Fund");
        for expected in ["surprise", "consonant run", "digit share"] {
            assert!(
                finding.reason.contains(expected),
                "'{expected}' fehlt in: {}",
                finding.reason
            );
        }
    }

    #[test]
    fn a_word_list_dga_is_honestly_not_detected() {
        // Steht als Grenze in der Moduldokumentation und in FEATURES.md D3.
        // Der Test hält fest, dass es eine bekannte Grenze ist und keine
        // Überraschung — wenn er eines Tages fehlschlägt, ist das Modell besser
        // geworden und die Dokumentation überholt.
        for name in [
            "sandwichfriend.net",
            "morningwindow.com",
            "silverforest.net",
        ] {
            assert!(
                score(name) < super::super::permille(DEFAULT_THRESHOLD),
                "{name} wurde erkannt — die Dokumentation ist überholt"
            );
        }
    }

    #[test]
    fn consonant_runs_are_counted() {
        assert_eq!(longest_consonant_run("aeiou"), 0);
        assert_eq!(longest_consonant_run("strand"), 3);
        assert_eq!(longest_consonant_run("kqxvbnz"), 7);
        assert_eq!(longest_consonant_run(""), 0);
        assert_eq!(longest_consonant_run("ab1cd"), 2);
    }

    #[test]
    fn digit_ratio_counts_digits() {
        assert!((digit_ratio("abc123") - 0.5).abs() < f32::EPSILON);
        assert_eq!(digit_ratio("abcdef"), 0.0);
        assert_eq!(digit_ratio(""), 0.0);
    }

    #[test]
    fn hostile_names_do_not_panic() {
        let long = "a".repeat(5000);
        for name in [
            "",
            ".",
            "..",
            "xn--",
            "\u{0}\u{0}\u{0}\u{0}\u{0}\u{0}\u{0}\u{0}",
            &long,
            &format!("{long}.{long}"),
            "ÄÖÜ-mit-Umlauten.example",
        ] {
            let _ = inspect(name);
        }
    }
}
