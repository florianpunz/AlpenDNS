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
use hickory_net::dnssec::DnssecDnsHandle;
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
use crate::dnssec;
use crate::privacy;
use crate::resolve::ResolveError;

/// Wie viele Anfragen gleichzeitig auf einer Verbindung unterwegs sein dürfen.
/// Darüber wartet der Multiplexer, statt weitere Streams zu öffnen.
const MAX_ACTIVE_REQUESTS: usize = 256;

/// Ob dieser Fehler daher kommt, dass der Upstream nichts Prüfbares lieferte.
///
/// Siehe [`dnssec::from_error`]: dort steht, warum das etwas anderes ist als
/// eine faule Signatur, und was es kostet, die beiden zu verwechseln.
fn is_unproven(error: &hickory_net::NetError) -> bool {
    matches!(
        error,
        hickory_net::NetError::Dns(hickory_net::DnsError::Nsec { .. })
    )
}

/// Eine offene Verbindung, gegebenenfalls mit vorgeschalteter Validierung.
///
/// Der validierende Griff steht neben der Verbindung und wird nicht je Anfrage
/// neu gebaut: er führt einen Cache über bereits geprüfte DNSKEY- und
/// DS-Sätze, und den jedes Mal wegzuwerfen hieße, für jede Anfrage die halbe
/// Kette neu zu holen.
#[derive(Clone)]
struct Connection {
    exchange: DnsExchange<TokioRuntimeProvider>,
    /// Nur gesetzt, wenn `privacy.dnssec` an ist.
    validating: Option<DnssecDnsHandle<DnsExchange<TokioRuntimeProvider>>>,
}

/// Ein verschlüsselter Weg zu genau einem Upstream.
pub struct Transport {
    addr: UpstreamAddr,
    server_name: Arc<str>,
    timeout: Duration,
    privacy: privacy::Settings,
    tls: Arc<rustls::ClientConfig>,
    provider: TokioRuntimeProvider,
    /// Die offene Verbindung. `None` heißt: noch nicht oder nicht mehr verbunden.
    connection: Mutex<Option<Connection>>,
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
        self.send_checked(request).await.map(|(message, _)| message)
    }

    /// Wie [`Self::send`], gibt zusätzlich das DNSSEC-Urteil zurück.
    ///
    /// Getrennt, weil nur der Pool das Urteil in den Trace schreiben kann — er
    /// weiß, welcher Resolver geantwortet hat, der Transport nicht.
    pub async fn send_checked(
        &self,
        request: &Message,
    ) -> Result<(Message, Option<dnssec::Verdict>), ResolveError> {
        let connection = self.exchange().await?;

        // Unabhängig vom CD-Bit des Clients validieren: die Antwort landet im
        // gemeinsamen Cache (crate::caching), ein CD-Client würde sonst die
        // Prüfung für alle abschalten. Was der einzelne Client davon zu sehen
        // bekommt, entscheidet `dnssec::for_client` an der Außenkante.
        let validate = self.privacy.dnssec;

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

        // Beide Wege liefern denselben Strom; nur der validierende schiebt vor
        // der Antwort noch DNSKEY- und DS-Abfragen über dieselbe Verbindung.
        let request_out = DnsRequest::new(outbound.clone(), options);
        let mut stream: std::pin::Pin<
            Box<
                dyn futures_util::Stream<
                        Item = Result<hickory_proto::op::DnsResponse, hickory_net::NetError>,
                    > + Send,
            >,
        > = match (validate, connection.validating.as_ref()) {
            (true, Some(handle)) => Box::pin(handle.send(request_out)),
            _ => Box::pin(connection.exchange.send(request_out)),
        };
        // Ein Fehler aus dem validierenden Griff kann in Wahrheit ein Urteil
        // sein — hickory liefert ein negatives Ergebnis mit nicht aufgehendem
        // NSEC-Beweis als Fehler samt Antwort (crate::dnssec::from_error).
        // Dann ist die Verbindung heil und der Upstream nicht schuld.
        let mut settled = None;
        let mut response = match stream.next().await {
            Some(Ok(response)) => response.into_message(),
            Some(Err(error)) => match dnssec::from_error(&error) {
                Some((verdict, recovered)) => {
                    settled = Some(verdict);
                    recovered
                }
                // Kein Urteil, sondern ein Ausfall: der Upstream hat nichts
                // Prüfbares geliefert. Die Verbindung bleibt stehen — sie ist
                // nicht kaputt, der Inhalt war leer.
                None if is_unproven(&error) => return Err(ResolveError::Unproven),
                None => {
                    self.invalidate().await;
                    return Err(ResolveError::Upstream(error.to_string()));
                }
            },
            None => {
                self.invalidate().await;
                return Err(ResolveError::Timeout);
            }
        };

        // Die Query-ID verwaltet der Multiplexer der Verbindung, deshalb wird
        // sie hier nicht geprüft. Frage, Typ und Klasse schon: ein Upstream, der
        // etwas anderes beantwortet als gefragt, ist ein Fehler — egal ob aus
        // Bosheit oder aus Kaputtheit.
        // Eine aus dem Fehler geborgene Antwort trägt die Frage in der Form, in
        // der hickory sie gestellt hat; der Vergleich gegen unsere ausgehende
        // Nachricht wäre hier bedeutungslos.
        if settled.is_none() {
            dns::check_question(&outbound, &response).map_err(ResolveError::Mismatch)?;
        }

        // Die Query-ID gehört auf dieser Verbindung dem Multiplexer: er schreibt
        // sie beim Senden um und gibt sie in der Antwort nicht zurück. Der Client
        // erwartet aber seine eigene — und ebenso seine eigene Frage, die wir für
        // Padding und ECS angefasst haben.
        response.metadata.id = request.metadata.id;
        response.queries = request.queries.clone();

        if !validate {
            return Ok((response, None));
        }

        // `DnssecDnsHandle` stempelt jedem Record ein Urteil auf; das
        // Zusammenfassen und die Folgerung daraus sind unsere (crate::dnssec).
        // Bei Bogus wird kein anderer Upstream versucht: eine kaputte Zone ist
        // bei jedem Anbieter kaputt, und ein zweiter Versuch würde den Namen
        // nur einem weiteren Anbieter zeigen — das Gegenteil dessen, wofür
        // split_by_zone da ist (siehe pool::Pool::resolve).
        let verdict = match settled {
            Some(verdict) => dnssec::apply_verdict(&mut response, verdict),
            None => dnssec::apply(&mut response),
        }
        .inspect_err(|_| {
            tracing::warn!(
                upstream = %self.addr.socket_addr(),
                "Antwort verworfen: Signaturkette schließt nicht"
            );
        })?;
        Ok((response, Some(verdict)))
    }

    /// Liefert die offene Verbindung oder baut sie auf.
    ///
    /// Der Lock wird nur für das Klonen der Handles gehalten, nicht für die
    /// Anfrage selbst — `DnsExchange` multiplext intern, ein Lock über der
    /// Anfrage würde den Upstream auf eine Anfrage nach der anderen drosseln.
    async fn exchange(&self) -> Result<Connection, ResolveError> {
        let mut guard = self.connection.lock().await;
        if let Some(connection) = guard.as_ref() {
            return Ok(connection.clone());
        }
        let exchange = self.connect().await?;
        let connection = Connection {
            validating: self
                .privacy
                .dnssec
                .then(|| DnssecDnsHandle::new(exchange.clone())),
            exchange,
        };
        *guard = Some(connection.clone());
        Ok(connection)
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
    // Welcher Resolver das hier ist, weiß nur der Pool — den Schritt
    // `UpstreamUsed` schreibt deshalb er. Das DNSSEC-Urteil dagegen fällt hier
    // und nirgends sonst, und ohne diesen Eintrag stünde in der
    // Begründungskette nicht, dass überhaupt geprüft wurde.
    async fn resolve(
        &self,
        request: &Message,
        ctx: &mut crate::trace::Ctx,
    ) -> Result<Message, ResolveError> {
        let (response, verdict) = self.send_checked(request).await?;
        if let Some(verdict) = verdict {
            ctx.record(crate::trace::Step::DnssecChecked { verdict });
        }
        Ok(response)
    }
}
