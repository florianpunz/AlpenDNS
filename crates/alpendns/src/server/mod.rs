//! Listener und Request-Pipeline.
//!
//! In Phase 1 ist die Pipeline kurz: parsen, weiterleiten, antworten. Cache,
//! Policy und Filter hängen sich in späteren Phasen zwischen `handle_request`
//! und das [`ResolveBackend`] (ARCHITECTURE.md §1).

mod tcp;
mod udp;

use std::net::SocketAddr;
use std::sync::Arc;

use hickory_proto::op::{Message, OpCode, ResponseCode};
use tokio::net::{TcpListener, UdpSocket};
use tokio_util::sync::CancellationToken;
use tokio_util::task::TaskTracker;

use crate::config::ServerConfig;
use crate::dns;
use crate::resolve::ResolveBackend;

/// Gemeinsamer Zustand aller Listener.
#[derive(Debug)]
pub struct Server<B> {
    backend: Arc<B>,
    /// Ab dieser Antwortgröße wird über UDP gekürzt und TC gesetzt.
    udp_payload_size: usize,
}

impl<B: ResolveBackend> Server<B> {
    pub fn new(backend: B, udp_payload_size: u16) -> Self {
        Self {
            backend: Arc::new(backend),
            udp_payload_size: usize::from(udp_payload_size),
        }
    }

    /// Öffnet alle konfigurierten Listener.
    ///
    /// Getrennt von [`Bound::run`], damit die tatsächlich vergebenen Ports vorher
    /// abfragbar sind — Tests binden auf Port 0 und brauchen die echte Adresse.
    pub async fn bind(self, config: &ServerConfig) -> std::io::Result<Bound<B>> {
        let mut udp = Vec::with_capacity(config.listen_udp.len());
        for addr in &config.listen_udp {
            udp.push(Arc::new(UdpSocket::bind(addr).await?));
        }
        let mut tcp = Vec::with_capacity(config.listen_tcp.len());
        for addr in &config.listen_tcp {
            tcp.push(TcpListener::bind(addr).await?);
        }
        Ok(Bound {
            server: self,
            udp,
            tcp,
        })
    }
}

/// Ein Server mit offenen Sockets, aber noch ohne laufende Schleifen.
#[derive(Debug)]
pub struct Bound<B> {
    server: Server<B>,
    udp: Vec<Arc<UdpSocket>>,
    tcp: Vec<TcpListener>,
}

impl<B: ResolveBackend> Bound<B> {
    /// Tatsächlich gebundene UDP-Adressen.
    pub fn udp_addrs(&self) -> Vec<SocketAddr> {
        self.udp
            .iter()
            .filter_map(|s| s.local_addr().ok())
            .collect()
    }

    /// Tatsächlich gebundene TCP-Adressen.
    pub fn tcp_addrs(&self) -> Vec<SocketAddr> {
        self.tcp
            .iter()
            .filter_map(|l| l.local_addr().ok())
            .collect()
    }

    /// Läuft, bis `shutdown` ausgelöst wird.
    ///
    /// Danach werden keine neuen Anfragen mehr angenommen, aber alle bereits
    /// laufenden zu Ende beantwortet, bevor die Funktion zurückkehrt.
    pub async fn run(self, shutdown: CancellationToken) {
        let tracker = TaskTracker::new();

        for socket in self.udp {
            tracker.spawn(udp::serve(
                socket,
                Arc::clone(&self.server.backend),
                self.server.udp_payload_size,
                shutdown.clone(),
                tracker.clone(),
            ));
        }
        for listener in self.tcp {
            tracker.spawn(tcp::serve(
                listener,
                Arc::clone(&self.server.backend),
                shutdown.clone(),
                tracker.clone(),
            ));
        }

        tracker.close();
        tracker.wait().await;
    }
}

/// Beantwortet ein einzelnes Anfragepaket.
///
/// Gibt `None` zurück, wenn es nichts gibt, worauf man antworten könnte — dann
/// wird das Paket kommentarlos verworfen.
///
/// Geloggt wird hier bewusst ohne Query-Namen (CLAUDE.md B.1, Regel 3). Die
/// Log-Schicht, die Namen abhängig vom konfigurierten Modus behandeln darf,
/// kommt in Phase 6.
pub(crate) async fn handle_request<B: ResolveBackend>(backend: &B, raw: &[u8]) -> Option<Message> {
    let request = match Message::from_vec(raw) {
        Ok(request) => request,
        Err(_) => {
            tracing::debug!(bytes = raw.len(), "Anfrage nicht dekodierbar");
            return dns::format_error(raw);
        }
    };

    if request.queries.is_empty() {
        return Some(dns::error_response(&request, ResponseCode::FormErr));
    }
    if request.metadata.op_code != OpCode::Query {
        return Some(dns::error_response(&request, ResponseCode::NotImp));
    }

    match backend.resolve(&request).await {
        Ok(mut response) => {
            response.metadata.recursion_available = true;
            Some(response)
        }
        Err(error) => {
            tracing::warn!(%error, "Auflösung fehlgeschlagen");
            Some(dns::error_response(&request, ResponseCode::ServFail))
        }
    }
}
