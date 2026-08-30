//! Die Rechnerei hinter der Upstream-Auswahl.
//!
//! Alles hier ist rein und ohne Netzwerk testbar. Wer die Auswahl *anwendet*,
//! ist [`super::pool::Pool`].

use std::hash::{Hash as _, Hasher as _};

use hickory_proto::rr::Name;

/// Die registrierbare Domain eines Namens — die Einheit, nach der
/// `split_by_zone` verteilt.
///
/// Bestimmt über die Public Suffix List. Die Näherung "letzte zwei Labels", mit
/// der Phase 3 gestartet ist, lieferte für `shop.example.co.uk` das
/// wirkungslose `co.uk`: sämtliche `.co.uk`-Namen landeten bei einem einzigen
/// Upstream. Keine Privacy-Lücke, aber eine schiefe Verteilung — und damit
/// genau das, was Phase 7, Schritt 1 messbar unter 5 % drücken soll.
///
/// Fällt die Liste nicht zu (einzelnes Label wie `localhost`, oder ein Name,
/// der nur aus einem Suffix besteht), gilt der Name selbst als Einheit. Das ist
/// die sichere Richtung: im Zweifel *weniger* aufteilen, damit nicht plötzlich
/// zwei Upstreams denselben Namensraum sehen.
pub fn registrable_domain(name: &Name) -> String {
    let ascii = name.to_ascii();
    let text = ascii.trim_end_matches('.').to_ascii_lowercase();
    if text.is_empty() {
        return String::new();
    }
    psl::domain_str(&text).map_or(text.clone(), ToOwned::to_owned)
}

/// Welcher von `count` Upstreams für diesen Namen zuständig ist.
///
/// Derselbe Name geht immer zum selben Upstream, solange der Seed gleich bleibt.
/// Der Seed wird beim Start zufällig gezogen und danach im konfigurierten
/// Abstand neu gewürfelt: kein Anbieter lernt über die Zeit ein stabiles Bild
/// (FEATURES.md P2).
pub fn zone_index(seed: u64, name: &Name, count: usize) -> usize {
    if count <= 1 {
        return 0;
    }
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    seed.hash(&mut hasher);
    registrable_domain(name).hash(&mut hasher);
    // count ist hier > 1, der Rest ist immer kleiner.
    usize::try_from(hasher.finish() % count as u64).unwrap_or(0)
}

/// Wie sich eine Menge von Namen auf `count` Upstreams verteilt.
///
/// Das Messwerkzeug zu Phase 7, Schritt 1. Es steht hier und nicht im Test,
/// weil die Frage "wie schief ist die Verteilung gerade" auch außerhalb eines
/// Tests eine sinnvolle ist — und weil eine Kennzahl, die nur im Testcode
/// existiert, beim nächsten Umbau still verschwindet.
pub fn distribution<'a>(
    seed: u64,
    names: impl IntoIterator<Item = &'a Name>,
    count: usize,
) -> Vec<u64> {
    let mut buckets = vec![0_u64; count];
    if count == 0 {
        return buckets;
    }
    for name in names {
        if let Some(slot) = buckets.get_mut(zone_index(seed, name, count)) {
            *slot = slot.saturating_add(1);
        }
    }
    buckets
}

/// Die größte Abweichung eines Upstreams vom gleichen Anteil, als Bruchteil.
///
/// `0.05` heißt: der am stärksten belastete oder am wenigsten belastete
/// Upstream liegt 5 % neben `1/n`. Das ist die Zahl aus dem Abnahmekriterium.
/// Ohne Namen ist sie 0 — nichts zu verteilen ist nicht schief.
pub fn max_deviation(buckets: &[u64]) -> f64 {
    let count = buckets.len();
    let total: u64 = buckets.iter().sum();
    if count == 0 || total == 0 {
        return 0.0;
    }
    #[expect(
        clippy::cast_precision_loss,
        reason = "Zählwerte einer Messreihe liegen weit unter der Genauigkeitsgrenze von f64"
    )]
    let expected = total as f64 / count as f64;
    buckets
        .iter()
        .map(|&value| {
            #[expect(clippy::cast_precision_loss, reason = "siehe oben")]
            let value = value as f64;
            (value - expected).abs() / expected
        })
        .fold(0.0_f64, f64::max)
}

/// Reihenfolge reihum, beginnend beim `start`-ten Eintrag.
///
/// Keine Strategie für sich — nur die Hilfsfunktion, die [`by_zone`] die
/// Ausweichwege hinter den zuständigen Upstream hängt.
pub fn round_robin(start: usize, count: usize) -> Vec<usize> {
    if count == 0 {
        return Vec::new();
    }
    (0..count)
        .map(|i| (start.wrapping_add(i)) % count)
        .collect()
}

/// Reihenfolge für `split_by_zone`: der zuständige Upstream zuerst, der Rest
/// dahinter als Ausweichweg.
pub fn by_zone(seed: u64, name: &Name, count: usize) -> Vec<usize> {
    if count == 0 {
        return Vec::new();
    }
    let first = zone_index(seed, name, count);
    round_robin(first, count)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn name(text: &str) -> Name {
        Name::from_ascii(text).expect("gültiger Name")
    }

    #[test]
    fn registrable_domain_strips_everything_below_the_registrable_label() {
        assert_eq!(registrable_domain(&name("www.example.com.")), "example.com");
        assert_eq!(registrable_domain(&name("example.com.")), "example.com");
        assert_eq!(
            registrable_domain(&name("a.b.c.d.example.com.")),
            "example.com"
        );
    }

    #[test]
    fn a_multi_label_public_suffix_is_not_the_registrable_domain() {
        // Der Grund für die Public Suffix List. Mit der alten Näherung "letzte
        // zwei Labels" kam hier "co.uk" heraus, und damit landete das halbe
        // britische Internet bei einem einzigen Upstream.
        assert_eq!(
            registrable_domain(&name("shop.example.co.uk.")),
            "example.co.uk"
        );
        assert_eq!(
            registrable_domain(&name("www.example.ac.at.")),
            "example.ac.at"
        );
        assert_eq!(
            registrable_domain(&name("a.b.example.com.au.")),
            "example.com.au"
        );
    }

    #[test]
    fn two_sites_under_the_same_public_suffix_stay_apart() {
        // Die Kehrseite: nach der Korrektur sind das zwei verschiedene
        // Einheiten und dürfen zu verschiedenen Upstreams gehen.
        assert_ne!(
            registrable_domain(&name("www.eins.co.uk.")),
            registrable_domain(&name("www.zwei.co.uk."))
        );
    }

    #[test]
    fn a_name_that_is_only_a_public_suffix_stays_whole() {
        // Kein registrierbarer Teil vorhanden. Dann ist der Name selbst die
        // Einheit — im Zweifel weniger aufteilen, nicht mehr.
        assert_eq!(registrable_domain(&name("co.uk.")), "co.uk");
        assert_eq!(registrable_domain(&name("com.")), "com");
    }

    #[test]
    fn registrable_domain_is_case_insensitive() {
        assert_eq!(
            registrable_domain(&name("WWW.Example.COM.")),
            registrable_domain(&name("www.example.com."))
        );
    }

    #[test]
    fn registrable_domain_handles_short_names() {
        assert_eq!(registrable_domain(&name("localhost.")), "localhost");
        assert_eq!(registrable_domain(&Name::root()), "");
    }

    #[test]
    fn the_same_domain_always_goes_to_the_same_upstream() {
        let seed = 0x5eed;
        let first = zone_index(seed, &name("www.example.com."), 3);
        for _ in 0..100 {
            assert_eq!(zone_index(seed, &name("www.example.com."), 3), first);
        }
    }

    #[test]
    fn subdomains_share_the_upstream_of_their_registrable_domain() {
        // Der eigentliche Zweck: ein Upstream sieht example.com ganz oder gar
        // nicht, statt jeden Host einzeln.
        let seed = 42;
        let expected = zone_index(seed, &name("example.com."), 4);
        for host in ["www.example.com.", "mail.example.com.", "a.b.example.com."] {
            assert_eq!(zone_index(seed, &name(host), 4), expected, "{host}");
        }
    }

    #[test]
    fn a_different_seed_gives_a_different_distribution() {
        // Nach einem Neustart sieht jeder Upstream einen anderen Ausschnitt.
        let domains: Vec<Name> = (0..200)
            .map(|i| name(&format!("domain{i}.example.")))
            .collect();
        let with_seed =
            |seed: u64| -> Vec<usize> { domains.iter().map(|d| zone_index(seed, d, 3)).collect() };
        assert_ne!(with_seed(1), with_seed(2), "Seed ändert nichts");
    }

    #[test]
    fn zone_index_stays_in_range_and_uses_every_upstream() {
        let seed = 7;
        let mut seen = [false; 3];
        for i in 0..500 {
            let index = zone_index(seed, &name(&format!("d{i}.example.")), 3);
            assert!(index < 3, "Index {index} außerhalb");
            if let Some(slot) = seen.get_mut(index) {
                *slot = true;
            }
        }
        assert!(
            seen.iter().all(|&used| used),
            "ein Upstream blieb ungenutzt"
        );
    }

    /// 10 000 Domains, gemischt über ein- und mehrteilige Public Suffixes.
    ///
    /// Genau das Korpus, an dem die alte Näherung scheiterte: ein Fünftel der
    /// Namen steht unter einem zweiteiligen Suffix.
    fn corpus(size: usize) -> Vec<Name> {
        const SUFFIXES: [&str; 10] = [
            "com", "net", "org", "at", "de", "co.uk", "com.au", "ac.at", "co.jp", "or.jp",
        ];
        (0..size)
            .map(|i| {
                let suffix = SUFFIXES
                    .get(i % SUFFIXES.len())
                    .copied()
                    .unwrap_or("example");
                name(&format!("host{i}.site{i}.{suffix}."))
            })
            .collect()
    }

    #[test]
    fn ten_thousand_domains_stay_within_five_percent_per_upstream() {
        // Das Abnahmekriterium aus Phase 7, Schritt 1. Über mehrere Seeds
        // geprüft, damit nicht ein einzelner glücklicher Wert die Zusage trägt.
        let domains = corpus(10_000);
        for count in [2, 3, 4, 5] {
            for seed in [0_u64, 1, 42, 0x5eed, u64::MAX, 0xdead_beef, 7, 99] {
                let buckets = distribution(seed, domains.iter(), count);
                assert_eq!(
                    buckets.iter().sum::<u64>(),
                    domains.len() as u64,
                    "eine Domain ging verloren"
                );
                let deviation = max_deviation(&buckets);
                assert!(
                    deviation < 0.05,
                    "Seed {seed}, {count} Upstreams: Abweichung {:.2} % — {buckets:?}",
                    deviation * 100.0
                );
            }
        }
    }

    #[test]
    fn a_multi_label_suffix_no_longer_lands_on_one_upstream() {
        // Die Gegenprobe zum Test darüber: nur .co.uk-Namen. Mit der alten
        // Näherung wäre die Abweichung 100 % gewesen, weil "co.uk" für alle
        // derselbe Hashwert ist.
        let domains: Vec<Name> = (0..2000)
            .map(|i| name(&format!("www.site{i}.co.uk.")))
            .collect();
        let buckets = distribution(0x5eed, domains.iter(), 3);
        assert!(
            max_deviation(&buckets) < 0.05,
            "Verteilung schief: {buckets:?}"
        );
    }

    #[test]
    fn the_deviation_of_an_empty_measurement_is_zero() {
        assert!((max_deviation(&[]) - 0.0).abs() < f64::EPSILON);
        assert!((max_deviation(&[0, 0, 0]) - 0.0).abs() < f64::EPSILON);
    }

    #[test]
    fn a_completely_skewed_distribution_is_reported_as_such() {
        // Die Kennzahl muss auch anschlagen können, sonst prüft der Test
        // darüber nichts. Drei Upstreams, alles bei einem: der leere liegt
        // 100 % unter dem Erwartungswert, der volle 200 % darüber.
        let deviation = max_deviation(&[300, 0, 0]);
        assert!(deviation > 1.9, "{deviation}");
    }

    #[test]
    fn a_single_upstream_is_always_index_zero() {
        assert_eq!(zone_index(1, &name("example.com."), 1), 0);
        assert_eq!(zone_index(1, &name("example.com."), 0), 0);
    }

    #[test]
    fn round_robin_walks_through_everyone_once() {
        assert_eq!(round_robin(0, 3), vec![0, 1, 2]);
        assert_eq!(round_robin(1, 3), vec![1, 2, 0]);
        assert_eq!(round_robin(2, 3), vec![2, 0, 1]);
        assert_eq!(round_robin(5, 3), vec![2, 0, 1], "Start jenseits der Länge");
        assert!(round_robin(0, 0).is_empty());
    }

    #[test]
    fn zone_order_keeps_the_others_as_fallback() {
        let order = by_zone(1, &name("example.com."), 3);
        assert_eq!(order.len(), 3, "Ausweichwege fehlen");
        let mut sorted = order.clone();
        sorted.sort_unstable();
        assert_eq!(sorted, vec![0, 1, 2], "ein Upstream kommt doppelt vor");
    }
}
