//! Wer fragt — und welche Policy für ihn gilt.
//!
//! In Phase 5 gibt es genau einen Weg der Identifikation: die Quelladresse.
//! mTLS-Zertifikate und DoH-Pfad-Token sind in ARCHITECTURE.md §6 vorgesehen und
//! brauchen verschlüsselte *Listener*, die es noch nicht gibt.

use std::net::IpAddr;
use std::sync::Arc;

use ipnet::IpNet;

use crate::trace::MatchKind;

/// Ein konfigurierter Client.
#[derive(Debug, Clone)]
pub struct ClientRule {
    pub name: Arc<str>,
    /// Einzeladressen sind hier Netze mit voller Präfixlänge.
    pub nets: Vec<IpNet>,
    pub policy: Arc<str>,
}

/// Wem welche Policy gehört.
#[derive(Debug)]
pub struct Clients {
    /// Absteigend nach Präfixlänge sortiert: die genaueste Regel gewinnt.
    rules: Vec<ClientRule>,
    default_policy: Arc<str>,
}

/// Ergebnis der Identifikation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Identity {
    pub client: Arc<str>,
    pub policy: Arc<str>,
    pub by: MatchKind,
}

impl Clients {
    pub fn new(mut rules: Vec<ClientRule>, default_policy: Arc<str>) -> Self {
        // Innerhalb einer Regel zuerst das genaueste Netz, damit der Vergleich
        // unten die richtige Länge sieht.
        for rule in &mut rules {
            rule.nets
                .sort_by_key(|net| std::cmp::Reverse(net.prefix_len()));
        }
        // 10.0.10.42/32 muss vor 10.0.0.0/16 geprüft werden, sonst gewinnt die
        // Hausregel über die Ausnahme.
        rules.sort_by_key(|rule| std::cmp::Reverse(rule.nets.first().map_or(0, IpNet::prefix_len)));
        Self {
            rules,
            default_policy,
        }
    }

    /// Findet den Client zu einer Adresse, sonst die Default-Policy.
    pub fn identify(&self, addr: IpAddr) -> Identity {
        for rule in &self.rules {
            if rule.nets.iter().any(|net| net.contains(&addr)) {
                return Identity {
                    client: Arc::clone(&rule.name),
                    policy: Arc::clone(&rule.policy),
                    by: MatchKind::Address,
                };
            }
        }
        Identity {
            client: Arc::from("default"),
            policy: Arc::clone(&self.default_policy),
            by: MatchKind::Default,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn net(text: &str) -> IpNet {
        text.parse().expect("gültiges Netz")
    }

    fn addr(text: &str) -> IpAddr {
        text.parse().expect("gültige Adresse")
    }

    fn rule(name: &str, nets: &[&str], policy: &str) -> ClientRule {
        ClientRule {
            name: Arc::from(name),
            nets: nets.iter().map(|n| net(n)).collect(),
            policy: Arc::from(policy),
        }
    }

    #[test]
    fn a_single_address_is_matched() {
        let clients = Clients::new(
            vec![rule("tablet", &["10.0.10.42/32"], "kids")],
            Arc::from("default"),
        );
        let found = clients.identify(addr("10.0.10.42"));
        assert_eq!(&*found.client, "tablet");
        assert_eq!(&*found.policy, "kids");
        assert_eq!(found.by, MatchKind::Address);
    }

    #[test]
    fn an_unknown_address_gets_the_default_policy() {
        let clients = Clients::new(
            vec![rule("tablet", &["10.0.10.42/32"], "kids")],
            Arc::from("default"),
        );
        let found = clients.identify(addr("10.0.10.43"));
        assert_eq!(&*found.policy, "default");
        assert_eq!(found.by, MatchKind::Default);
    }

    #[test]
    fn a_subnet_matches_every_address_in_it() {
        let clients = Clients::new(
            vec![rule("gaeste", &["192.168.5.0/24"], "restriktiv")],
            Arc::from("default"),
        );
        for text in ["192.168.5.1", "192.168.5.200", "192.168.5.255"] {
            assert_eq!(
                &*clients.identify(addr(text)).policy,
                "restriktiv",
                "{text}"
            );
        }
        assert_eq!(&*clients.identify(addr("192.168.6.1")).policy, "default");
    }

    #[test]
    fn the_most_specific_rule_wins() {
        // Ein Gerät im Gästenetz, für das eine Ausnahme gilt: die Einzeladresse
        // muss das Netz schlagen, egal in welcher Reihenfolge konfiguriert.
        let clients = Clients::new(
            vec![
                rule("gaeste", &["192.168.5.0/24"], "restriktiv"),
                rule("drucker", &["192.168.5.10/32"], "offen"),
            ],
            Arc::from("default"),
        );
        assert_eq!(&*clients.identify(addr("192.168.5.10")).policy, "offen");
        assert_eq!(
            &*clients.identify(addr("192.168.5.11")).policy,
            "restriktiv"
        );
    }

    #[test]
    fn ipv6_works_the_same_way() {
        let clients = Clients::new(
            vec![rule("laptop", &["fd00::/8"], "default")],
            Arc::from("streng"),
        );
        assert_eq!(&*clients.identify(addr("fd00::1")).policy, "default");
        assert_eq!(&*clients.identify(addr("2001:db8::1")).policy, "streng");
    }

    #[test]
    fn a_client_can_have_several_networks() {
        let clients = Clients::new(
            vec![rule("laptop", &["10.0.1.5/32", "fd00::5/128"], "mobil")],
            Arc::from("default"),
        );
        assert_eq!(&*clients.identify(addr("10.0.1.5")).policy, "mobil");
        assert_eq!(&*clients.identify(addr("fd00::5")).policy, "mobil");
    }

    #[test]
    fn without_rules_everything_is_default() {
        let clients = Clients::new(Vec::new(), Arc::from("default"));
        assert_eq!(&*clients.identify(addr("10.0.0.1")).client, "default");
    }
}
