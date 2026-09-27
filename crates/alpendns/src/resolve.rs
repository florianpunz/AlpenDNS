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
