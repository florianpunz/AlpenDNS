//! Upstreams: Transporte, Pool und Auswahlstrategien.
//!
//! [`ForwardBackend`] ist der Klartext-Weg über UDP. Er ist nach B.1 Regel 7 nur
//! noch für `forward_zone`-Einträge ins eigene Netz zulässig; die Konfiguration
//! lehnt ihn in einem `upstream_pool` ab. Alles, was ins Internet geht, läuft
//! über [`transport::Transport`] und damit über DoT, DoH oder DoQ.

pub mod odoh;
pub mod pool;
pub mod strategy;
pub mod transport;

use std::net::{IpAddr, SocketAddr};
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use hickory_proto::op::Message;
use hickory_proto::rr::Name;

use crate::dns;
use crate::privacy;
use crate::resolve::{ResolveBackend, ResolveError};

/// Ein verschlüsselter Upstream — direkt oder über einen ODoH-Proxy.
///
/// Ein Enum statt zweier Pool-Typen: `Pool<B, C>` ist über seinen Inhalt
/// generisch, und zwei verschiedene Inhalte hießen zwei verschiedene Pool-Typen
/// durch das ganze Binary bis in die API-Struktur hinein. Die Wahl fällt einmal
/// beim Start und ändert sich danach nicht; ein Match je Anfrage ist dafür der
/// billigere Preis.
#[derive(Debug)]
pub enum Encrypted {
    /// DoT, DoH oder DoQ direkt zum Resolver.
    Direct(transport::Transport),
    /// DoH zum Resolver, aber über einen Proxy und für ihn verschlüsselt.
    Oblivious(odoh::OdohBackend),
}

impl ResolveBackend for Encrypted {
    async fn resolve(
        &self,
        request: &Message,
        ctx: &mut crate::trace::Ctx,
    ) -> Result<Message, ResolveError> {
        match self {
            Self::Direct(transport) => transport.resolve(request, ctx).await,
            Self::Oblivious(backend) => backend.resolve(request, ctx).await,
        }
    }
}

/// Größter Puffer, den wir für eine UDP-Antwort vom Upstream bereithalten.
/// Mehr als 4096 Byte kommen über UDP nicht sinnvoll an; der Rest ist TCP.
const MAX_UDP_RESPONSE: usize = 4096;

/// Leitet Anfragen im Klartext über UDP weiter.
///
/// Nur für Zonen im eigenen Netz. Ein LAN-Nameserver spricht in aller Regel kein
/// DoT, und die Anfrage verlässt das eigene Netz nicht.
#[derive(Debug)]
pub struct ForwardBackend {
    upstream: SocketAddr,
    timeout: Duration,
    privacy: privacy::Settings,
    client_cookie: [u8; privacy::CLIENT_COOKIE_LEN],
    /// Was der Server uns beim letzten Mal als Cookie mitgegeben hat.
    server_cookie: Mutex<Option<Vec<u8>>>,
    /// 0x20 verträgt nicht jeder Server. Antwortet einer mit veränderter
    /// Schreibweise, wird es für ihn abgeschaltet, statt Anfragen scheitern zu
    /// lassen (Fallstrick aus der Roadmap, Phase 3).
    dns0x20: AtomicBool,
}

impl ForwardBackend {
    pub fn new(upstream: SocketAddr, timeout: Duration, privacy: privacy::Settings) -> Self {
        Self {
            upstream,
            timeout,
            privacy,
            client_cookie: privacy::new_client_cookie(),
            server_cookie: Mutex::new(None),
            dns0x20: AtomicBool::new(privacy.dns0x20),
        }
    }

    fn stored_server_cookie(&self) -> Option<Vec<u8>> {
        self.server_cookie
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    fn remember_server_cookie(&self, response: &Message) {
        if let Some(cookie) = privacy::server_cookie(response) {
            *self
                .server_cookie
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(cookie);
        }
    }

    /// Baut die Nachricht, die tatsächlich rausgeht.
    ///
    /// Gibt zusätzlich den 0x20-Namen zurück, gegen den die Antwort geprüft
    /// wird — `None`, wenn 0x20 aus ist.
    fn prepare(&self, request: &Message) -> (Message, Option<Name>) {
        let mut outbound = request.clone();
        // Gegenüber dem Upstream benutzen wir eine eigene, zufällige Query-ID.
        // Die des Clients ist von außen wählbar; sie nach draußen zu übernehmen
        // würde einem Angreifer die halbe Arbeit abnehmen.
        outbound.metadata.id = rand::random();

        if self.privacy.strip_ecs {
            privacy::strip_ecs(&mut outbound);
        }
        if self.privacy.cookies {
            privacy::set_cookie(
                &mut outbound,
                &self.client_cookie,
                self.stored_server_cookie().as_deref(),
            );
        }

        let expected = if self.dns0x20.load(Ordering::Relaxed) {
            let original = outbound.queries.first().map(|q| q.name().clone());
            original.and_then(|name| {
                let randomized = privacy::randomize_case(&name);
                privacy::replace_question_name(&mut outbound, randomized.clone())?;
                Some(randomized)
            })
        } else {
            None
        };

        (outbound, expected)
    }

    async fn exchange(&self, outbound: &Message) -> Result<Message, ResolveError> {
        let bytes = outbound
            .to_vec()
            .map_err(|e| ResolveError::Encode(e.to_string()))?;

        let socket = tokio::net::UdpSocket::bind(local_bind_addr(self.upstream)).await?;
        // `connect` lässt den Kernel alles verwerfen, was nicht von der
        // Upstream-Adresse kommt. Das ist die erste, billigste Hürde gegen
        // Off-Path-Antworten; die inhaltliche Prüfung folgt darunter.
        socket.connect(self.upstream).await?;
        socket.send(&bytes).await?;

        let mut buf = vec![0_u8; MAX_UDP_RESPONSE];
        let len = tokio::time::timeout(self.timeout, socket.recv(&mut buf))
            .await
            .map_err(|_| ResolveError::Timeout)??;
        let packet = buf
            .get(..len)
            .ok_or_else(|| ResolveError::Malformed("Länge außerhalb des Puffers".to_owned()))?;

        let response =
            Message::from_vec(packet).map_err(|e| ResolveError::Malformed(e.to_string()))?;
        dns::check_response(outbound, &response).map_err(ResolveError::Mismatch)?;
        Ok(response)
    }
}

impl ResolveBackend for ForwardBackend {
    fn resolve(
        &self,
        request: &Message,
        _ctx: &mut crate::trace::Ctx,
    ) -> impl std::future::Future<Output = Result<Message, ResolveError>> + Send {
        let client_id = request.metadata.id;
        let request = request.clone();

        async move {
            let (outbound, expected_case) = self.prepare(&request);
            let mut response = self.exchange(&outbound).await?;

            // 0x20: die Antwort muss die Schreibweise exakt gespiegelt haben.
            if let Some(expected) = expected_case {
                let echoed = response.queries.first().map(|q| q.name().clone());
                if !echoed.is_some_and(|name| privacy::echoed_same_case(&expected, &name)) {
                    // Nicht scheitern lassen: der Server kann 0x20 schlicht
                    // nicht. Einmal ohne wiederholen und es für ihn abschalten.
                    self.dns0x20.store(false, Ordering::Relaxed);
                    tracing::warn!(
                        upstream = %self.upstream,
                        "Upstream spiegelt die Schreibweise nicht; 0x20 wird für ihn abgeschaltet"
                    );
                    let (retry, _) = self.prepare(&request);
                    response = self.exchange(&retry).await?;
                }
            }

            self.remember_server_cookie(&response);
            // Der Client erwartet seine eigene ID und seine eigene Schreibweise.
            response.metadata.id = client_id;
            response.queries = request.queries.clone();
            Ok(response)
        }
    }
}

/// Lokale Adresse für einen ausgehenden Socket, passend zur Adressfamilie des
/// Ziels. Ein v4-Socket kann keinen v6-Upstream erreichen.
pub const fn local_bind_addr(target: SocketAddr) -> SocketAddr {
    match target.ip() {
        IpAddr::V4(_) => SocketAddr::new(IpAddr::V4(std::net::Ipv4Addr::UNSPECIFIED), 0),
        IpAddr::V6(_) => SocketAddr::new(IpAddr::V6(std::net::Ipv6Addr::UNSPECIFIED), 0),
    }
}
