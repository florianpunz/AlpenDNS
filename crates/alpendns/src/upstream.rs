//! Weiterleiten an einen Upstream-Resolver.
//!
//! Phase 1 kann genau einen Upstream über Klartext-UDP. Phase 3 ersetzt das durch
//! DoT/DoH/DoQ und einen Pool mit Auswahlstrategie; die Schnittstelle nach oben
//! ([`ResolveBackend`]) ändert sich dabei nicht.

use std::net::{IpAddr, SocketAddr};
use std::time::Duration;

use hickory_proto::op::Message;

use crate::dns;
use crate::resolve::{ResolveBackend, ResolveError};

/// Größter Puffer, den wir für eine UDP-Antwort vom Upstream bereithalten.
/// Mehr als 4096 Byte kommen über UDP nicht sinnvoll an; der Rest ist TCP.
const MAX_UDP_RESPONSE: usize = 4096;

/// Leitet Anfragen an genau einen Upstream weiter.
#[derive(Debug, Clone)]
pub struct ForwardBackend {
    upstream: SocketAddr,
    timeout: Duration,
}

impl ForwardBackend {
    /// `timeout` ist das Zeitbudget für die Upstream-Anfrage und damit in dieser
    /// Phase auch das für die gesamte Client-Anfrage — es gibt sonst nichts,
    /// was dauern könnte.
    pub const fn new(upstream: SocketAddr, timeout: Duration) -> Self {
        Self { upstream, timeout }
    }

    /// Lokale Adresse für den ausgehenden Socket, passend zur Adressfamilie des
    /// Upstreams. Ein v4-Socket kann keinen v6-Upstream erreichen.
    const fn local_bind_addr(&self) -> SocketAddr {
        match self.upstream.ip() {
            IpAddr::V4(_) => SocketAddr::new(IpAddr::V4(std::net::Ipv4Addr::UNSPECIFIED), 0),
            IpAddr::V6(_) => SocketAddr::new(IpAddr::V6(std::net::Ipv6Addr::UNSPECIFIED), 0),
        }
    }
}

impl ResolveBackend for ForwardBackend {
    fn resolve(
        &self,
        request: &Message,
    ) -> impl std::future::Future<Output = Result<Message, ResolveError>> + Send {
        let upstream = self.upstream;
        let timeout = self.timeout;
        let bind_addr = self.local_bind_addr();

        // Gegenüber dem Upstream benutzen wir eine eigene, zufällige Query-ID.
        // Die ID des Clients ist von außen wählbar; sie nach draußen zu
        // übernehmen würde einem Angreifer die Hälfte der Arbeit abnehmen, eine
        // gefälschte Antwort passend zu machen.
        let client_id = request.metadata.id;
        let mut outbound = request.clone();
        outbound.metadata.id = rand::random();

        async move {
            let bytes = outbound
                .to_vec()
                .map_err(|e| ResolveError::Encode(e.to_string()))?;

            let socket = tokio::net::UdpSocket::bind(bind_addr).await?;
            // `connect` lässt den Kernel alles verwerfen, was nicht von der
            // Upstream-Adresse kommt. Das ist die erste, billigste Hürde gegen
            // Off-Path-Antworten; die inhaltliche Prüfung folgt darunter.
            socket.connect(upstream).await?;
            socket.send(&bytes).await?;

            let mut buf = vec![0_u8; MAX_UDP_RESPONSE];
            let len = tokio::time::timeout(timeout, socket.recv(&mut buf))
                .await
                .map_err(|_| ResolveError::Timeout)??;
            let packet = buf
                .get(..len)
                .ok_or_else(|| ResolveError::Malformed("Länge außerhalb des Puffers".to_owned()))?;

            let mut response =
                Message::from_vec(packet).map_err(|e| ResolveError::Malformed(e.to_string()))?;
            dns::check_response(&outbound, &response).map_err(ResolveError::Mismatch)?;

            // Der Client erwartet seine eigene ID zurück.
            response.metadata.id = client_id;
            Ok(response)
        }
    }
}
