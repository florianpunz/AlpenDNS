//! TCP-Listener mit Längenpräfix nach RFC 1035 §4.2.2.

use std::sync::Arc;
use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio_util::sync::CancellationToken;
use tokio_util::task::TaskTracker;

use crate::resolve::ResolveBackend;

/// Wie lange eine offene Verbindung ohne neue Anfrage warten darf, bevor sie
/// geschlossen wird (RFC 7766 §6.2.3 empfiehlt einige Sekunden). Ohne dieses
/// Limit hält eine Handvoll stiller Verbindungen den Server offen.
const IDLE_TIMEOUT: Duration = Duration::from_secs(10);

pub(crate) async fn serve<B: ResolveBackend>(
    listener: TcpListener,
    backend: Arc<B>,
    shutdown: CancellationToken,
    tracker: TaskTracker,
) {
    loop {
        let (stream, _peer) = tokio::select! {
            () = shutdown.cancelled() => break,
            result = listener.accept() => match result {
                Ok(accepted) => accepted,
                Err(error) => {
                    tracing::warn!(%error, "accept fehlgeschlagen");
                    continue;
                }
            },
        };

        let backend = Arc::clone(&backend);
        let shutdown = shutdown.clone();
        tracker.spawn(async move {
            if let Err(error) = handle_connection(stream, backend.as_ref(), &shutdown).await {
                tracing::debug!(%error, "TCP-Verbindung beendet");
            }
        });
    }
}

/// Beantwortet Anfragen auf einer Verbindung, bis der Client sie schließt.
async fn handle_connection<B: ResolveBackend>(
    mut stream: TcpStream,
    backend: &B,
    shutdown: &CancellationToken,
) -> std::io::Result<()> {
    loop {
        let mut len_buf = [0_u8; 2];
        let read = tokio::select! {
            () = shutdown.cancelled() => return Ok(()),
            result = tokio::time::timeout(IDLE_TIMEOUT, stream.read_exact(&mut len_buf)) => result,
        };
        match read {
            // Zeitüberschreitung im Leerlauf ist der Normalfall, kein Fehler.
            Err(_elapsed) => return Ok(()),
            // Sauberes Verbindungsende zwischen zwei Anfragen.
            Ok(Err(error)) if error.kind() == std::io::ErrorKind::UnexpectedEof => return Ok(()),
            Ok(Err(error)) => return Err(error),
            Ok(Ok(_)) => {}
        }

        let mut packet = vec![0_u8; usize::from(u16::from_be_bytes(len_buf))];
        stream.read_exact(&mut packet).await?;

        let Some(response) = crate::server::handle_request(backend, &packet).await else {
            continue;
        };
        let bytes = match response.to_vec() {
            Ok(bytes) => bytes,
            Err(error) => {
                tracing::warn!(%error, "Antwort nicht kodierbar");
                continue;
            }
        };
        // Über TCP wird nicht gekürzt; die Längenangabe ist 16 Bit, größer geht
        // eine DNS-Nachricht ohnehin nicht.
        let Ok(len) = u16::try_from(bytes.len()) else {
            tracing::warn!(len = bytes.len(), "Antwort passt nicht in das Längenpräfix");
            continue;
        };
        stream.write_all(&len.to_be_bytes()).await?;
        stream.write_all(&bytes).await?;
        stream.flush().await?;
    }
}
