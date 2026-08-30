//! Die Rechnerei hinter der Upstream-Auswahl.
//!
//! Alles hier ist rein und ohne Netzwerk testbar. Wer die Auswahl *anwendet*,
//! ist [`super::pool::Pool`].

use std::hash::{Hash as _, Hasher as _};

use hickory_proto::rr::Name;

/// Die registrierbare Domain eines Namens — die Einheit, nach der
/// `split_by_zone` verteilt.
///
/// **Näherung:** die letzten beiden Labels. Für `www.example.com` ist das
/// `example.com` und damit richtig; für `shop.example.co.uk` liefert es `co.uk`
/// und damit zu grob. Die Folge ist keine Privacy-Lücke — alle `.co.uk`-Namen
/// landen beim selben Upstream, statt sich zu verteilen — sondern eine
/// ungleichmäßige Verteilung. Die saubere Lösung braucht die Public Suffix List;
/// sie ist als Phase 7, Schritt 1 eingeplant ("Verteilung messen, Abweichung
/// unter 5 %"). Bis dahin wäre ein weiteres Dependency mit eigener Datenpflege
/// zu früh.
pub fn registrable_domain(name: &Name) -> String {
    let labels: Vec<&[u8]> = name.iter().collect();
    let take = labels.len().min(2);
    let start = labels.len().saturating_sub(take);
    labels
        .get(start..)
        .unwrap_or_default()
        .iter()
        .map(|label| String::from_utf8_lossy(label).to_lowercase())
        .collect::<Vec<_>>()
        .join(".")
}

/// Welcher von `count` Upstreams für diesen Namen zuständig ist.
///
/// Derselbe Name geht immer zum selben Upstream, solange der Seed gleich bleibt.
/// Der Seed wird beim Start zufällig gezogen: nach einem Neustart sieht jeder
/// Upstream einen anderen Ausschnitt.
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
    fn registrable_domain_takes_the_last_two_labels() {
        assert_eq!(registrable_domain(&name("www.example.com.")), "example.com");
        assert_eq!(registrable_domain(&name("example.com.")), "example.com");
        assert_eq!(
            registrable_domain(&name("a.b.c.d.example.com.")),
            "example.com"
        );
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
