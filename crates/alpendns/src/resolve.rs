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
    #[error("Upstream hat nicht innerhalb des Zeitbudgets geantwortet")]
    Timeout,
    #[error("Netzwerkfehler zum Upstream: {0}")]
    Io(#[from] std::io::Error),
    #[error("Anfrage konnte nicht kodiert werden: {0}")]
    Encode(String),
    #[error("Antwort des Upstreams war nicht dekodierbar: {0}")]
    Malformed(String),
    #[error("Antwort des Upstreams passt nicht zur Frage: {0:?}")]
    Mismatch(crate::dns::Mismatch),
    #[error("die zusammengefasste Anfrage an den Upstream ist fehlgeschlagen")]
    Coalesced,
    #[error("Verbindung zum Upstream nicht möglich: {0}")]
    Connect(String),
    #[error("Upstream-Fehler: {0}")]
    Upstream(String),
    #[error("kein Upstream im Pool konnte antworten")]
    NoUpstreamLeft,
    /// Die Signaturkette schließt nicht. Terminal: es wird kein weiterer
    /// Upstream gefragt (Begründung in `upstream::pool::Pool::resolve`).
    #[error("DNSSEC-Prüfung fehlgeschlagen, die Antwort wurde verworfen")]
    Bogus,
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
