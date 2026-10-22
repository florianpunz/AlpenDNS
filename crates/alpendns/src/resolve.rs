//! Die Schnittstelle zwischen Server-Pipeline und Namensauflösung.
//!
//! Das ist die Stelle aus ARCHITECTURE.md §1, an der eine spätere Rekursion
//! eingehängt würde. v1 hat genau eine Implementierung ([`crate::upstream::ForwardBackend`]),
//! und ADR-0003 zählt auf, was dieser Trait ausdrücklich *nicht* rechtfertigt:
//! keine Root-Hints, keine Delegation-Strukturen, keine Konfigurationsschlüssel
//! für etwas, das es nicht gibt.

use std::future::Future;

use hickory_proto::op::Message;

use crate::trace::Ctx;

/// Warum eine Auflösung fehlgeschlagen ist.
#[derive(Debug, thiserror::Error)]
pub enum ResolveError {
    #[error("Upstream did not answer within the time budget")]
    Timeout,
    #[error("Network error to the upstream: {0}")]
    Io(#[from] std::io::Error),
    #[error("Query could not be encoded: {0}")]
    Encode(String),
    #[error("Answer from the upstream could not be decoded: {0}")]
    Malformed(String),
    #[error("Answer from the upstream does not match the question: {0:?}")]
    Mismatch(crate::dns::Mismatch),
    #[error("The coalesced query to the upstream failed")]
    Coalesced,
    #[error("Cannot connect to the upstream: {0}")]
    Connect(String),
    #[error("Upstream error: {0}")]
    Upstream(String),
    #[error("No upstream in the pool could answer")]
    NoUpstreamLeft,
    /// Die Signaturkette schließt nicht. Terminal: es wird kein weiterer
    /// Upstream gefragt (Begründung in `upstream::pool::Pool::resolve`).
    #[error("DNSSEC validation failed, the answer was dropped")]
    Bogus,
    /// Der Upstream hat nichts geliefert, worüber sich urteilen ließe — eine
    /// leere Fehlerantwort. Das ist **kein** DNSSEC-Befund, sondern ein
    /// Ausfall: der nächste Upstream wird gefragt, ohne dass diesem hier ein
    /// Fehlversuch angerechnet wird (Begründung in `crate::dnssec::from_error`).
    #[error("Upstream delivered no verifiable answer")]
    Unproven,
    /// Dieselbe Frage ist gerade eben schon gescheitert und der Fehler wird
    /// kurz gemerkt (RFC 9520 §3.2, ADR-0027). Der Upstream wird innerhalb der
    /// Frist nicht erneut gefragt.
    #[error("resolution failed recently; the failure is cached")]
    CachedFailure,
}

/// Löst eine Anfrage auf — in v1 durch Weiterleiten an einen Upstream.
///
/// Übergeben wird die vollständige Nachricht und nicht nur die Frage: ein
/// Forwarder reicht Flags und EDNS-Optionen mit weiter, ein späterer Rekursor
/// würde sich daraus nehmen, was er braucht.
///
/// Der Rückgabetyp ist explizit `impl Future + Send`, damit die Antwort in einem
/// `tokio::spawn` verarbeitet werden kann.
///
/// Der Kontext wird **exklusiv** durchgereicht: die Pipeline ist eine Kette ohne
/// Verzweigung, seit immer genau ein Upstream gefragt wird (ADR-0012).
pub trait ResolveBackend: Send + Sync + 'static {
    fn resolve(
        &self,
        request: &Message,
        ctx: &mut Ctx,
    ) -> impl Future<Output = Result<Message, ResolveError>> + Send;
}

/// Damit ein Backend geteilt werden kann, ohne dass der Besitzer es aufgibt —
/// `main` behält so einen Griff auf den Pool, um dessen Statistik zu loggen.
impl<B: ResolveBackend> ResolveBackend for std::sync::Arc<B> {
    fn resolve(
        &self,
        request: &Message,
        ctx: &mut Ctx,
    ) -> impl Future<Output = Result<Message, ResolveError>> + Send {
        (**self).resolve(request, ctx)
    }
}

/// Fragt mehrere Backends der Reihe nach und nimmt die erste Antwort.
///
/// Für eine `forward_zone` mit mehreren Servern: fällt einer aus, übernimmt der
/// nächste. Gefragt wird dabei immer nur einer — die Anfrage verzweigt nicht,
/// sie wird wiederholt, deshalb bleibt der [`Ctx`] exklusiv.
///
/// Der Preis der Einfachheit: ein toter erster Server kostet bei jeder Anfrage
/// seinen Timeout. Die Reihenfolge in der Konfiguration ist die Priorität; wer
/// das nicht warten will, trägt den erreichbaren Server zuerst ein.
#[derive(Debug)]
pub struct Failover<B> {
    backends: Vec<B>,
}

impl<B> Failover<B> {
    pub fn new(backends: Vec<B>) -> Self {
        Self { backends }
    }
}

impl<B: ResolveBackend> ResolveBackend for Failover<B> {
    async fn resolve(&self, request: &Message, ctx: &mut Ctx) -> Result<Message, ResolveError> {
        let mut last = ResolveError::NoUpstreamLeft;
        for backend in &self.backends {
            match backend.resolve(request, ctx).await {
                Ok(answer) => return Ok(answer),
                Err(error) => last = error,
            }
        }
        Err(last)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::net::SocketAddr;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use hickory_proto::op::OpCode;

    /// Ein Backend, das mitzählt und entweder antwortet oder unter seinem
    /// Namen scheitert — damit lässt sich die Reihenfolge belegen.
    struct Fake {
        outcome: Result<(), &'static str>,
        calls: AtomicUsize,
    }

    impl Fake {
        fn answering() -> Arc<Self> {
            Arc::new(Self {
                outcome: Ok(()),
                calls: AtomicUsize::new(0),
            })
        }

        fn failing(label: &'static str) -> Arc<Self> {
            Arc::new(Self {
                outcome: Err(label),
                calls: AtomicUsize::new(0),
            })
        }

        fn calls(&self) -> usize {
            self.calls.load(Ordering::SeqCst)
        }
    }

    impl ResolveBackend for Fake {
        fn resolve(
            &self,
            request: &Message,
            _ctx: &mut Ctx,
        ) -> impl Future<Output = Result<Message, ResolveError>> + Send {
            self.calls.fetch_add(1, Ordering::SeqCst);
            let outcome = self.outcome;
            let id = request.metadata.id;
            async move {
                match outcome {
                    Ok(()) => Ok(Message::response(id, OpCode::Query)),
                    Err(label) => Err(ResolveError::Upstream(label.to_owned())),
                }
            }
        }
    }

    fn ctx() -> Ctx {
        Ctx::new(SocketAddr::from(([127, 0, 0, 1], 5555)))
    }

    fn ask() -> Message {
        Message::new(1, hickory_proto::op::MessageType::Query, OpCode::Query)
    }

    #[tokio::test]
    async fn the_next_server_is_asked_when_the_first_one_fails() {
        let first = Fake::failing("first");
        let second = Fake::answering();
        let failover = Failover::new(vec![Arc::clone(&first), Arc::clone(&second)]);

        failover
            .resolve(&ask(), &mut ctx())
            .await
            .expect("der zweite antwortet");
        assert_eq!(first.calls(), 1);
        assert_eq!(second.calls(), 1);
    }

    #[tokio::test]
    async fn the_first_answer_wins_and_the_rest_stay_unasked() {
        let first = Fake::answering();
        let second = Fake::answering();
        let failover = Failover::new(vec![Arc::clone(&first), Arc::clone(&second)]);

        failover
            .resolve(&ask(), &mut ctx())
            .await
            .expect("der erste antwortet");
        assert_eq!(first.calls(), 1);
        assert_eq!(second.calls(), 0);
    }

    #[tokio::test]
    async fn when_all_fail_the_last_error_is_reported() {
        let first = Fake::failing("first");
        let second = Fake::failing("second");
        let failover = Failover::new(vec![Arc::clone(&first), Arc::clone(&second)]);

        match failover.resolve(&ask(), &mut ctx()).await {
            Err(ResolveError::Upstream(label)) => assert_eq!(label, "second"),
            other => panic!("erwartet war der letzte Fehler, gekommen ist {other:?}"),
        }
        assert_eq!(first.calls(), 1);
        assert_eq!(second.calls(), 1);
    }
}
