//! Verschlüsselte Transporte zu einem Upstream: DoT, DoH, DoQ.
//!
//! Die Verbindung wird aufgebaut, wenn sie zum ersten Mal gebraucht wird, und
//! danach wiederverwendet — ein TLS-Handshake pro Anfrage würde den Zweck des
//! Cachings zunichtemachen. Bricht sie ab, wird sie beim nächsten Versuch neu
//! aufgebaut.
//!
//! Die Bytes auf dem Draht kommen aus `hickory-net` (ADR-0002). Was wir selbst
//! machen: wann verbunden wird, wann eine Verbindung als kaputt gilt, und die
//! Prüfung der Antwort gegen die Frage.

use std::sync::Arc;
use std::time::Duration;

use futures_util::StreamExt as _;
use hickory_net::DnsHandle as _;
use hickory_net::h2::HttpsClientStream;
use hickory_net::quic::QuicClientStream;
use hickory_net::runtime::RuntimeProvider as _;
use hickory_net::runtime::TokioRuntimeProvider;
use hickory_net::tls::tls_exchange;
use hickory_net::xfer::DnsExchange;
use hickory_proto::op::{DnsRequest, DnsRequestOptions, Message};
use rustls::pki_types::ServerName;
use tokio::sync::Mutex;

use crate::config::UpstreamAddr;
use crate::dns;
use crate::privacy;
use crate::resolve::ResolveError;

/// Wie viele Anfragen gleichzeitig auf einer Verbindung unterwegs sein dürfen.
/// Darüber wartet der Multiplexer, statt weitere Streams zu öffnen.
const MAX_ACTIVE_REQUESTS: usize = 256;

/// Ein verschlüsselter Weg zu genau einem Upstream.
pub struct Transport {
    addr: UpstreamAddr,
    server_name: Arc<str>,
    timeout: Duration,
    privacy: privacy::Settings,
    tls: Arc<rustls::ClientConfig>,
    provider: TokioRuntimeProvider,
    /// Die offene Verbindung. `None` heißt: noch nicht oder nicht mehr verbunden.
    connection: Mutex<Option<DnsExchange<TokioRuntimeProvider>>>,
}

// Von Hand, weil weder der Runtime-Provider noch die TLS-Konfiguration von
// hickory/rustls `Debug` implementieren.
impl std::fmt::Debug for Transport {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Transport")
            .field("addr", &self.addr)
            .field("server_name", &self.server_name)
            .field("timeout", &self.timeout)
            .finish_non_exhaustive()
    }
}

impl Transport {
    /// `server_name` ist der Name, gegen den das Zertifikat geprüft wird.
    pub fn new(
        addr: UpstreamAddr,
        server_name: &str,
        timeout: Duration,
        privacy: privacy::Settings,
        tls: Arc<rustls::ClientConfig>,
    ) -> Self {
        Self {
            addr,
            server_name: Arc::from(server_name),
            timeout,
            privacy,
            tls,
            provider: TokioRuntimeProvider::new(),
            connection: Mutex::new(None),
        }
    }

    /// Die Standard-TLS-Konfiguration: Mozillas Wurzelzertifikate, einkompiliert.
    ///
    /// Bewusst nicht der Zertifikatsspeicher des Systems: der Resolver soll auf
    /// einer frisch installierten Kiste genauso funktionieren wie auf einer
    /// gepflegten, und keine Überraschungen aus `/etc/ssl` ziehen.
    pub fn default_tls_config() -> Result<rustls::ClientConfig, ResolveError> {
        hickory_net::tls::client_config()
            .map_err(|e| ResolveError::Connect(format!("TLS-Konfiguration ungültig: {e}")))
    }

    pub const fn addr(&self) -> &UpstreamAddr {
        &self.addr
    }

    /// Stellt eine Anfrage und gibt die geprüfte Antwort zurück.
    pub async fn send(&self, request: &Message) -> Result<Message, ResolveError> {
        let exchange = self.exchange().await?;

        let mut outbound = request.clone();
        if self.privacy.strip_ecs {
            privacy::strip_ecs(&mut outbound);
        }
        if self.privacy.padding {
            // Scheitert das Kodieren, geht die Anfrage ungepolstert raus statt
            // gar nicht — Padding ist eine Härtung, kein Muss.
            if let Err(error) = privacy::pad_to_block(&mut outbound, privacy::PADDING_BLOCK) {
                tracing::debug!(%error, "Padding nicht möglich");
            }
        }

        let mut options = DnsRequestOptions::default();
        // 0x20 machen wir selbst (siehe crate::privacy), damit es für alle
        // Transporte gleich läuft und einen eigenen Test hat. Doppelt
        // randomisiert würde die Antwort nicht mehr zur Frage passen.
        options.case_randomization = false;
        // EDNS-Optionen setzt die Privacy-Schicht, bevor die Nachricht hier
        // ankommt; hickory soll sie nicht überschreiben.
        options.use_edns = outbound.edns.is_some();

        let mut stream = exchange.send(DnsRequest::new(outbound.clone(), options));
        let mut response = match stream.next().await {
            Some(Ok(response)) => response.into_message(),
            Some(Err(error)) => {
                self.invalidate().await;
                return Err(ResolveError::Upstream(error.to_string()));
            }
            None => {
                self.invalidate().await;
                return Err(ResolveError::Timeout);
            }
        };

        // Die Query-ID verwaltet der Multiplexer der Verbindung, deshalb wird
        // sie hier nicht geprüft. Frage, Typ und Klasse schon: ein Upstream, der
        // etwas anderes beantwortet als gefragt, ist ein Fehler — egal ob aus
        // Bosheit oder aus Kaputtheit.
        dns::check_question(&outbound, &response).map_err(ResolveError::Mismatch)?;

        // Die Query-ID gehört auf dieser Verbindung dem Multiplexer: er schreibt
        // sie beim Senden um und gibt sie in der Antwort nicht zurück. Der Client
        // erwartet aber seine eigene — und ebenso seine eigene Frage, die wir für
        // Padding und ECS angefasst haben.
        response.metadata.id = request.metadata.id;
        response.queries = request.queries.clone();
        Ok(response)
    }

    /// Liefert die offene Verbindung oder baut sie auf.
    ///
    /// Der Lock wird nur für das Klonen des Handles gehalten, nicht für die
    /// Anfrage selbst — `DnsExchange` multiplext intern, ein Lock über der
    /// Anfrage würde den Upstream auf eine Anfrage nach der anderen drosseln.
    async fn exchange(&self) -> Result<DnsExchange<TokioRuntimeProvider>, ResolveError> {
        let mut guard = self.connection.lock().await;
        if let Some(exchange) = guard.as_ref() {
            return Ok(exchange.clone());
        }
        let exchange = self.connect().await?;
        *guard = Some(exchange.clone());
        Ok(exchange)
    }

    async fn invalidate(&self) {
        *self.connection.lock().await = None;
    }

    async fn connect(&self) -> Result<DnsExchange<TokioRuntimeProvider>, ResolveError> {
        let server_name = ServerName::try_from(self.server_name.as_ref())
            .map_err(|e| ResolveError::Connect(format!("ungültiger tls_name: {e}")))?
            .to_owned();

        let exchange = match &self.addr {
            UpstreamAddr::Dot(addr) => tls_exchange(
                *addr,
                server_name,
                self.tls.as_ref().clone(),
                self.timeout,
                Some(MAX_ACTIVE_REQUESTS),
                self.provider.clone(),
            )
            .await
            .map_err(|e| ResolveError::Connect(e.to_string()))?,

            UpstreamAddr::Doh { addr, path } => {
                HttpsClientStream::builder(Arc::clone(&self.tls), self.provider.clone())
                    .exchange(
                        *addr,
                        Arc::clone(&self.server_name),
                        Arc::from(path.as_str()),
                    )
                    .await
                    .map_err(|e| ResolveError::Connect(e.to_string()))?
            }

            UpstreamAddr::Doq(addr) => {
                let bind = super::local_bind_addr(*addr);
                let socket = self
                    .provider
                    .quic_binder()
                    .ok_or_else(|| ResolveError::Connect("Runtime kann kein QUIC".to_owned()))?
                    .bind_quic(bind, *addr)
                    .map_err(|e| ResolveError::Connect(e.to_string()))?;
                QuicClientStream::builder()
                    .crypto_config(self.tls.as_ref().clone())
                    .exchange(
                        socket,
                        *addr,
                        Arc::clone(&self.server_name),
                        self.provider.clone(),
                    )
                    .await
                    .map_err(|e| ResolveError::Connect(e.to_string()))?
            }

            // Die Konfiguration lässt Klartext in einem Pool nicht zu; dieser
            // Zweig existiert nur, damit das Match vollständig ist.
            UpstreamAddr::Udp(_) => {
                return Err(ResolveError::Connect(
                    "Klartext-UDP ist kein verschlüsselter Transport".to_owned(),
                ));
            }
        };

        tracing::debug!(
            transport = self.addr.scheme(),
            server = %self.addr.socket_addr(),
            "Verbindung zum Upstream aufgebaut"
        );
        Ok(exchange)
    }
}

impl crate::resolve::ResolveBackend for Transport {
    fn resolve(
        &self,
        request: &Message,
        _ctx: &crate::trace::Ctx,
    ) -> impl std::future::Future<Output = Result<Message, ResolveError>> + Send {
        // Der Transport trägt nichts in den Trace ein; der Pool weiß, welcher
        // Resolver er ist, und schreibt den Schritt.
        self.send(request)
    }
}
