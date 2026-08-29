//! Die Schnittstelle zwischen Server-Pipeline und Namensauflösung.
//!
//! Das ist die Stelle aus ARCHITECTURE.md §1, an der eine spätere Rekursion
//! eingehängt würde. v1 hat genau eine Implementierung ([`crate::upstream::ForwardBackend`]),
//! und ADR-0003 zählt auf, was dieser Trait ausdrücklich *nicht* rechtfertigt:
//! keine Root-Hints, keine Delegation-Strukturen, keine Konfigurationsschlüssel
//! für etwas, das es nicht gibt.

use std::future::Future;

use hickory_proto::op::Message;

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
}

/// Löst eine Anfrage auf — in v1 durch Weiterleiten an einen Upstream.
///
/// Übergeben wird die vollständige Nachricht und nicht nur die Frage: ein
/// Forwarder reicht Flags und EDNS-Optionen mit weiter, ein späterer Rekursor
/// würde sich daraus nehmen, was er braucht.
///
/// Der Rückgabetyp ist explizit `impl Future + Send`, damit die Antwort in einem
/// `tokio::spawn` verarbeitet werden kann.
pub trait ResolveBackend: Send + Sync + 'static {
    fn resolve(
        &self,
        request: &Message,
    ) -> impl Future<Output = Result<Message, ResolveError>> + Send;
}
