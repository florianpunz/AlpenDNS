//! UDP-Listener.

use std::sync::Arc;

use tokio::net::UdpSocket;
use tokio::sync::Semaphore;
use tokio_util::sync::CancellationToken;
use tokio_util::task::TaskTracker;

use crate::dns;
use crate::logging::QueryLog;
use crate::ratelimit::RateLimiter;
use crate::resolve::ResolveBackend;

/// Größte Anfrage, die wir über UDP entgegennehmen. Alles darüber ist entweder
/// kaputt oder gehört über TCP.
const MAX_UDP_REQUEST: usize = 4096;

/// Die beiden Grenzen, die nur den UDP-Pfad betreffen.
pub(crate) struct Limits {
    /// Ab dieser Antwortgröße wird gekürzt und TC gesetzt.
    pub payload_size: usize,
    /// Erlaubnisse für gleichzeitig laufende Anfragen, über alle Listener.
    pub inflight: Arc<Semaphore>,
}

pub(crate) async fn serve<B: ResolveBackend>(
    socket: Arc<UdpSocket>,
    backend: Arc<B>,
    log: Arc<QueryLog>,
    limiter: Option<Arc<RateLimiter>>,
    limits: Limits,
    shutdown: CancellationToken,
    tracker: TaskTracker,
) {
    let mut buf = vec![0_u8; MAX_UDP_REQUEST];
    loop {
        let (len, peer) = tokio::select! {
            () = shutdown.cancelled() => break,
            result = socket.recv_from(&mut buf) => match result {
                Ok(received) => received,
                Err(error) => {
                    tracing::warn!(%error, "recv_from failed");
                    continue;
                }
            },
        };

        let Some(packet) = buf.get(..len).map(<[u8]>::to_vec) else {
            continue;
        };

        // Verworfen, nicht abgelehnt: eine Antwort an eine womöglich gefälschte
        // Absenderadresse wäre die Reflexion, gegen die gedrosselt wird
        // (ADR-0020). Die Grenze zählt für alle UDP-Listener zusammen; das
        // Permit hängt an der Aufgabe und fällt mit ihr.
        let Ok(permit) = Arc::clone(&limits.inflight).try_acquire_owned() else {
            tracing::debug!(%peer, "query dropped: too many in flight");
            continue;
        };
        let payload_size = limits.payload_size;

        // Die Antwort muss über denselben Socket an genau die Quelladresse
        // zurück, von der die Anfrage kam.
        let socket = Arc::clone(&socket);
        let backend = Arc::clone(&backend);
        let log = Arc::clone(&log);
        let limiter = limiter.clone();
        tracker.spawn(async move {
            let _permit = permit;
            let Some(response) = crate::server::handle_request(
                backend.as_ref(),
                &packet,
                peer,
                log.as_ref(),
                limiter.as_deref(),
            )
            .await
            else {
                return;
            };
            match dns::encode_for_udp(&response, payload_size) {
                Ok(bytes) => {
                    if let Err(error) = socket.send_to(&bytes, peer).await {
                        tracing::warn!(%error, "answer could not be sent");
                    }
                }
                Err(error) => tracing::warn!(%error, "answer not encodable"),
            }
        });
    }
}
