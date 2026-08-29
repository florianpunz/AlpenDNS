//! UDP-Listener.

use std::sync::Arc;

use tokio::net::UdpSocket;
use tokio_util::sync::CancellationToken;
use tokio_util::task::TaskTracker;

use crate::dns;
use crate::logging::QueryLog;
use crate::resolve::ResolveBackend;

/// Größte Anfrage, die wir über UDP entgegennehmen. Alles darüber ist entweder
/// kaputt oder gehört über TCP.
const MAX_UDP_REQUEST: usize = 4096;

pub(crate) async fn serve<B: ResolveBackend>(
    socket: Arc<UdpSocket>,
    backend: Arc<B>,
    log: Arc<QueryLog>,
    udp_payload_size: usize,
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
                    tracing::warn!(%error, "recv_from fehlgeschlagen");
                    continue;
                }
            },
        };

        let Some(packet) = buf.get(..len).map(<[u8]>::to_vec) else {
            continue;
        };

        // Die Antwort muss über denselben Socket an genau die Quelladresse
        // zurück, von der die Anfrage kam.
        let socket = Arc::clone(&socket);
        let backend = Arc::clone(&backend);
        let log = Arc::clone(&log);
        tracker.spawn(async move {
            let Some(response) =
                crate::server::handle_request(backend.as_ref(), &packet, peer, log.as_ref()).await
            else {
                return;
            };
            match dns::encode_for_udp(&response, udp_payload_size) {
                Ok(bytes) => {
                    if let Err(error) = socket.send_to(&bytes, peer).await {
                        tracing::warn!(%error, "Antwort konnte nicht gesendet werden");
                    }
                }
                Err(error) => tracing::warn!(%error, "Antwort nicht kodierbar"),
            }
        });
    }
}
