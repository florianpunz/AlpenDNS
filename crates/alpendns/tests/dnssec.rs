//! DNSSEC: die Testvektoren aus Phase 7, Schritt 3.
//!
//! Drei Fälle, jeweils mit **echten** Signaturen und echter Kryptografie, nicht
//! mit von Hand gesetzten Urteilen:
//!
//! | Vektor | erwartet |
//! |---|---|
//! | gültige Signatur | `Secure`, Antwort geht durch, AD-Bit gesetzt |
//! | ungültige Signatur | `Bogus`, Antwort wird verworfen |
//! | fehlende Signatur in einer signierten Zone | `Bogus`, Antwort wird verworfen |
//!
//! **Aufbau.** Ein gefälschter Upstream (`Zone`) spielt eine kleine signierte
//! Zone: `example.test.` mit einem A-Record und dem DNSKEY der Zone. Der
//! öffentliche Schlüssel wird als Trust Anchor eingehängt, statt eine Kette bis
//! zur echten Root zu bauen — `DnssecDnsHandle` hört bei einem Schlüssel aus
//! dem Anchor-Store auf, nach DS-Records zu suchen. Damit bleibt der Test
//! klein, ohne dass an der Rechnerei etwas abgekürzt wird: dieselbe
//! Signaturprüfung läuft, die im Betrieb läuft.
//!
//! **Was hier nicht geprüft wird:** dass `Transport` den validierenden Griff
//! tatsächlich vorschaltet. Das steht in `encrypted.rs`
//! (`dnssec_sets_the_do_bit_on_the_wire`), weil es einen echten Transport
//! braucht.

#![allow(clippy::expect_used, clippy::indexing_slicing, clippy::panic)]

use std::sync::Arc;
use std::time::Duration;

use alpendns::dnssec::{self, Verdict};
use futures_util::StreamExt as _;
use hickory_net::DnsHandle;
use hickory_net::dnssec::DnssecDnsHandle;
use hickory_net::runtime::TokioRuntimeProvider;
use hickory_proto::dnssec::crypto::Ed25519SigningKey;
use hickory_proto::dnssec::rdata::DNSSECRData;
use hickory_proto::dnssec::rdata::{DNSKEY, RRSIG};
use hickory_proto::dnssec::{DnssecSigner, SigningKey as _, TrustAnchors};
use hickory_proto::op::{DnsRequest, DnsResponse, Message, OpCode, ResponseCode};
use hickory_proto::rr::rdata::A;
use hickory_proto::rr::{DNSClass, Name, RData, Record, RecordSet, RecordType};

const ZONE: &str = "example.test.";
const HOST: &str = "www.example.test.";

fn name(text: &str) -> Name {
    Name::from_ascii(text).expect("gültiger Name")
}

/// Wie eine Antwort von der Wahrheit abweicht.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Damage {
    /// Alles korrekt signiert.
    None,
    /// Die Signatur über dem A-Record ist verfälscht.
    BrokenSignature,
    /// Der A-Record kommt ohne Signatur, obwohl die Zone signiert ist.
    MissingSignature,
}

/// Eine kleine signierte Zone, die auf Anfragen antwortet.
///
/// Ein echter Signierer mit echtem Schlüssel — die Vektoren sollen an der
/// Kryptografie scheitern oder bestehen, nicht an einem Schalter im Fake.
struct Zone {
    signer: DnssecSigner,
    dnskey: DNSKEY,
    damage: Damage,
}

impl Zone {
    fn new(damage: Damage) -> Self {
        let der = Ed25519SigningKey::generate_pkcs8().expect("Schlüssel erzeugbar");
        let key = Ed25519SigningKey::from_pkcs8(&der).expect("Schlüssel lesbar");
        let public = key.to_public_key().expect("öffentlicher Teil");
        let dnskey = DNSKEY::from_key(&public);
        let signer = DnssecSigner::new(
            dnskey.clone(),
            Box::new(key),
            name(ZONE),
            Duration::from_secs(3600),
        );
        Self {
            signer,
            dnskey,
            damage,
        }
    }

    /// Der öffentliche Schlüssel als Vertrauensanker.
    fn trust_anchors(&self) -> Arc<TrustAnchors> {
        let mut anchors = TrustAnchors::empty();
        let public = self
            .signer
            .key()
            .to_public_key()
            .expect("öffentlicher Teil");
        anchors.insert(&public);
        Arc::new(anchors)
    }

    /// Signiert eine RRSet und hängt Records plus RRSIG in die Antwort.
    fn signed_into(&self, message: &mut Message, rrset: &RecordSet, sign: bool) {
        for record in rrset.records_without_rrsigs() {
            message.answers.push(record.clone());
        }
        if !sign {
            return;
        }
        // Der Beginn der Gültigkeit liegt bewusst eine Minute in der
        // Vergangenheit: eine Signatur, die exakt jetzt beginnt, gilt je nach
        // Rundung der Sekunden noch nicht.
        let inception = time::OffsetDateTime::now_utc() - time::Duration::minutes(1);
        let mut rrsig = RRSIG::from_rrset(rrset, DNSClass::IN, inception, &self.signer)
            .expect("RRSIG erzeugbar");
        if self.damage == Damage::BrokenSignature && rrset.record_type() == RecordType::A {
            rrsig = broken(&rrsig);
        }
        message.answers.push(Record::from_rdata(
            rrset.name().clone(),
            rrset.ttl(),
            RData::DNSSEC(DNSSECRData::RRSIG(rrsig)),
        ));
    }

    fn answer(&self, request: &Message) -> Message {
        let mut response = Message::response(request.metadata.id, OpCode::Query);
        response.queries = request.queries.clone();
        response.metadata.authoritative = true;

        let Some(query) = request.queries.first() else {
            response.metadata.response_code = ResponseCode::FormErr;
            return response;
        };

        match (query.name().to_lowercase(), query.query_type()) {
            (name, RecordType::DNSKEY) if name == self.zone() => {
                let mut rrset = RecordSet::new(self.zone(), RecordType::DNSKEY, 0);
                rrset.add_rdata(RData::DNSSEC(DNSSECRData::DNSKEY(self.dnskey.clone())));
                rrset.set_ttl(3600);
                self.signed_into(&mut response, &rrset, true);
            }
            (name, RecordType::A) if name == self.host() => {
                let mut rrset = RecordSet::new(self.host(), RecordType::A, 0);
                rrset.add_rdata(RData::A(A::new(192, 0, 2, 42)));
                rrset.set_ttl(300);
                self.signed_into(
                    &mut response,
                    &rrset,
                    self.damage != Damage::MissingSignature,
                );
            }
            // Alles andere existiert in dieser Zone nicht. Ein DS-Lookup landet
            // hier — beim Trust Anchor wird er gar nicht erst gestellt.
            _ => response.metadata.response_code = ResponseCode::NXDomain,
        }
        response
    }

    fn zone(&self) -> Name {
        name(ZONE).to_lowercase()
    }

    fn host(&self) -> Name {
        name(HOST).to_lowercase()
    }
}

/// Verdreht ein Bit in der Signatur.
///
/// Genau das, was ein Angreifer hinterlässt, der den Inhalt ändert, aber die
/// Signatur nicht neu erzeugen kann.
fn broken(rrsig: &RRSIG) -> RRSIG {
    let mut bytes = rrsig.sig().to_vec();
    if let Some(first) = bytes.first_mut() {
        *first ^= 0xff;
    }
    RRSIG::from_sig(rrsig.input().clone(), bytes)
}

/// Ein `DnsHandle`, der die Zone bedient, statt über das Netz zu gehen.
#[derive(Clone)]
struct FakeUpstream(Arc<Zone>);

impl DnsHandle for FakeUpstream {
    type Response = futures_util::stream::Once<
        futures_util::future::Ready<Result<DnsResponse, hickory_net::NetError>>,
    >;
    type Runtime = TokioRuntimeProvider;

    fn send(&self, request: DnsRequest) -> Self::Response {
        let response = self.0.answer(&request);
        futures_util::stream::once(futures_util::future::ready(
            DnsResponse::from_message(response)
                .map_err(|e| hickory_net::NetError::from(e.to_string())),
        ))
    }
}

/// Fragt `www.example.test.` durch die Validierung und gibt die Antwort zurück.
async fn resolve(damage: Damage) -> Message {
    let zone = Arc::new(Zone::new(damage));
    let anchors = zone.trust_anchors();
    let handle = DnssecDnsHandle::with_trust_anchor(FakeUpstream(Arc::clone(&zone)), anchors);

    let mut request = Message::query();
    request.add_query(hickory_proto::op::Query::query(name(HOST), RecordType::A));

    let mut stream = handle.send(DnsRequest::new(
        request,
        hickory_proto::op::DnsRequestOptions::default(),
    ));
    stream
        .next()
        .await
        .expect("eine Antwort")
        .expect("die Antwort ist dekodierbar")
        .into_message()
}

#[tokio::test]
async fn a_valid_signature_is_secure_and_passes() {
    let response = resolve(Damage::None).await;
    let verdict = dnssec::verdict(&response);
    assert_eq!(verdict, Verdict::Secure, "{response:?}");
    assert!(!verdict.must_be_dropped());
    assert!(
        response
            .answers
            .iter()
            .any(|record| record.record_type() == RecordType::A),
        "die eigentliche Antwort fehlt"
    );
}

#[tokio::test]
async fn a_broken_signature_is_bogus_and_is_dropped() {
    let response = resolve(Damage::BrokenSignature).await;
    let verdict = dnssec::verdict(&response);
    assert_eq!(verdict, Verdict::Bogus, "{response:?}");
    assert!(
        verdict.must_be_dropped(),
        "eine gefälschte Antwort käme durch"
    );
}

#[tokio::test]
async fn a_missing_signature_in_a_signed_zone_is_bogus() {
    // Der interessanteste der drei: die Antwort ist für sich genommen
    // unauffällig. Nur weil die Zone nachweislich signiert ist, ist das Fehlen
    // der Signatur ein Befund und kein unsignierter Normalfall.
    let response = resolve(Damage::MissingSignature).await;
    let verdict = dnssec::verdict(&response);
    assert_eq!(verdict, Verdict::Bogus, "{response:?}");
    assert!(verdict.must_be_dropped());
}

#[tokio::test]
async fn the_signatures_do_not_reach_a_client_that_did_not_ask_for_them() {
    // RFC 4035 §3.2.1. Wir haben selbst nachgerechnet; dem Client die Kette
    // trotzdem mitzuschicken kostet je Antwort ein paar hundert Byte.
    let mut response = resolve(Damage::None).await;
    assert!(
        response
            .answers
            .iter()
            .any(|record| record.record_type() == RecordType::RRSIG),
        "der Aufbau des Tests stimmt nicht: es gibt gar keine Signatur"
    );

    let removed = dnssec::strip_records(&mut response);
    assert!(removed > 0);
    assert!(
        response
            .answers
            .iter()
            .all(|record| !record.record_type().is_dnssec())
    );
    assert!(
        response
            .answers
            .iter()
            .any(|record| record.record_type() == RecordType::A),
        "mit den Signaturen ist die Antwort verschwunden"
    );
}

#[tokio::test]
async fn what_each_client_gets_is_decided_per_client() {
    // Der Cache hält die vollständige Antwort; zugeschnitten wird erst beim
    // Ausliefern (`dnssec::for_client`). Sonst bekäme der Zweite, was der
    // Erste angefordert hat.
    let cached = resolve(Damage::None).await;
    assert!(
        cached
            .answers
            .iter()
            .any(|record| record.record_type() == RecordType::RRSIG),
        "der Aufbau des Tests stimmt nicht: es gibt gar keine Signatur"
    );

    let mut plain = Message::query();
    plain.add_query(hickory_proto::op::Query::query(name(HOST), RecordType::A));
    let mut with_do = plain.clone();
    let mut edns = hickory_proto::op::Edns::new();
    edns.enable_dnssec();
    with_do.set_edns(edns);

    let mut without = cached.clone();
    dnssec::for_client(&plain, &mut without);
    assert!(
        without
            .answers
            .iter()
            .all(|record| !record.record_type().is_dnssec()),
        "der Client ohne DO bekam Signaturen"
    );

    let mut asked = cached;
    dnssec::for_client(&with_do, &mut asked);
    assert!(
        asked
            .answers
            .iter()
            .any(|record| record.record_type() == RecordType::RRSIG),
        "dem Client mit DO fehlt die Kette"
    );
}
