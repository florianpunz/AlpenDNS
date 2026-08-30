//! DNSSEC-Validierung: selbst nachrechnen statt dem Upstream zu glauben.
//!
//! Ein Forwarder bekommt vom Upstream ein AD-Bit geliefert. Das ist ein Bit in
//! einem Paket, das genau der Rechner gesetzt hat, dem man gerade nicht mehr
//! vertrauen möchte als nötig — [THREAT-MODEL.md](../../../docs/THREAT-MODEL.md)
//! nennt den kompromittierten Upstream ausdrücklich als offenen Punkt. Seit
//! Phase 7 rechnet AlpenDNS die Signaturkette selbst nach, von den
//! einkompilierten Root-Schlüsseln abwärts.
//!
//! **Die Krypto kommt aus `hickory-proto`/`hickory-net`** (ADR-0002 und
//! [ADR-0016](../../../docs/adr/0016-dnssec-validierung-im-forwarder.md)):
//! `DnssecDnsHandle` hängt sich vor die Verbindung, setzt das DO-Bit, holt sich
//! DNSKEY- und DS-Sätze nach und stempelt jedem Record ein [`Proof`] auf. Was
//! dieses Modul beisteuert, ist die Auswertung: aus vielen Record-Stempeln ein
//! Urteil je Antwort, die Entscheidung daraus, und die Buchführung darüber.

use std::sync::atomic::{AtomicU64, Ordering};

use hickory_proto::dnssec::Proof;
use hickory_proto::op::Message;

/// Das Urteil über eine ganze Antwort.
///
/// Die vier Zustände sind die aus RFC 4035 §4.3, in derselben Bedeutung, in der
/// `hickory` sie je Record führt. Zusammengefasst wird pessimistisch: ein
/// einziger fauler Record macht die Antwort faul.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    /// Kette bis zum Root-Schlüssel geschlossen, alle Signaturen gültig.
    Secure,
    /// Die Zone ist nachweislich unsigniert. Kein Fehler — der größte Teil des
    /// Netzes sieht so aus.
    Insecure,
    /// Die Zone gibt sich signiert, die Kette schließt aber nicht. Entweder ein
    /// Angriff oder eine kaputte Zone; von hier aus nicht unterscheidbar.
    Bogus,
    /// Es ließ sich nicht feststellen, ob überhaupt eine Kette existieren
    /// müsste — etwa weil die Antwort gar keine Records enthält.
    Indeterminate,
}

impl Verdict {
    pub const ALL: [Self; 4] = [
        Self::Secure,
        Self::Insecure,
        Self::Bogus,
        Self::Indeterminate,
    ];

    /// Kurzform für Metrik-Labels und die API. Geschlossene Menge, kein Name.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Secure => "secure",
            Self::Insecure => "insecure",
            Self::Bogus => "bogus",
            Self::Indeterminate => "indeterminate",
        }
    }

    /// Ob diese Antwort verworfen werden muss.
    ///
    /// Nur `Bogus`. `Insecure` ist der Normalfall im heutigen Netz, und
    /// `Indeterminate` heißt "keine Aussage" — daraus einen Fehler zu machen
    /// hieße, jede Antwort ohne Records zu verwerfen.
    pub const fn must_be_dropped(self) -> bool {
        matches!(self, Self::Bogus)
    }
}

impl std::fmt::Display for Verdict {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let text = match self {
            Self::Secure => "signiert und gültig",
            Self::Insecure => "unsignierte Zone",
            Self::Bogus => "Signatur ungültig oder fehlt",
            Self::Indeterminate => "keine Aussage möglich",
        };
        f.write_str(text)
    }
}

/// Fasst die Record-Stempel einer Antwort zu einem Urteil zusammen.
///
/// Angesehen werden Answer- und Authority-Abschnitt: eine negative Antwort
/// (NXDOMAIN, NODATA) trägt ihren Beweis in den NSEC-Records der Authority,
/// nicht im leeren Answer-Abschnitt. Der Additional-Abschnitt bleibt außen vor
/// — dort steht Beiwerk wie Glue, dessen Fehlen die Antwort nicht falsch macht.
pub fn verdict(message: &Message) -> Verdict {
    let mut seen = false;
    let mut all_secure = true;
    let mut any_insecure = false;

    for record in message.answers.iter().chain(message.authorities.iter()) {
        seen = true;
        match record.proof {
            // Ein einziger fauler Record genügt. Sofort raus: was danach kommt,
            // kann das Urteil nicht mehr verbessern.
            Proof::Bogus => return Verdict::Bogus,
            Proof::Secure => {}
            Proof::Insecure => {
                all_secure = false;
                any_insecure = true;
            }
            Proof::Indeterminate => all_secure = false,
        }
    }

    if !seen {
        return Verdict::Indeterminate;
    }
    if all_secure {
        return Verdict::Secure;
    }
    if any_insecure {
        return Verdict::Insecure;
    }
    Verdict::Indeterminate
}

/// Entfernt DNSSEC-Records aus einer Antwort und gibt zurück, wie viele es waren.
///
/// Wir haben die Kette selbst nachgerechnet; die Signaturen dem Client
/// weiterzureichen, der sie nicht angefordert hat, ist nach RFC 4035 §3.2.1
/// unzulässig und kostet je Antwort ein paar hundert Byte — genug, um über die
/// UDP-Grenze zu geraten und den Client unnötig auf TCP zu schicken.
///
/// **Grenze, die dokumentiert gehört:** ein Client, der selbst validieren
/// möchte, bekommt hier nichts mehr dafür. AlpenDNS validiert *für* seine
/// Clients, es reicht keine Kette durch. Das ist eine bewusste Vereinfachung
/// für v1 und in ADR-0016 als solche vermerkt.
pub fn strip_records(message: &mut Message) -> usize {
    let before = message
        .answers
        .len()
        .saturating_add(message.authorities.len())
        .saturating_add(message.additionals.len());
    message
        .answers
        .retain(|record| !record.record_type().is_dnssec());
    message
        .authorities
        .retain(|record| !record.record_type().is_dnssec());
    message
        .additionals
        .retain(|record| !record.record_type().is_dnssec());
    before.saturating_sub(
        message
            .answers
            .len()
            .saturating_add(message.authorities.len())
            .saturating_add(message.additionals.len()),
    )
}

/// Ob der Client die Signaturen selbst haben will — das DO-Bit in EDNS.
///
/// Nur dann bleiben die DNSSEC-Records in der Antwort stehen. Das AD-Bit in
/// einer *Anfrage* zählt hier ausdrücklich **nicht**: nach RFC 6840 §5.7 heißt
/// es "sag mir dein Urteil", nicht "schick mir die Kette". `dig` setzt es per
/// Default — würde man es als Wunsch nach Records lesen, bekäme praktisch jeder
/// Client die ganze Kette mitgeschickt, für die er keine Verwendung hat.
pub fn client_wants_records(request: &Message) -> bool {
    request
        .edns
        .as_ref()
        .is_some_and(|edns| edns.flags().dnssec_ok)
}

/// Ob der Client überhaupt an unserem Urteil interessiert ist.
///
/// RFC 6840 §5.8: das AD-Bit in der Antwort wird nur gesetzt, wenn der Client
/// in seiner Anfrage DO oder AD gesetzt hat. Wer keins von beidem tut, weiß mit
/// dem Bit nichts anzufangen, und es unaufgefordert zu setzen macht es für die
/// anderen wertloser.
pub fn client_wants_verdict(request: &Message) -> bool {
    request.metadata.authentic_data || client_wants_records(request)
}

/// Ob dieser Fehler in Wahrheit ein DNSSEC-Urteil ist.
///
/// `hickory` liefert ein negatives Ergebnis, dessen NSEC-Beweis nicht aufgeht,
/// **als Fehler** statt als gestempelte Nachricht — samt der Antwort, um die es
/// ging. Ohne diesen Zweig ginge eine faule Signatur als Netzwerkfehler durch:
/// der Zähler bliebe auf null, dem Upstream würde ein Fehlversuch angerechnet,
/// und der nächste Anbieter bekäme dieselbe Frage vorgelegt — genau die drei
/// Dinge, die [ADR-0016](../../../docs/adr/0016-dnssec-validierung-im-forwarder.md)
/// ausschließt.
///
/// Gefunden beim ersten Lauf gegen echte Upstreams: `dnssec-failed.org` ergab
/// zwar korrekt SERVFAIL, aber `alpendns_dnssec_total{result="bogus"}` blieb
/// auf 0.
pub fn from_error(error: &hickory_net::NetError) -> Option<(Verdict, Message)> {
    let hickory_net::NetError::Dns(hickory_net::DnsError::Nsec {
        proof, response, ..
    }) = error
    else {
        return None;
    };
    let verdict = match proof {
        Proof::Bogus => Verdict::Bogus,
        Proof::Insecure => Verdict::Insecure,
        Proof::Secure => Verdict::Secure,
        Proof::Indeterminate => Verdict::Indeterminate,
    };
    Some((verdict, response.as_ref().clone().into_message()))
}

/// Ob der Client die Prüfung ausdrücklich abbestellt hat (CD-Bit, RFC 4035 §3.2.2).
///
/// Dann wird nicht validiert und nichts verworfen. Das ist kein Schlupfloch für
/// einen Angreifer: CD setzt der Client selbst, und wer die Prüfung abschaltet,
/// schadet nur sich.
pub fn checking_disabled(request: &Message) -> bool {
    request.metadata.checking_disabled
}

/// Wertet eine validierte Antwort aus: Urteil bilden, Bogus verwerfen, AD-Bit
/// setzen.
///
/// Was der *einzelne Client* davon zu sehen bekommt, entscheidet [`for_client`]
/// — und zwar erst hinter dem Cache. Hier unten darf nichts weggeräumt werden,
/// was ein anderer Client noch braucht: der Cache liegt über dieser Schicht und
/// hält eine Antwort für alle (ARCHITECTURE.md §4).
///
/// Steht hier und nicht im Transport, weil es zwei Wege gibt, die validieren
/// können — den direkten (`upstream::transport`) und den über einen ODoH-Proxy
/// (`upstream::odoh`). Zweimal dieselbe Folgerung zu schreiben wäre die Sorte
/// Verdopplung, bei der eine Hälfte später stillschweigend anders entscheidet.
pub fn apply(response: &mut Message) -> Result<Verdict, crate::resolve::ResolveError> {
    let verdict = verdict(response);
    apply_verdict(response, verdict)
}

/// Wie [`apply`], aber mit einem Urteil, das schon feststeht.
///
/// Der Weg für [`from_error`]: dort steckt das Urteil im Fehler, und es aus der
/// mitgelieferten Antwort noch einmal abzuleiten würde etwas anderes ergeben —
/// die Records darin tragen keinen Stempel.
pub fn apply_verdict(
    response: &mut Message,
    verdict: Verdict,
) -> Result<Verdict, crate::resolve::ResolveError> {
    record(verdict);
    if verdict.must_be_dropped() {
        return Err(crate::resolve::ResolveError::Bogus);
    }
    // Wir haben selbst nachgerechnet, also steht das AD-Bit für unser Urteil
    // und nicht mehr für die Behauptung des Upstreams.
    response.metadata.authentic_data = verdict == Verdict::Secure;
    Ok(verdict)
}

/// Passt eine fertige Antwort an das an, was dieser Client angefordert hat.
///
/// Läuft an der Außenkante, hinter dem Cache — und genau deshalb überhaupt.
/// Würde hier unten im Transport gestrippt, landete die gestutzte Fassung im
/// Cache, und wer als Nächster mit DO fragt, bekäme sie ohne Signaturen
/// ausgeliefert. Beim ersten Lauf gegen echte Upstreams war das so: eine
/// Anfrage mit `dig +dnssec` bekam null RRSIGs, weil ein `dig` ohne davor da
/// war.
///
/// Zwei Regeln, beide aus RFC 6840:
///
/// * §5.7 — die Signaturen gehen nur an einen Client mit DO-Bit. Ohne DO sind
///   sie ein paar hundert Byte, die niemand liest, und schicken ihn womöglich
///   unnötig auf TCP.
/// * §5.8 — das AD-Bit nur an einen Client, der DO oder AD gesetzt hat.
pub fn for_client(request: &Message, response: &mut Message) {
    if !client_wants_verdict(request) {
        response.metadata.authentic_data = false;
    }
    if !client_wants_records(request) {
        strip_records(response);
    }
}

/// Wie oft welches Urteil gefallen ist.
///
/// Prozessweit und atomar, aus demselben Grund wie bei [`crate::privacy`]: die
/// Einstellung liegt als `Copy` in jedem Transport, ein Zähler darin würde je
/// Transport getrennt zählen.
#[derive(Debug, Default)]
pub struct Counters {
    pub secure: AtomicU64,
    pub insecure: AtomicU64,
    pub bogus: AtomicU64,
    pub indeterminate: AtomicU64,
}

/// Momentaufnahme der Zähler, für Metriken und API.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct CounterSnapshot {
    pub secure: u64,
    pub insecure: u64,
    pub bogus: u64,
    pub indeterminate: u64,
}

impl CounterSnapshot {
    /// Der Zählerstand zu einem Urteil.
    pub const fn get(&self, verdict: Verdict) -> u64 {
        match verdict {
            Verdict::Secure => self.secure,
            Verdict::Insecure => self.insecure,
            Verdict::Bogus => self.bogus,
            Verdict::Indeterminate => self.indeterminate,
        }
    }

    /// Alle validierten Antworten zusammen.
    pub const fn total(&self) -> u64 {
        self.secure
            .saturating_add(self.insecure)
            .saturating_add(self.bogus)
            .saturating_add(self.indeterminate)
    }
}

static COUNTERS: Counters = Counters {
    secure: AtomicU64::new(0),
    insecure: AtomicU64::new(0),
    bogus: AtomicU64::new(0),
    indeterminate: AtomicU64::new(0),
};

/// Trägt ein Urteil in die Statistik ein.
pub fn record(verdict: Verdict) {
    let counter = match verdict {
        Verdict::Secure => &COUNTERS.secure,
        Verdict::Insecure => &COUNTERS.insecure,
        Verdict::Bogus => &COUNTERS.bogus,
        Verdict::Indeterminate => &COUNTERS.indeterminate,
    };
    counter.fetch_add(1, Ordering::Relaxed);
}

/// Der aktuelle Stand aller Urteile.
pub fn counters() -> CounterSnapshot {
    CounterSnapshot {
        secure: COUNTERS.secure.load(Ordering::Relaxed),
        insecure: COUNTERS.insecure.load(Ordering::Relaxed),
        bogus: COUNTERS.bogus.load(Ordering::Relaxed),
        indeterminate: COUNTERS.indeterminate.load(Ordering::Relaxed),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use hickory_proto::op::{Edns, MessageType, OpCode, Query};
    use hickory_proto::rr::rdata::A;
    use hickory_proto::rr::{Name, RData, Record, RecordType};

    fn name(text: &str) -> Name {
        Name::from_ascii(text).expect("gültiger Name")
    }

    fn answer_with(proofs: &[Proof]) -> Message {
        let mut message = Message::response(1, OpCode::Query);
        message.add_query(Query::query(name("example.com."), RecordType::A));
        for proof in proofs {
            let mut record =
                Record::from_rdata(name("example.com."), 60, RData::A(A::new(192, 0, 2, 1)));
            record.proof = *proof;
            message.answers.push(record);
        }
        message
    }

    #[test]
    fn a_fully_signed_answer_is_secure() {
        assert_eq!(
            verdict(&answer_with(&[Proof::Secure, Proof::Secure])),
            Verdict::Secure
        );
    }

    #[test]
    fn one_bogus_record_makes_the_whole_answer_bogus() {
        // Pessimistisch zusammengefasst: sonst könnte ein Angreifer einen
        // gefälschten Record neben echte hängen und käme damit durch.
        assert_eq!(
            verdict(&answer_with(&[Proof::Secure, Proof::Bogus, Proof::Secure])),
            Verdict::Bogus
        );
    }

    #[test]
    fn an_unsigned_zone_is_insecure_not_an_error() {
        let message = answer_with(&[Proof::Insecure]);
        assert_eq!(verdict(&message), Verdict::Insecure);
        assert!(!verdict(&message).must_be_dropped());
    }

    #[test]
    fn only_bogus_is_dropped() {
        for candidate in Verdict::ALL {
            assert_eq!(
                candidate.must_be_dropped(),
                candidate == Verdict::Bogus,
                "{candidate:?}"
            );
        }
    }

    #[test]
    fn an_answer_without_records_says_nothing() {
        let mut message = Message::response(1, OpCode::Query);
        message.add_query(Query::query(name("example.com."), RecordType::A));
        assert_eq!(verdict(&message), Verdict::Indeterminate);
    }

    #[test]
    fn the_authority_section_counts_too() {
        // Eine negative Antwort trägt ihren Beweis in den NSEC-Records der
        // Authority. Würden wir nur den Answer-Abschnitt ansehen, käme jedes
        // gefälschte NXDOMAIN als Indeterminate durch.
        let mut message = Message::response(1, OpCode::Query);
        message.add_query(Query::query(name("example.com."), RecordType::A));
        let mut record =
            Record::from_rdata(name("example.com."), 60, RData::A(A::new(192, 0, 2, 1)));
        record.proof = Proof::Bogus;
        message.authorities.push(record);
        assert_eq!(verdict(&message), Verdict::Bogus);
    }

    #[test]
    fn stripping_removes_signatures_and_keeps_the_answer() {
        let mut message = answer_with(&[Proof::Secure]);
        let signature = Record::update0(name("example.com."), 60, RecordType::RRSIG);
        message.answers.push(signature);
        message.additionals.push(Record::update0(
            name("example.com."),
            60,
            RecordType::DNSKEY,
        ));

        assert_eq!(strip_records(&mut message), 2);
        assert_eq!(message.answers.len(), 1, "die eigentliche Antwort ist weg");
        assert!(message.additionals.is_empty());
        assert!(
            message
                .answers
                .iter()
                .all(|record| !record.record_type().is_dnssec())
        );
    }

    #[test]
    fn stripping_an_answer_without_signatures_changes_nothing() {
        let mut message = answer_with(&[Proof::Insecure]);
        assert_eq!(strip_records(&mut message), 0);
        assert_eq!(message.answers.len(), 1);
    }

    #[test]
    fn only_the_do_bit_asks_for_the_records() {
        // RFC 6840 §5.7: AD in der Anfrage heißt "sag mir dein Urteil", nicht
        // "schick mir die Kette". `dig` setzt AD per Default — die beiden zu
        // verwechseln hieße, praktisch jedem Client die Signaturen
        // mitzuschicken. Genau das ist im ersten Lauf gegen echte Upstreams
        // passiert.
        let mut plain = Message::new(1, MessageType::Query, OpCode::Query);
        plain.add_query(Query::query(name("example.com."), RecordType::A));
        assert!(!client_wants_records(&plain));
        assert!(!client_wants_verdict(&plain));

        let mut with_do = plain.clone();
        let mut edns = Edns::new();
        edns.enable_dnssec();
        with_do.set_edns(edns);
        assert!(client_wants_records(&with_do));
        assert!(client_wants_verdict(&with_do));

        let mut with_ad = plain.clone();
        with_ad.metadata.authentic_data = true;
        assert!(
            !client_wants_records(&with_ad),
            "AD in der Anfrage ist kein Wunsch nach Records"
        );
        assert!(client_wants_verdict(&with_ad));

        assert!(!checking_disabled(&plain));
        let mut with_cd = plain;
        with_cd.metadata.checking_disabled = true;
        assert!(checking_disabled(&with_cd));
    }

    #[test]
    fn the_ad_bit_is_only_handed_to_a_client_that_asked() {
        let mut question = Message::new(1, MessageType::Query, OpCode::Query);
        question.add_query(Query::query(name("example.com."), RecordType::A));

        // Im Cache landet das Urteil; wer es sieht, entscheidet die Außenkante.
        let mut cached = answer_with(&[Proof::Secure]);
        apply(&mut cached).expect("geht durch");
        assert!(cached.metadata.authentic_data, "das Urteil fehlt im Cache");

        let mut plain = cached.clone();
        for_client(&question, &mut plain);
        assert!(
            !plain.metadata.authentic_data,
            "AD ausgeliefert, obwohl niemand danach gefragt hat"
        );

        question.metadata.authentic_data = true;
        let mut asked = cached;
        for_client(&question, &mut asked);
        assert!(asked.metadata.authentic_data);

        // Eine unsignierte Zone ist nicht "authentic data".
        let mut insecure = answer_with(&[Proof::Insecure]);
        apply(&mut insecure).expect("geht durch");
        assert!(!insecure.metadata.authentic_data);
    }

    /// Der Fall, der beim ersten Lauf gegen echte Upstreams schiefging.
    #[test]
    fn a_client_with_do_still_gets_the_chain_after_someone_without() {
        let mut signed = answer_with(&[Proof::Secure]);
        signed
            .answers
            .push(Record::update0(name("example.com."), 60, RecordType::RRSIG));

        let mut plain = Message::new(1, MessageType::Query, OpCode::Query);
        plain.add_query(Query::query(name("example.com."), RecordType::A));
        let mut with_do = plain.clone();
        let mut edns = Edns::new();
        edns.enable_dnssec();
        with_do.set_edns(edns);

        // Was im Cache liegt, ist unberührt — hier wird nur ausgeliefert.
        let mut first = signed.clone();
        for_client(&plain, &mut first);
        assert!(
            first
                .answers
                .iter()
                .all(|record| !record.record_type().is_dnssec()),
            "der Client ohne DO bekam Signaturen"
        );

        let mut second = signed;
        for_client(&with_do, &mut second);
        assert!(
            second
                .answers
                .iter()
                .any(|record| record.record_type() == RecordType::RRSIG),
            "dem Client mit DO fehlt die Kette"
        );
    }

    #[test]
    fn a_bogus_answer_is_dropped_by_apply() {
        let mut question = Message::new(1, MessageType::Query, OpCode::Query);
        question.add_query(Query::query(name("example.com."), RecordType::A));
        let mut response = answer_with(&[Proof::Bogus]);
        assert!(matches!(
            apply(&mut response),
            Err(crate::resolve::ResolveError::Bogus)
        ));
    }

    #[test]
    fn an_error_without_a_proof_is_not_a_verdict() {
        // Ein Netzwerkfehler darf nicht als DNSSEC-Urteil durchgehen, sonst
        // bekäme ein toter Upstream keinen Fehlversuch angerechnet.
        assert!(from_error(&hickory_net::NetError::Timeout).is_none());
        assert!(from_error(&hickory_net::NetError::Busy).is_none());
    }

    #[test]
    fn every_verdict_has_a_label_and_a_sentence() {
        let mut seen = std::collections::HashSet::new();
        for candidate in Verdict::ALL {
            assert!(seen.insert(candidate.as_str()), "{candidate:?} doppelt");
            assert!(!candidate.to_string().is_empty());
            // Label-Werte landen in Prometheus. Keine Punkte, keine Namen.
            assert!(!candidate.as_str().contains('.'));
        }
        assert_eq!(seen.len(), 4);
    }

    #[test]
    fn the_snapshot_reads_back_every_verdict() {
        let snapshot = CounterSnapshot {
            secure: 1,
            insecure: 2,
            bogus: 3,
            indeterminate: 4,
        };
        assert_eq!(snapshot.get(Verdict::Secure), 1);
        assert_eq!(snapshot.get(Verdict::Insecure), 2);
        assert_eq!(snapshot.get(Verdict::Bogus), 3);
        assert_eq!(snapshot.get(Verdict::Indeterminate), 4);
        assert_eq!(snapshot.total(), 10);
    }
}
