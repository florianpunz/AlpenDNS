//! Reine Funktionen auf DNS-Nachrichten: Validieren, Zuschneiden, Fehlerantworten.
//!
//! Alles hier ist ohne Netzwerk und ohne Laufzeit testbar. Das Wire-Format selbst
//! kommt aus `hickory-proto` (ADR-0002).

use hickory_proto::op::{Message, MessageType, OpCode, ResponseCode};

/// Warum eine Upstream-Antwort verworfen wurde.
///
/// Jede Variante ist ein möglicher Spoofing-Versuch und wird später eine eigene
/// Metrik bekommen (ARCHITECTURE.md §10).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mismatch {
    /// Die Query-ID passt nicht zur gestellten Frage.
    Id,
    /// Die Nachricht ist gar keine Antwort (QR-Bit nicht gesetzt).
    NotAResponse,
    /// Die Anzahl der Fragen weicht ab.
    QuestionCount,
    /// Der Name in der Frage weicht ab (case-insensitiv verglichen).
    Name,
    /// Der Record-Typ der Frage weicht ab.
    QueryType,
    /// Die Klasse der Frage weicht ab.
    QueryClass,
}

/// Prüft eine Upstream-Antwort gegen die Frage, die wir gestellt haben.
///
/// Ohne diese Prüfung ist jede Antwort, die schnell genug am Socket ankommt, eine
/// gültige Antwort — das ist Cache-Poisoning per Off-Path-Angriff (THREAT-MODEL A3).
/// Der Namensvergleich ist bewusst case-insensitiv: DNS-Namen sind es laut RFC 1035
/// §2.3.3, und ein Upstream darf die Schreibweise ändern. Die case-*sensitive*
/// Prüfung für 0x20 kommt in Phase 3 zusätzlich dazu, sie ersetzt diese nicht.
pub fn check_response(request: &Message, response: &Message) -> Result<(), Mismatch> {
    if response.metadata.id != request.metadata.id {
        return Err(Mismatch::Id);
    }
    if response.metadata.message_type != MessageType::Response {
        return Err(Mismatch::NotAResponse);
    }
    if response.queries.len() != request.queries.len() {
        return Err(Mismatch::QuestionCount);
    }
    for (asked, echoed) in request.queries.iter().zip(response.queries.iter()) {
        // `Name: PartialEq` ist in hickory-proto case-insensitiv.
        if asked.name() != echoed.name() {
            return Err(Mismatch::Name);
        }
        if asked.query_type() != echoed.query_type() {
            return Err(Mismatch::QueryType);
        }
        if asked.query_class() != echoed.query_class() {
            return Err(Mismatch::QueryClass);
        }
    }
    Ok(())
}

/// Baut eine Fehlerantwort auf eine Anfrage, die wir noch parsen konnten.
pub fn error_response(request: &Message, code: ResponseCode) -> Message {
    let mut response = Message::error_msg(request.metadata.id, request.metadata.op_code, code);
    response.metadata.recursion_desired = request.metadata.recursion_desired;
    response.add_queries(request.queries.iter().cloned());
    response
}

/// Baut eine FORMERR-Antwort auf Bytes, die sich nicht parsen ließen.
///
/// Ohne geparste Nachricht kennen wir nur die ersten beiden Bytes als Query-ID.
/// Ist nicht einmal die da, gibt es nichts, worauf man antworten könnte, und das
/// Paket wird verworfen (ARCHITECTURE.md §10).
pub fn format_error(raw: &[u8]) -> Option<Message> {
    let id = raw.get(..2).and_then(|b| <[u8; 2]>::try_from(b).ok())?;
    Some(Message::error_msg(
        u16::from_be_bytes(id),
        OpCode::Query,
        ResponseCode::FormErr,
    ))
}

/// Serialisiert eine Antwort für UDP und schneidet sie zu, wenn sie nicht passt.
///
/// Passt die Antwort nicht in `max_size`, wird sie nach RFC 1035 §4.1.1 geleert
/// und mit gesetztem TC-Flag zurückgegeben — der Client wiederholt die Anfrage
/// dann über TCP.
pub fn encode_for_udp(
    response: &Message,
    max_size: usize,
) -> Result<Vec<u8>, hickory_proto::ProtoError> {
    let bytes = response.to_vec()?;
    if bytes.len() <= max_size {
        return Ok(bytes);
    }
    response.truncate().to_vec()
}

#[cfg(test)]
mod tests {
    use super::*;
    use hickory_proto::op::Query;
    use hickory_proto::rr::rdata::TXT;
    use hickory_proto::rr::{DNSClass, Name, RData, Record, RecordType};

    fn request() -> Message {
        let mut msg = Message::new(0x1234, MessageType::Query, OpCode::Query);
        msg.add_query(Query::query(
            Name::from_ascii("example.com.").expect("gültiger Name"),
            RecordType::A,
        ));
        msg
    }

    fn response_to(request: &Message) -> Message {
        let mut msg = Message::response(request.metadata.id, OpCode::Query);
        msg.add_queries(request.queries.iter().cloned());
        msg
    }

    #[test]
    fn matching_response_is_accepted() {
        let req = request();
        assert_eq!(check_response(&req, &response_to(&req)), Ok(()));
    }

    #[test]
    fn different_case_in_qname_is_accepted() {
        // Der Upstream darf die Schreibweise ändern; das ist keine Fälschung.
        let req = request();
        let mut res = response_to(&req);
        res.queries = vec![Query::query(
            Name::from_ascii("ExAmPlE.CoM.").expect("gültiger Name"),
            RecordType::A,
        )];
        assert_eq!(check_response(&req, &res), Ok(()));
    }

    #[test]
    fn wrong_id_is_rejected() {
        let req = request();
        let mut res = response_to(&req);
        res.metadata.id = res.metadata.id.wrapping_add(1);
        assert_eq!(check_response(&req, &res), Err(Mismatch::Id));
    }

    #[test]
    fn query_instead_of_response_is_rejected() {
        let req = request();
        let mut res = response_to(&req);
        res.metadata.message_type = MessageType::Query;
        assert_eq!(check_response(&req, &res), Err(Mismatch::NotAResponse));
    }

    #[test]
    fn wrong_name_is_rejected() {
        let req = request();
        let mut res = response_to(&req);
        res.queries = vec![Query::query(
            Name::from_ascii("evil.example.com.").expect("gültiger Name"),
            RecordType::A,
        )];
        assert_eq!(check_response(&req, &res), Err(Mismatch::Name));
    }

    #[test]
    fn wrong_qtype_is_rejected() {
        let req = request();
        let mut res = response_to(&req);
        res.queries = vec![Query::query(
            Name::from_ascii("example.com.").expect("gültiger Name"),
            RecordType::AAAA,
        )];
        assert_eq!(check_response(&req, &res), Err(Mismatch::QueryType));
    }

    #[test]
    fn wrong_qclass_is_rejected() {
        let req = request();
        let mut res = response_to(&req);
        let mut query = Query::query(
            Name::from_ascii("example.com.").expect("gültiger Name"),
            RecordType::A,
        );
        query.set_query_class(DNSClass::CH);
        res.queries = vec![query];
        assert_eq!(check_response(&req, &res), Err(Mismatch::QueryClass));
    }

    #[test]
    fn missing_question_is_rejected() {
        let req = request();
        let mut res = response_to(&req);
        res.queries.clear();
        assert_eq!(check_response(&req, &res), Err(Mismatch::QuestionCount));
    }

    #[test]
    fn small_response_is_not_truncated() {
        let req = request();
        let res = response_to(&req);
        let bytes = encode_for_udp(&res, 1232).expect("kodierbar");
        let decoded = Message::from_vec(&bytes).expect("dekodierbar");
        assert!(!decoded.metadata.truncation);
    }

    #[test]
    fn oversized_response_sets_tc_and_drops_answers() {
        let req = request();
        let mut res = response_to(&req);
        let name = Name::from_ascii("example.com.").expect("gültiger Name");
        for _ in 0..40 {
            res.add_answer(Record::from_rdata(
                name.clone(),
                60,
                RData::TXT(TXT::new(vec!["x".repeat(200)])),
            ));
        }
        assert!(res.to_vec().expect("kodierbar").len() > 1232);

        let bytes = encode_for_udp(&res, 1232).expect("kodierbar");
        assert!(
            bytes.len() <= 1232,
            "gekürzte Antwort ist {} Byte",
            bytes.len()
        );
        let decoded = Message::from_vec(&bytes).expect("dekodierbar");
        assert!(decoded.metadata.truncation, "TC-Flag fehlt");
        assert!(decoded.answers.is_empty(), "Answer-Section nicht geleert");
        assert_eq!(decoded.queries.len(), 1, "Frage muss erhalten bleiben");
    }

    #[test]
    fn format_error_echoes_the_id_from_the_first_two_bytes() {
        let msg = format_error(&[0xab, 0xcd, 0xff]).expect("zwei Bytes reichen");
        assert_eq!(msg.metadata.id, 0xabcd);
        assert_eq!(msg.metadata.response_code, ResponseCode::FormErr);
    }

    #[test]
    fn format_error_on_too_short_input_is_none() {
        assert!(format_error(&[]).is_none());
        assert!(format_error(&[0x00]).is_none());
    }
}
