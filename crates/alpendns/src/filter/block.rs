//! Antworten auf geblockte Anfragen.
//!
//! Es gibt keinen "richtigen" Weg, einen Namen zu blocken; jede Variante hat
//! einen sichtbaren Preis, und deshalb ist es eine Einstellung
//! (`blocking.mode`).

use std::net::{Ipv4Addr, Ipv6Addr};

use hickory_proto::op::{Message, ResponseCode};
use hickory_proto::rr::rdata::{A, AAAA};
use hickory_proto::rr::{RData, Record, RecordType};
use serde::Deserialize;

/// Wie lange ein Client eine Block-Antwort behalten soll.
///
/// Kurz gehalten, damit eine Änderung an Listen oder Allowlist schnell greift.
/// Länger wäre sparsamer, aber "ich habe es freigegeben und es geht immer noch
/// nicht" ist der teurere Fehler.
const BLOCK_TTL: u32 = 60;

/// Womit auf eine geblockte Anfrage geantwortet wird.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BlockMode {
    /// "Diesen Namen gibt es nicht." Der Client gibt sofort auf, und für ihn
    /// sieht es aus wie ein Tippfehler, nicht wie eine Sperre.
    #[default]
    Nxdomain,
    /// Eine Antwort mit 0.0.0.0 bzw. ::. Manche Clients versuchen daraufhin
    /// eine Verbindung dorthin und laufen in einen Timeout.
    ZeroIp,
    /// "Ich beantworte das nicht." Ehrlich, aber manche Clients fragen dann
    /// den nächsten Resolver in ihrer Liste — und der antwortet.
    Refused,
    /// Eine Antwort mit einer konfigurierten Adresse, auf der eine Erklärseite
    /// stehen kann. Für HTTPS bricht die Verbindung trotzdem ab, weil das
    /// Zertifikat nicht passt.
    Sinkhole,
}

/// Baut die Antwort auf eine geblockte Anfrage.
pub fn synthesize(
    request: &Message,
    mode: BlockMode,
    sinkhole_v4: Ipv4Addr,
    sinkhole_v6: Ipv6Addr,
) -> Message {
    let mut response = Message::response(request.metadata.id, request.metadata.op_code);
    response.metadata.recursion_desired = request.metadata.recursion_desired;
    response.metadata.recursion_available = true;
    response.add_queries(request.queries.iter().cloned());

    let (v4, v6) = match mode {
        BlockMode::Nxdomain => {
            response.metadata.response_code = ResponseCode::NXDomain;
            return response;
        }
        BlockMode::Refused => {
            response.metadata.response_code = ResponseCode::Refused;
            return response;
        }
        BlockMode::ZeroIp => (Ipv4Addr::UNSPECIFIED, Ipv6Addr::UNSPECIFIED),
        BlockMode::Sinkhole => (sinkhole_v4, sinkhole_v6),
    };

    // Auf alles außer A und AAAA gibt es NOERROR ohne Antwortsatz (NODATA).
    // Eine Adresse auf eine MX-Frage wäre schlicht falsch.
    if let Some(query) = request.queries.first() {
        let data = match query.query_type() {
            RecordType::A => Some(RData::A(A(v4))),
            RecordType::AAAA => Some(RData::AAAA(AAAA(v6))),
            _ => None,
        };
        if let Some(data) = data {
            response.add_answer(Record::from_rdata(query.name().clone(), BLOCK_TTL, data));
        }
    }
    response
}

#[cfg(test)]
mod tests {
    use super::*;
    use hickory_proto::op::{MessageType, OpCode, Query};
    use hickory_proto::rr::Name;

    fn ask(name: &str, record_type: RecordType) -> Message {
        let mut message = Message::new(0x99, MessageType::Query, OpCode::Query);
        message.metadata.recursion_desired = true;
        message.add_query(Query::query(
            Name::from_ascii(name).expect("gültiger Name"),
            record_type,
        ));
        message
    }

    fn block(request: &Message, mode: BlockMode) -> Message {
        synthesize(
            request,
            mode,
            Ipv4Addr::new(127, 0, 0, 1),
            Ipv6Addr::LOCALHOST,
        )
    }

    #[test]
    fn every_mode_answers_the_question_that_was_asked() {
        let request = ask("ads.example.com.", RecordType::A);
        for mode in [
            BlockMode::Nxdomain,
            BlockMode::ZeroIp,
            BlockMode::Refused,
            BlockMode::Sinkhole,
        ] {
            let response = block(&request, mode);
            assert_eq!(response.metadata.id, request.metadata.id, "{mode:?}");
            assert_eq!(response.queries, request.queries, "{mode:?}");
            assert_eq!(
                response.metadata.message_type,
                MessageType::Response,
                "{mode:?}"
            );
        }
    }

    #[test]
    fn nxdomain_mode_returns_nxdomain_without_records() {
        let response = block(&ask("ads.example.com.", RecordType::A), BlockMode::Nxdomain);
        assert_eq!(response.metadata.response_code, ResponseCode::NXDomain);
        assert!(response.answers.is_empty());
    }

    #[test]
    fn refused_mode_returns_refused_without_records() {
        let response = block(&ask("ads.example.com.", RecordType::A), BlockMode::Refused);
        assert_eq!(response.metadata.response_code, ResponseCode::Refused);
        assert!(response.answers.is_empty());
    }

    #[test]
    fn zero_ip_mode_answers_a_with_the_unspecified_address() {
        let response = block(&ask("ads.example.com.", RecordType::A), BlockMode::ZeroIp);
        assert_eq!(response.metadata.response_code, ResponseCode::NoError);
        let record = response.answers.first().expect("ein Record");
        assert_eq!(record.data, RData::A(A(Ipv4Addr::UNSPECIFIED)));
        assert_eq!(record.ttl, BLOCK_TTL);
    }

    #[test]
    fn zero_ip_mode_answers_aaaa_with_the_unspecified_address() {
        let response = block(
            &ask("ads.example.com.", RecordType::AAAA),
            BlockMode::ZeroIp,
        );
        assert_eq!(
            response.answers.first().map(|r| &r.data),
            Some(&RData::AAAA(AAAA(Ipv6Addr::UNSPECIFIED)))
        );
    }

    #[test]
    fn sinkhole_mode_answers_with_the_configured_address() {
        let response = block(&ask("ads.example.com.", RecordType::A), BlockMode::Sinkhole);
        assert_eq!(
            response.answers.first().map(|r| &r.data),
            Some(&RData::A(A(Ipv4Addr::new(127, 0, 0, 1))))
        );
    }

    #[test]
    fn other_record_types_get_nodata_not_an_address() {
        // Eine Adresse auf eine MX- oder TXT-Frage wäre eine falsche Antwort,
        // kein Block.
        for record_type in [RecordType::MX, RecordType::TXT, RecordType::SRV] {
            for mode in [BlockMode::ZeroIp, BlockMode::Sinkhole] {
                let response = block(&ask("ads.example.com.", record_type), mode);
                assert_eq!(
                    response.metadata.response_code,
                    ResponseCode::NoError,
                    "{record_type} / {mode:?}"
                );
                assert!(
                    response.answers.is_empty(),
                    "{record_type} / {mode:?} bekam einen Record"
                );
            }
        }
    }

    #[test]
    fn the_recursion_desired_flag_is_mirrored() {
        let mut request = ask("ads.example.com.", RecordType::A);
        request.metadata.recursion_desired = false;
        let response = block(&request, BlockMode::Nxdomain);
        assert!(!response.metadata.recursion_desired);
    }
}
