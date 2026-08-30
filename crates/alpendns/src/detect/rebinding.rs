//! DNS-Rebinding: private Adressen als Antwort auf öffentliche Namen.
//!
//! Der Angriff ist alt und wirkt weiter: eine Webseite lässt den Browser einen
//! Namen auflösen, den sie kontrolliert, und bekommt dafür `192.168.1.1`
//! geliefert. Von da an gilt für den Browser die Seite und der Router als
//! *derselbe* Ursprung, und das JavaScript der Seite darf mit dem Router
//! reden — an der Same-Origin-Policy vorbei.
//!
//! **Das ist der einzige Detektor in diesem Modul, der keine Heuristik ist.**
//! Er liefert kein abgestuftes Urteil, sondern eine Ja-Nein-Antwort: entweder
//! steht in der Antwort eine private Adresse für einen öffentlichen Namen oder
//! nicht. Deshalb ist sein Score immer 1.000, und deshalb hat er in der
//! Konfiguration eine Ausnahmeliste statt einer Schwelle.
//!
//! **Die Ausnahmeliste ist nicht optional.** Ein Split-Horizon-DNS, eine
//! Weboberfläche unter einem echten Namen, ein Gerätehersteller, der
//! `geraet.hersteller.example` auf `192.168.1.50` zeigen lässt — alles legitim
//! und alles hier ein Treffer. `forward_zone`-Einträge trägt die
//! Konfiguration von selbst ein (siehe `crate::config`), damit der eigene
//! LAN-Nameserver den Schutz nicht beim ersten Start auslöst.

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

use hickory_proto::op::Message;
use hickory_proto::rr::{Name, RData};

use super::{AnswerDetector, Detector, Finding};

/// Prüft Antworten auf private Adressen.
#[derive(Debug)]
pub struct Rebinding {
    /// Zonen, für die private Adressen erlaubt sind. Absolut und klein.
    allowed: Vec<Name>,
}

impl Rebinding {
    pub fn new(allow_zones: Vec<Name>) -> Self {
        Self {
            allowed: allow_zones
                .into_iter()
                .map(|zone| {
                    let mut zone = zone.to_lowercase();
                    zone.set_fqdn(true);
                    zone
                })
                .collect(),
        }
    }

    /// Ob dieser Name von der Prüfung ausgenommen ist.
    fn is_allowed(&self, name: &Name) -> bool {
        self.allowed.iter().any(|zone| zone.zone_of(name))
    }
}

/// Ob diese Adresse in einer Antwort auf einen öffentlichen Namen nichts zu
/// suchen hat.
///
/// Von Hand aufgezählt statt über die Hilfsmethoden der Standardbibliothek:
/// `Ipv4Addr::is_shared` und `is_benchmarking` sind dort noch nicht stabil, und
/// eine Liste, die man lesen kann, ist an dieser Stelle mehr wert als eine, die
/// kürzer ist. Jede Zeile nennt das RFC, aus dem sie stammt.
pub fn is_private(addr: IpAddr) -> bool {
    match addr {
        IpAddr::V4(v4) => is_private_v4(v4),
        // Eine IPv4-Adresse, die als IPv6 verpackt ist, ist dieselbe Adresse.
        // `::ffff:192.168.1.1` wäre sonst der offene Seiteneingang.
        IpAddr::V6(v6) => v6
            .to_ipv4_mapped()
            .map_or_else(|| is_private_v6(v6), is_private_v4),
    }
}

fn is_private_v4(addr: Ipv4Addr) -> bool {
    let [a, b, ..] = addr.octets();
    addr.is_unspecified()          // 0.0.0.0/8      RFC 1122
        || addr.is_loopback()      // 127.0.0.0/8    RFC 1122
        || addr.is_private()       // 10/8, 172.16/12, 192.168/16  RFC 1918
        || addr.is_link_local()    // 169.254.0.0/16 RFC 3927
        || addr.is_broadcast()     // 255.255.255.255
        || (a == 100 && (64..128).contains(&b))  // 100.64/10  RFC 6598 (CGNAT)
        || (a == 192 && b == 0)    // 192.0.0/24     RFC 6890 (IETF-Protokolle)
        || (a == 198 && (18..20).contains(&b)) // 198.18/15 RFC 2544 (Benchmark)
}

fn is_private_v6(addr: Ipv6Addr) -> bool {
    let first = addr.segments().first().copied().unwrap_or(0);
    addr.is_unspecified()                 // ::            RFC 4291
        || addr.is_loopback()             // ::1           RFC 4291
        || (first & 0xfe00) == 0xfc00     // fc00::/7      RFC 4193 (unique local)
        || (first & 0xffc0) == 0xfe80 // fe80::/10     RFC 4291 (link local)
}

impl AnswerDetector for Rebinding {
    fn detector(&self) -> Detector {
        Detector::Rebinding
    }

    fn inspect(&self, name: &str, response: &Message) -> Option<Finding> {
        let parsed = Name::from_str_relaxed(name).ok()?;
        if self.is_allowed(&parsed) {
            return None;
        }

        // Auch der Authority- und Additional-Abschnitt zählt: eine private
        // Adresse als Glue-Record erreicht den Client genauso.
        let offending = response
            .answers
            .iter()
            .chain(response.authorities.iter())
            .chain(response.additionals.iter())
            .find_map(|record| match &record.data {
                RData::A(a) => is_private(IpAddr::V4(a.0)).then_some(IpAddr::V4(a.0)),
                RData::AAAA(aaaa) => is_private(IpAddr::V6(aaaa.0)).then_some(IpAddr::V6(aaaa.0)),
                _ => None,
            })?;

        Some(Finding::new(
            Detector::Rebinding,
            1000,
            format!(
                "Antwort auf den öffentlichen Namen '{name}' enthält die private \
                 Adresse {offending}"
            ),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use hickory_proto::op::{OpCode, Query};
    use hickory_proto::rr::rdata::{A, AAAA};
    use hickory_proto::rr::{Record, RecordType};

    fn name(text: &str) -> Name {
        Name::from_ascii(text).expect("gültiger Name")
    }

    fn answer(query: &str, addresses: &[IpAddr]) -> Message {
        let mut message = Message::response(1, OpCode::Query);
        message.add_query(Query::query(name(query), RecordType::A));
        for address in addresses {
            let data = match address {
                IpAddr::V4(v4) => RData::A(A(*v4)),
                IpAddr::V6(v6) => RData::AAAA(AAAA(*v6)),
            };
            message
                .answers
                .push(Record::from_rdata(name(query), 60, data));
        }
        message
    }

    fn detector() -> Rebinding {
        Rebinding::new(vec![name("home.arpa.")])
    }

    #[test]
    fn a_private_address_for_a_public_name_is_found() {
        for address in [
            "192.168.1.1",
            "10.0.0.5",
            "172.16.0.1",
            "127.0.0.1",
            "169.254.1.1",
            "0.0.0.0",
            "100.64.0.1",
        ] {
            let parsed: IpAddr = address.parse().expect("Adresse");
            let response = answer("boese.example.com.", &[parsed]);
            let finding = detector()
                .inspect("boese.example.com", &response)
                .unwrap_or_else(|| panic!("{address} nicht erkannt"));
            assert_eq!(finding.score, 1000);
            assert!(finding.reason.contains(address), "{}", finding.reason);
        }
    }

    #[test]
    fn an_ipv4_address_wrapped_as_ipv6_is_the_same_address() {
        // Der offene Seiteneingang, wenn man nur `Ipv6Addr::is_loopback` prüft.
        let mapped: IpAddr = "::ffff:192.168.1.1".parse().expect("Adresse");
        let response = answer("boese.example.com.", &[mapped]);
        assert!(detector().inspect("boese.example.com", &response).is_some());
    }

    #[test]
    fn private_ipv6_is_found_too() {
        for address in ["::1", "fd00::1", "fe80::1", "::"] {
            let parsed: IpAddr = address.parse().expect("Adresse");
            let response = answer("boese.example.com.", &[parsed]);
            assert!(
                detector().inspect("boese.example.com", &response).is_some(),
                "{address} nicht erkannt"
            );
        }
    }

    #[test]
    fn a_public_address_is_not_a_finding() {
        for address in [
            "93.184.216.34",
            "8.8.8.8",
            "2606:2800:220:1:248:1893:25c8:1946",
        ] {
            let parsed: IpAddr = address.parse().expect("Adresse");
            let response = answer("example.com.", &[parsed]);
            assert!(
                detector().inspect("example.com", &response).is_none(),
                "{address} fälschlich erkannt"
            );
        }
    }

    #[test]
    fn the_allow_list_works_and_covers_subdomains() {
        // Das Abnahmekriterium aus Schritt 2. Ohne diese Ausnahme wäre der
        // eigene LAN-Nameserver ab dem ersten Start unbrauchbar.
        let response = answer(
            "drucker.home.arpa.",
            &["10.0.0.7".parse().expect("Adresse")],
        );
        assert!(detector().inspect("drucker.home.arpa", &response).is_none());

        let deep = answer(
            "a.b.c.home.arpa.",
            &["192.168.1.9".parse().expect("Adresse")],
        );
        assert!(detector().inspect("a.b.c.home.arpa", &deep).is_none());

        // Ein Name, der nur so *aussieht*, als stünde er unter der Zone.
        let lookalike = answer(
            "home.arpa.boese.example.",
            &["10.0.0.7".parse().expect("Adresse")],
        );
        assert!(
            detector()
                .inspect("home.arpa.boese.example", &lookalike)
                .is_some(),
            "die Ausnahmeliste greift zu weit"
        );
    }

    #[test]
    fn the_allow_list_is_case_insensitive() {
        let response = answer(
            "Drucker.HOME.arpa.",
            &["10.0.0.7".parse().expect("Adresse")],
        );
        assert!(detector().inspect("drucker.home.arpa", &response).is_none());
    }

    #[test]
    fn one_private_address_among_public_ones_is_enough() {
        // Der typische Angriff liefert beides, damit die Seite auch normal lädt.
        let response = answer(
            "boese.example.com.",
            &[
                "93.184.216.34".parse().expect("Adresse"),
                "192.168.1.1".parse().expect("Adresse"),
            ],
        );
        assert!(detector().inspect("boese.example.com", &response).is_some());
    }

    #[test]
    fn a_private_address_in_the_additional_section_counts() {
        let mut response = answer("boese.example.com.", &[]);
        response.additionals.push(Record::from_rdata(
            name("ns.boese.example.com."),
            60,
            RData::A(A(Ipv4Addr::new(192, 168, 0, 1))),
        ));
        assert!(detector().inspect("boese.example.com", &response).is_some());
    }

    #[test]
    fn an_answer_without_addresses_is_not_a_finding() {
        let response = answer("example.com.", &[]);
        assert!(detector().inspect("example.com", &response).is_none());
    }

    #[test]
    fn every_documented_range_is_covered() {
        // Die Liste in `is_private_v4` ist die Zusicherung; dieser Test hält
        // fest, dass sie vollständig gelesen wird.
        let private = [
            "0.0.0.0",
            "10.255.255.255",
            "100.64.0.0",
            "100.127.255.255",
            "127.0.0.1",
            "169.254.0.1",
            "172.31.255.255",
            "192.0.0.1",
            "192.168.0.1",
            "198.19.255.255",
            "255.255.255.255",
        ];
        for address in private {
            let parsed: Ipv4Addr = address.parse().expect("Adresse");
            assert!(is_private_v4(parsed), "{address} gilt als öffentlich");
        }
        // Direkt neben den Grenzen liegt öffentlicher Adressraum.
        for address in [
            "100.63.255.255",
            "100.128.0.0",
            "198.17.255.255",
            "198.20.0.0",
        ] {
            let parsed: Ipv4Addr = address.parse().expect("Adresse");
            assert!(!is_private_v4(parsed), "{address} gilt als privat");
        }
    }
}
