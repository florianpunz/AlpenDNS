//! Die Messläufe zu Phase 8, Schritt 3 und 4 — und der Trainer für das
//! DGA-Modell.
//!
//! **Alles hier läuft nur mit `--ignored`**, wie der Lastgenerator aus Phase 2
//! (`tests/load.rs`). Der Grund ist derselbe: die Läufe brauchen einen Korpus,
//! der nicht im Repo liegt, und sie dauern länger, als eine Testsuite dauern
//! darf. Was sich ohne Korpus prüfen lässt, steht als gewöhnlicher Test bei den
//! Detektoren selbst.
//!
//! # Den Korpus besorgen
//!
//! ```sh
//! mkdir -p corpus
//! curl -sSL https://downloads.majestic.com/majestic_million.csv | tail -n +2 \
//!   | cut -d, -f3 > /tmp/majestic.txt
//! head -100000            /tmp/majestic.txt > corpus/top-100k.txt    # Messung
//! sed -n '100001,600000p' /tmp/majestic.txt > corpus/train-500k.txt  # Training
//! ```
//!
//! **Zwei Korpora, und das ist der Punkt:** gemessen wird auf der Top-100k,
//! trainiert auf den Rängen dahinter. Kein Name der Messung steckt im Modell.
//! Beim ersten Anlauf lief es andersherum, und der Unterschied war Faktor 500 —
//! 0,001 % gegen 0,54 %.
//!
//! Majestic Million, CC-BY 3.0. Warum diese Liste und nicht Tranco oder
//! Umbrella: sie kommt als reine CSV ohne Zip, und ihre Lizenz erlaubt die
//! Weitergabe abgeleiteter Werke unter Namensnennung — genau das ist das
//! Modell. Das Verzeichnis `corpus/` steht in `.gitignore`.
//!
//! # Die Läufe
//!
//! ```sh
//! # Modell neu trainieren (schreibt src/detect/dga/model.bin)
//! cargo test --release --test detect_corpus -- --ignored train_the_dga_model --nocapture
//!
//! # Falsch-Positiv-Raten und Trefferquoten messen
//! cargo test --release --test detect_corpus -- --ignored measure --nocapture
//! ```

#![allow(clippy::expect_used, clippy::indexing_slicing, clippy::panic)]

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use alpendns::clock::TestClock;
use alpendns::detect::{
    Detector, NameDetector as _, Observation, Permille, dga, permille, tunneling,
};
use hickory_proto::rr::RecordType;

/// Wo die Korpora liegen. Über die Umgebung überschreibbar.
fn corpus_path(name: &str) -> PathBuf {
    std::env::var_os("ALPENDNS_CORPUS_DIR").map_or_else(
        || {
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../..")
                .join("corpus")
                .join(name)
        },
        |dir| PathBuf::from(dir).join(name),
    )
}

fn model_path() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("src/detect/dga/model.bin")
}

/// Woraus das Modell lernt: die Ränge 100 001 bis 600 000.
///
/// Bewusst **nicht** die vordersten Ränge. Die gehören der Messung, und ein
/// Modell, das auf denselben Namen geprüft wird, aus denen es gebaut wurde,
/// meldet eine Zahl über sich selbst statt über die Wirklichkeit. Beim ersten
/// Anlauf war der Unterschied Faktor 500: 0,001 % auf den Trainingsdaten,
/// 0,54 % auf ungesehenen.
fn training_corpus() -> Vec<String> {
    load("train-500k.txt")
}

/// Woran gemessen wird: die Top-100k, so wie es die Roadmap verlangt.
///
/// Kein einziger dieser Namen steckt im Modell. Das ist zugleich der
/// aussagekräftigste Ausschnitt, den es gibt: es sind die Namen, die im Betrieb
/// tatsächlich gefragt werden.
fn holdout_corpus() -> Vec<String> {
    load("top-100k.txt")
}

fn load(name: &str) -> Vec<String> {
    let path = corpus_path(name);
    let text = std::fs::read_to_string(&path).unwrap_or_else(|error| {
        panic!(
            "Korpus {} nicht lesbar ({error}). Wie man ihn besorgt, steht im Kopf \
             dieser Datei.",
            path.display()
        )
    });
    text.lines()
        .map(|line| line.trim().trim_end_matches('.').to_lowercase())
        .filter(|line| !line.is_empty() && line.contains('.') && line.is_ascii())
        .collect()
}

/// Das Label, das das Modell bewertet — dieselbe Auswahl wie im Betrieb.
fn label_of(domain: &str) -> Option<String> {
    dga::scored_label(domain).map(ToOwned::to_owned)
}

// ---------------------------------------------------------------------------
// Trainer
// ---------------------------------------------------------------------------

/// Trainiert das 3-Gramm-Modell und schreibt `src/detect/dga/model.bin`.
///
/// Geschätzt wird `P(c3 | c1, c2)` mit additiver Glättung; gespeichert wird
/// `-log2(p)`, skaliert und als `u16`. Die Glättung ist der Grund, warum ein
/// nie gesehenes Tripel einen hohen, aber endlichen Wert bekommt statt
/// unendlich — sonst entschiede ein einziges seltenes Zeichen über den ganzen
/// Namen.
#[test]
#[ignore = "braucht einen Korpus; schreibt das Modell"]
fn train_the_dga_model() {
    /// Wie stark geglättet wird. Klein genug, dass ein nie gesehenes Tripel
    /// deutlich auffällt; groß genug, dass es nicht allein entscheidet.
    const ALPHA: f64 = 0.01;

    let domains = training_corpus();
    assert!(
        domains.len() > 10_000,
        "der Korpus ist mit {} Einträgen zu klein für ein Modell",
        domains.len()
    );

    let mut trigram = vec![0_u64; dga::TABLE_SIZE];
    let mut bigram = vec![0_u64; dga::ALPHABET * dga::ALPHABET];
    let mut labels = 0_u64;

    for domain in &domains {
        let Some(label) = label_of(domain) else {
            continue;
        };
        if label.len() < 2 {
            continue;
        }
        labels += 1;
        // Dieselbe Zerlegung wie bei der Bewertung — sie kommt aus dem Crate,
        // damit Trainer und Detektor nicht auseinanderlaufen können.
        let symbols = dga::symbols(&label);
        for window in symbols.windows(3) {
            let (a, b, c) = (window[0], window[1], window[2]);
            trigram[(a * dga::ALPHABET + b) * dga::ALPHABET + c] += 1;
            bigram[a * dga::ALPHABET + b] += 1;
        }
    }

    let denominator_bonus = ALPHA * dga::ALPHABET as f64;
    let mut out = Vec::with_capacity(dga::TABLE_SIZE * 2);
    let mut max_bits = 0.0_f64;
    for a in 0..dga::ALPHABET {
        for b in 0..dga::ALPHABET {
            let total = bigram[a * dga::ALPHABET + b] as f64;
            for c in 0..dga::ALPHABET {
                let count = trigram[(a * dga::ALPHABET + b) * dga::ALPHABET + c] as f64;
                let p = (count + ALPHA) / (total + denominator_bonus);
                let bits = -p.log2();
                max_bits = max_bits.max(bits);
                let scaled = (bits * f64::from(dga::SCALE)).round();
                let value = if scaled >= f64::from(u16::MAX) {
                    u16::MAX
                } else if scaled <= 0.0 {
                    0
                } else {
                    scaled as u16
                };
                out.extend_from_slice(&value.to_le_bytes());
            }
        }
    }

    assert_eq!(out.len(), dga::TABLE_SIZE * 2);
    std::fs::write(model_path(), &out).expect("Modell schreibbar");

    println!("\n=== DGA-Modell trainiert ===");
    println!("Korpus:            {} Domains", domains.len());
    println!("verwertbare Labels: {labels}");
    println!("Tabelle:           {} Byte", out.len());
    println!("größte Überraschung: {max_bits:.2} Bit");

    // Die Verteilung der gewachsenen Namen — daraus kommen NLL_FLOOR und
    // NLL_CEILING im Detektor.
    let mut surprises: Vec<f32> = domains
        .iter()
        .filter_map(|domain| label_of(domain))
        .filter(|label| label.len() >= dga::MIN_LENGTH)
        .map(|label| dga::mean_surprise(&label))
        .collect();
    surprises.sort_by(|left, right| left.partial_cmp(right).unwrap_or(std::cmp::Ordering::Equal));
    println!("\nÜberraschung gewachsener Namen (Bit je Tripel):");
    for (label, quantile) in [
        ("Median", 0.50),
        ("90 %", 0.90),
        ("99 %", 0.99),
        ("99,9 %", 0.999),
        ("Maximum", 1.0),
    ] {
        let index = ((surprises.len() - 1) as f64 * quantile) as usize;
        println!("  {label:>8}: {:.2}", surprises[index]);
    }
    println!(
        "\nHINWEIS: Das Modell wurde neu geschrieben. Die Messungen darunter gelten \
         erst nach einem erneuten `cargo build`."
    );
}

// ---------------------------------------------------------------------------
// DGA-Familien
// ---------------------------------------------------------------------------

/// Ein kleiner, deterministischer Generator.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 = self
            .0
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        self.0 >> 33
    }

    fn pick(&mut self, from: &[u8]) -> char {
        char::from(from[(self.next() as usize) % from.len()])
    }

    fn between(&mut self, low: usize, high: usize) -> usize {
        low + (self.next() as usize) % (high - low + 1)
    }
}

/// Wie eine DGA-Familie ihre Namen baut.
///
/// **Nachgebaut, nicht mitgeschnitten.** Was hier steht, sind die
/// veröffentlichten Merkmale der Familien — Alphabet und Längenbereich —, nicht
/// ihre echten Saaten. Für die Frage, ob ein Zeichenmodell solche Namen von
/// gewachsenen unterscheidet, ist genau das ausschlaggebend; die echte Saat
/// würde dieselbe Verteilung erzeugen. Wo eine Familie etwas anderes tut als
/// gleichverteilt zu würfeln, steht es dabei.
struct Family {
    name: &'static str,
    generate: fn(&mut Rng) -> String,
}

const LOWERCASE: &[u8] = b"abcdefghijklmnopqrstuvwxyz";
const CONSONANTS: &[u8] = b"bcdfghjklmnpqrstvwxz";
const VOWELS: &[u8] = b"aeiou";

const FAMILIES: [Family; 5] = [
    // Gleichverteilte Kleinbuchstaben, 8–11 Zeichen.
    Family {
        name: "conficker-artig",
        generate: |rng| {
            let length = rng.between(8, 11);
            (0..length).map(|_| rng.pick(LOWERCASE)).collect()
        },
    },
    // Wie oben, aber deutlich länger — der Bereich, in dem `necurs` arbeitet.
    Family {
        name: "necurs-artig",
        generate: |rng| {
            let length = rng.between(12, 20);
            (0..length).map(|_| rng.pick(LOWERCASE)).collect()
        },
    },
    // Abwechselnd Konsonant und Vokal: der Fall, der einem Zeichenmodell am
    // meisten zu schaffen macht, weil er aussprechbar aussieht.
    Family {
        name: "kraken-artig (aussprechbar)",
        generate: |rng| {
            let syllables = rng.between(4, 6);
            (0..syllables)
                .map(|_| {
                    let consonant = rng.pick(CONSONANTS);
                    let vowel = rng.pick(VOWELS);
                    format!("{consonant}{vowel}")
                })
                .collect()
        },
    },
    // Buchstaben und Ziffern gemischt, wie sie einige neuere Familien benutzen.
    Family {
        name: "alphanumerisch",
        generate: |rng| {
            let length = rng.between(10, 16);
            (0..length)
                .map(|_| rng.pick(b"abcdefghijklmnopqrstuvwxyz0123456789"))
                .collect()
        },
    },
    // Zwei Wörterbuchwörter aneinander. Die dokumentierte Grenze: das Modell
    // soll das *nicht* erkennen, und die gemessene Quote belegt es.
    Family {
        name: "suppobox-artig (Wörterbuch)",
        generate: |rng| {
            const WORDS: [&str; 16] = [
                "sandwich", "friend", "morning", "window", "silver", "forest", "yellow",
                "mountain", "summer", "garden", "winter", "harbour", "orange", "candle", "purple",
                "river",
            ];
            let first = WORDS[(rng.next() as usize) % WORDS.len()];
            let second = WORDS[(rng.next() as usize) % WORDS.len()];
            format!("{first}{second}")
        },
    },
];

// ---------------------------------------------------------------------------
// Messläufe
// ---------------------------------------------------------------------------

/// Schritt 4: Falsch-Positiv-Rate auf der Top-100k, Trefferquote je Familie.
#[test]
#[ignore = "braucht einen Korpus"]
fn measure_dga_rates() {
    let threshold = permille(dga::DEFAULT_THRESHOLD);
    // Gemessen wird auf dem Korpus, den das Modell nie gesehen hat.
    let domains = holdout_corpus();
    let detector = dga::Dga::new();

    let mut examined = 0_u64;
    let mut flagged: Vec<(String, Permille)> = Vec::new();
    for domain in &domains {
        examined += 1;
        if let Some(finding) = detector.inspect(&Observation {
            name: domain,
            query_type: RecordType::A,
        }) && finding.score >= threshold
        {
            flagged.push((domain.clone(), finding.score));
        }
    }

    let rate = flagged.len() as f64 / examined as f64;
    println!("\n=== DGA: Falsch-Positive auf der Top-100k (nicht im Training) ===");
    println!("Schwelle:  {} ({:.2})", threshold, dga::DEFAULT_THRESHOLD);
    println!("geprüft:   {examined}");
    println!("geflaggt:  {} ({:.4} %)", flagged.len(), rate * 100.0);
    flagged.sort_by_key(|(_, score)| std::cmp::Reverse(*score));
    println!("die zwanzig auffälligsten:");
    for (domain, score) in flagged.iter().take(20) {
        println!("  {score:>4}  {domain}");
    }

    println!("\n=== DGA: Trefferquote je Familie (je 5000 Namen) ===");
    println!(
        "  {:>30}  {:>7}  {:>9}  {:>9}",
        "Familie", "Treffer", "Ø Bit", "Ø Score"
    );
    for family in &FAMILIES {
        let mut rng = Rng(0x5eed_1234_abcd_ef01);
        let mut hits = 0_u32;
        let mut bits = 0.0_f64;
        let mut scores = 0_u64;
        for _ in 0..5000 {
            let label = (family.generate)(&mut rng);
            bits += f64::from(dga::mean_surprise(&label));
            scores += u64::from(dga::score_label(&label));
            let name = format!("{label}.com");
            if let Some(finding) = detector.inspect(&Observation {
                name: &name,
                query_type: RecordType::A,
            }) && finding.score >= threshold
            {
                hits += 1;
            }
        }
        println!(
            "  {:>30}  {:>6.1} %  {:>9.2}  {:>9.0}",
            family.name,
            f64::from(hits) / 50.0,
            bits / 5000.0,
            scores as f64 / 5000.0
        );
    }

    assert!(
        rate < 0.001,
        "Falsch-Positiv-Rate {:.4} % über der Zusage von 0,1 %",
        rate * 100.0
    );
}

/// Schritt 3: Falsch-Positiv-Rate der Tunneling-Erkennung auf demselben Korpus.
///
/// Der Korpus wird dabei so eingespielt, wie er im schlimmsten Fall aussähe:
/// alle Namen nacheinander innerhalb eines Fensters. Das ist deutlich mehr
/// Verkehr, als ein Haushalt je erzeugt, und damit die harte Probe.
#[test]
#[ignore = "braucht einen Korpus"]
fn measure_tunneling_false_positives() {
    let threshold = permille(tunneling::DEFAULT_THRESHOLD);
    let domains = holdout_corpus();
    let clock = Arc::new(TestClock::new());
    let detector = tunneling::Tunneling::new(Duration::from_secs(300), &[], Arc::clone(&clock));

    let mut flagged: HashMap<String, Permille> = HashMap::new();
    let mut examined = 0_u64;
    for domain in &domains {
        // Ein Host unter der Domain, so wie ein Client wirklich fragt.
        let name = format!("www.{domain}");
        examined += 1;
        if let Some(finding) = detector.inspect(&Observation {
            name: &name,
            query_type: RecordType::A,
        }) && finding.score >= threshold
        {
            flagged.insert(domain.clone(), finding.score);
        }
    }

    let rate = flagged.len() as f64 / examined as f64;
    println!("\n=== Tunneling: Falsch-Positive auf dem Korpus ===");
    println!(
        "Schwelle:  {} ({:.2})",
        threshold,
        tunneling::DEFAULT_THRESHOLD
    );
    println!("geprüft:   {examined}");
    println!("geflaggt:  {} ({:.4} %)", flagged.len(), rate * 100.0);
    for (domain, score) in flagged.iter().take(20) {
        println!("  {score:>4}  {domain}");
    }

    assert!(
        rate < 0.001,
        "Falsch-Positiv-Rate {:.4} % über der Zusage von 0,1 %",
        rate * 100.0
    );
}

/// Was der Typosquat-Wächter auf gewöhnlichem Verkehr meldet.
///
/// Kein Abnahmekriterium der Roadmap, aber dieselbe Frage: eine Schutzliste,
/// die den halben Korpus meldet, wird abgeschaltet.
#[test]
#[ignore = "braucht einen Korpus"]
fn measure_typosquat_false_positives() {
    let protect = [
        "sparkasse.at".to_owned(),
        "id.austria.gv.at".to_owned(),
        "google.com".to_owned(),
        "paypal.com".to_owned(),
        "microsoft.com".to_owned(),
    ];
    let detector = alpendns::detect::typosquat::Typosquat::new(&protect);
    let domains = holdout_corpus();

    let mut flagged: Vec<(String, Permille, String)> = Vec::new();
    for domain in &domains {
        if let Some(finding) = detector.inspect(&Observation {
            name: domain,
            query_type: RecordType::A,
        }) {
            flagged.push((domain.clone(), finding.score, finding.reason.to_string()));
        }
    }

    println!("\n=== Typosquat: Meldungen auf dem Korpus ===");
    println!("Schutzliste: {} Domains", protect.len());
    println!("geprüft:     {}", domains.len());
    println!(
        "gemeldet:    {} ({:.4} %)",
        flagged.len(),
        flagged.len() as f64 / domains.len() as f64 * 100.0
    );
    for (domain, score, reason) in flagged.iter().take(40) {
        println!("  {score:>4}  {domain}  — {reason}");
    }

    // Die Originale dürfen nicht darunter sein.
    for protected in &protect {
        assert!(
            !flagged.iter().any(|(domain, _, _)| domain == protected),
            "{protected} wurde gegen sich selbst gemeldet"
        );
    }
}

/// Der Detektor, der am ehesten alles meldet: was findet er im Leerlauf?
#[test]
#[ignore = "braucht einen Korpus"]
fn measure_detector_agreement() {
    let domains = holdout_corpus();
    let dga = dga::Dga::new();
    let mut counts: HashMap<Detector, u64> = HashMap::new();

    for domain in &domains {
        let observation = Observation {
            name: domain,
            query_type: RecordType::A,
        };
        if dga
            .inspect(&observation)
            .is_some_and(|finding| finding.score >= permille(dga::DEFAULT_THRESHOLD))
        {
            *counts.entry(Detector::Dga).or_default() += 1;
        }
    }

    println!("\n=== Meldungen je Detektor auf dem Korpus ===");
    for detector in Detector::ALL {
        println!(
            "  {:>12}: {}",
            detector.as_str(),
            counts.get(&detector).copied().unwrap_or(0)
        );
    }
}
