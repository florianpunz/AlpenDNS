//! Oblivious DoH (RFC 9230): der Proxy kennt die Adresse, das Ziel die Frage,
//! niemand beides.
//!
//! Alle anderen Transporte verteilen das Problem "der Upstream kennt deine IP"
//! nur — `split_by_zone` sorgt dafür, dass jeder Anbieter bloß einen Ausschnitt
//! sieht, aber jeder sieht ihn samt Absender. ODoH löst es: die Anfrage wird
//! für das Ziel verschlüsselt und über einen Proxy geschickt. Der Proxy sieht
//! die Adresse und einen undurchsichtigen Block, das Ziel sieht die Frage und
//! als Absender den Proxy.
//!
//! **Die Bedingung, ohne die das Theater ist:** Proxy und Ziel dürfen nicht
//! demselben Betreiber gehören. Das kann der Code nicht prüfen; es steht in
//! [FEATURES.md](../../../../docs/FEATURES.md) P4 und in der Beispiel-
//! konfiguration.
//!
//! **Was hier trotzdem sichtbar bleibt:** einmal je Prozessstart holt sich
//! dieser Transport den öffentlichen Schlüssel des Ziels direkt von dessen
//! `/.well-known/odohconfigs`. Diese eine Verbindung geht nicht über den Proxy,
//! das Ziel sieht dabei also die Adresse — aber keine einzige Frage. Der Weg
//! über den Proxy stünde offen, wenn Proxys das anböten; sie nehmen
//! ausschließlich ODoH-Nachrichten entgegen. Vermerkt in ADR-0017.
//!
//! Das Wire-Format und die HPKE-Rechnerei kommen aus `odoh-rs` (ADR-0017,
//! dieselbe Begründung wie ADR-0002 für DNS-Nachrichten). Unser Anteil: wann
//! der Schlüssel geholt wird, wie die HTTP-Anfrage aussieht, und die Prüfung
//! der Antwort gegen die Frage.

use std::sync::Arc;
use std::time::Duration;

use hickory_proto::op::Message;
use odoh_rs::{
    ObliviousDoHConfigContents, ObliviousDoHConfigs, ObliviousDoHMessage,
    ObliviousDoHMessagePlaintext, OdohSecret,
};
use tokio::sync::Mutex;

use crate::dns;
use crate::privacy;
use crate::resolve::ResolveError;

/// Der Pfad, unter dem ein ODoH-Ziel seine Schlüssel veröffentlicht (RFC 9230 §6.1).
const CONFIG_PATH: &str = "/.well-known/odohconfigs";

/// Die übliche Adresse, unter der ein Ziel seinen Schlüssel veröffentlicht.
///
/// Steht als eigene Funktion da, weil [`OdohTransport::new`] die Adresse
/// entgegennimmt statt sie zu bauen: RFC 9230 §6.1 lässt ausdrücklich offen,
/// woher die Konfiguration kommt, und ein Aufrufer, der sie anderswoher hat,
/// soll sie nicht über einen Umweg unterschieben müssen.
pub fn well_known_config_url(target_host: &str) -> String {
    format!("https://{target_host}{CONFIG_PATH}")
}

/// Content-Type für ODoH-Nachrichten. Kommt aus `odoh-rs`, damit hier keine
/// zweite Schreibweise derselben Zeichenkette steht.
const ODOH_CONTENT_TYPE: &str = odoh_rs::ODOH_HTTP_HEADER;

/// Zufall für die HPKE-Kapselung.
///
/// `odoh-rs` führt die Traits aus `hpke::rand_core` in seiner Signatur; unser
/// `rand` ist eine andere Hauptversion. Die zehn Zeilen hier sind billiger als
/// ein zweites Zufallssystem im Baum.
struct Random;

impl hpke::rand_core::RngCore for Random {
    fn next_u32(&mut self) -> u32 {
        rand::random()
    }

    fn next_u64(&mut self) -> u64 {
        rand::random()
    }

    fn fill_bytes(&mut self, dest: &mut [u8]) {
        for byte in dest.iter_mut() {
            *byte = rand::random();
        }
    }
}

impl hpke::rand_core::CryptoRng for Random {}

/// Ein ODoH-Weg zu genau einem Ziel, über genau einen Proxy.
pub struct OdohTransport {
    /// Vollständige URL des Proxys, so wie sie konfiguriert wurde.
    proxy: String,
    /// Wo der öffentliche Schlüssel des Ziels liegt.
    config_url: String,
    /// Der Name des Ziels, wie ihn der Proxy zu sehen bekommt.
    target_host: Arc<str>,
    /// Der Pfad beim Ziel, üblicherweise `/dns-query`.
    target_path: Arc<str>,
    http: reqwest::Client,
    timeout: Duration,
    privacy: privacy::Settings,
    /// Der öffentliche Schlüssel des Ziels. Einmal geholt, danach gehalten:
    /// jede Anfrage neu zu holen wäre eine zweite Verbindung je Query und
    /// zusätzlich ein Muster, an dem das Ziel Aktivität ablesen könnte.
    config: Mutex<Option<ObliviousDoHConfigContents>>,
}

impl std::fmt::Debug for OdohTransport {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OdohTransport")
            .field("proxy", &self.proxy)
            .field("target_host", &self.target_host)
            .field("target_path", &self.target_path)
            .finish_non_exhaustive()
    }
}

impl OdohTransport {
    /// `proxy` ist die vollständige URL des Proxys, `target_host` der Name des
    /// Ziels und `target_path` sein DoH-Pfad.
    ///
    /// `config_url` ist die Adresse des öffentlichen Schlüssels; im Normalfall
    /// liefert [`well_known_config_url`] sie.
    ///
    /// `target_addr` pinnt den Namen des Ziels auf eine bekannte Adresse: der
    /// Schlüsselabruf soll nicht seinerseits eine Namensauflösung brauchen —
    /// die wäre wieder DNS, und die will dieser Prozess ja gerade erst
    /// bereitstellen.
    pub fn new(
        proxy: String,
        target_host: &str,
        target_path: &str,
        target_addr: std::net::SocketAddr,
        config_url: String,
        timeout: Duration,
        privacy: privacy::Settings,
    ) -> Result<Self, ResolveError> {
        // Dasselbe Krypto-Backend wie beim Listen-Download; ohne installierten
        // Provider baut reqwest keinen TLS-Client.
        crate::filter::source::install_crypto_provider();
        let http = reqwest::Client::builder()
            .timeout(timeout)
            .resolve(target_host, target_addr)
            .build()
            .map_err(|e| ResolveError::Connect(format!("HTTP-Client für ODoH: {e}")))?;
        Ok(Self {
            proxy,
            config_url,
            target_host: Arc::from(target_host),
            target_path: Arc::from(target_path),
            http,
            timeout,
            privacy,
            config: Mutex::new(None),
        })
    }

    /// Der öffentliche Schlüssel des Ziels, notfalls frisch geholt.
    async fn target_config(&self) -> Result<ObliviousDoHConfigContents, ResolveError> {
        let mut guard = self.config.lock().await;
        if let Some(config) = guard.as_ref() {
            return Ok(config.clone());
        }
        let fetched = self.fetch_config().await?;
        *guard = Some(fetched.clone());
        Ok(fetched)
    }

    async fn fetch_config(&self) -> Result<ObliviousDoHConfigContents, ResolveError> {
        let url = &self.config_url;
        let response = self
            .http
            .get(url)
            .timeout(self.timeout)
            .send()
            .await
            .map_err(|e| ResolveError::Connect(format!("ODoH-Konfiguration von {url}: {e}")))?;
        if !response.status().is_success() {
            return Err(ResolveError::Connect(format!(
                "ODoH-Konfiguration von {url}: HTTP {}",
                response.status()
            )));
        }
        let body = response
            .bytes()
            .await
            .map_err(|e| ResolveError::Connect(format!("ODoH-Konfiguration von {url}: {e}")))?;
        parse_config(&body)
    }

    /// Baut die URL, unter der der Proxy an das Ziel weiterreicht.
    fn proxy_url(&self) -> String {
        let separator = if self.proxy.contains('?') { '&' } else { '?' };
        format!(
            "{}{separator}targethost={}&targetpath={}",
            self.proxy,
            urlencode(&self.target_host),
            urlencode(&self.target_path)
        )
    }

    /// Stellt eine Anfrage und gibt die geprüfte Antwort zurück.
    pub async fn send(&self, request: &Message) -> Result<Message, ResolveError> {
        let config = self.target_config().await?;

        let mut outbound = request.clone();
        // Gegenüber dem Ziel eine eigene, zufällige ID: die des Clients ist von
        // außen wählbar.
        outbound.metadata.id = rand::random();
        if self.privacy.strip_ecs {
            privacy::strip_ecs(&mut outbound);
        }
        if self.privacy.padding {
            if let Err(error) = privacy::pad_to_block(&mut outbound, privacy::PADDING_BLOCK) {
                tracing::debug!(%error, "Padding nicht möglich");
            }
        }

        let wire = outbound
            .to_vec()
            .map_err(|e| ResolveError::Encode(e.to_string()))?;
        // Die Polsterung steckt schon in der DNS-Nachricht (siehe oben), damit
        // es nur einen Mechanismus dafür gibt und nur einen Zähler.
        let plaintext = ObliviousDoHMessagePlaintext::new(&wire, 0);
        let (encrypted, secret) = odoh_rs::encrypt_query(&plaintext, &config, &mut Random)
            .map_err(|e| ResolveError::Encode(format!("ODoH-Verschlüsselung: {e}")))?;

        let answer = self.post(&encrypted).await?;
        let response = decrypt(&plaintext, &answer, secret)?;

        let mut response =
            Message::from_vec(&response).map_err(|e| ResolveError::Malformed(e.to_string()))?;
        // Dieselbe Prüfung wie bei jedem anderen Transport: ein Ziel, das etwas
        // anderes beantwortet als gefragt, ist ein Fehler.
        dns::check_response(&outbound, &response).map_err(ResolveError::Mismatch)?;

        response.metadata.id = request.metadata.id;
        response.queries = request.queries.clone();
        Ok(response)
    }

    async fn post(&self, message: &ObliviousDoHMessage) -> Result<Vec<u8>, ResolveError> {
        let body = odoh_rs::compose(message)
            .map_err(|e| ResolveError::Encode(format!("ODoH-Nachricht: {e}")))?
            .freeze();
        let response = self
            .http
            .post(self.proxy_url())
            .header(reqwest::header::CONTENT_TYPE, ODOH_CONTENT_TYPE)
            .header(reqwest::header::ACCEPT, ODOH_CONTENT_TYPE)
            .timeout(self.timeout)
            .body(body.to_vec())
            .send()
            .await
            .map_err(|e| {
                if e.is_timeout() {
                    ResolveError::Timeout
                } else {
                    ResolveError::Upstream(format!("ODoH-Proxy: {e}"))
                }
            })?;
        if !response.status().is_success() {
            return Err(ResolveError::Upstream(format!(
                "ODoH-Proxy antwortete mit HTTP {}",
                response.status()
            )));
        }
        response
            .bytes()
            .await
            .map(|bytes| bytes.to_vec())
            .map_err(|e| ResolveError::Upstream(format!("ODoH-Antwort: {e}")))
    }
}

/// Liest die veröffentlichten Schlüssel eines Ziels und nimmt den ersten
/// unterstützten.
///
/// Ein Ziel darf mehrere anbieten, etwa während eines Schlüsselwechsels.
/// `odoh-rs` sortiert die aus, deren Verfahren es nicht kann.
fn parse_config(body: &[u8]) -> Result<ObliviousDoHConfigContents, ResolveError> {
    let mut cursor = body;
    let configs: ObliviousDoHConfigs = odoh_rs::parse(&mut cursor)
        .map_err(|e| ResolveError::Connect(format!("ODoH-Konfiguration unlesbar: {e}")))?;
    configs
        .supported()
        .into_iter()
        .next()
        .map(Into::into)
        .ok_or_else(|| {
            ResolveError::Connect(
                "das Ziel bietet kein von uns unterstütztes ODoH-Verfahren an".to_owned(),
            )
        })
}

/// Entschlüsselt die Antwort des Ziels.
fn decrypt(
    query: &ObliviousDoHMessagePlaintext,
    body: &[u8],
    secret: OdohSecret,
) -> Result<Vec<u8>, ResolveError> {
    let mut cursor = body;
    let message: ObliviousDoHMessage = odoh_rs::parse(&mut cursor)
        .map_err(|e| ResolveError::Malformed(format!("ODoH-Antwort unlesbar: {e}")))?;
    let plaintext = odoh_rs::decrypt_response(query, &message, secret).map_err(|e| {
        // Das ist der interessante Fehlerfall: entweder hat der Proxy etwas
        // verändert, oder das Ziel hat mit einem anderen Schlüssel gearbeitet.
        ResolveError::Malformed(format!("ODoH-Antwort nicht entschlüsselbar: {e}"))
    })?;
    Ok(plaintext.into_msg().to_vec())
}

/// Prozentkodierung für die zwei Parameter, die an den Proxy gehen.
///
/// Von Hand statt über ein Dependency: es geht um Hostnamen und Pfade, und die
/// bestehen fast immer schon aus erlaubten Zeichen. Erlaubt bleibt genau die
/// Menge der "unreserved characters" aus RFC 3986 §2.3 — auch der Schrägstrich
/// wird maskiert, weil er hier in einem *Parameterwert* steht und nicht im Pfad
/// der URL.
fn urlencode(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for byte in value.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~') {
            out.push(char::from(byte));
        } else {
            out.push_str(&format!("%{byte:02X}"));
        }
    }
    out
}

/// Der ODoH-Transport als `DnsHandle`, damit `DnssecDnsHandle` sich davorhängen
/// kann.
///
/// Ohne diesen Umweg gäbe es mit eingeschaltetem ODoH gar keine Validierung —
/// eine Einstellung, die still nichts tut, ist schlimmer als eine, die es nicht
/// gibt (B.1 Regel 5).
#[derive(Clone)]
struct OdohHandle(Arc<OdohTransport>);

impl hickory_net::DnsHandle for OdohHandle {
    type Response = std::pin::Pin<
        Box<
            dyn futures_util::Stream<
                    Item = Result<hickory_proto::op::DnsResponse, hickory_net::NetError>,
                > + Send,
        >,
    >;
    type Runtime = hickory_net::runtime::TokioRuntimeProvider;

    fn send(&self, request: hickory_proto::op::DnsRequest) -> Self::Response {
        let transport = Arc::clone(&self.0);
        Box::pin(futures_util::stream::once(Box::pin(async move {
            let message = transport
                .send(&request)
                .await
                .map_err(|error| hickory_net::NetError::from(error.to_string()))?;
            hickory_proto::op::DnsResponse::from_message(message)
                .map_err(|error| hickory_net::NetError::from(error.to_string()))
        })))
    }
}

/// Ein ODoH-Upstream, wahlweise mit vorgeschalteter DNSSEC-Validierung.
///
/// Der validierende Griff steht *neben* dem Transport und nicht darin: er hält
/// selbst einen `Arc` darauf, und ein Feld im Transport wäre ein Zyklus, der
/// nie freigegeben würde.
pub struct OdohBackend {
    transport: Arc<OdohTransport>,
    validating: Option<hickory_net::dnssec::DnssecDnsHandle<OdohHandle>>,
}

impl std::fmt::Debug for OdohBackend {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OdohBackend")
            .field("transport", &self.transport)
            .field("validating", &self.validating.is_some())
            .finish()
    }
}

impl OdohBackend {
    pub fn new(transport: OdohTransport) -> Self {
        let dnssec = transport.privacy.dnssec;
        let transport = Arc::new(transport);
        Self {
            validating: dnssec.then(|| {
                hickory_net::dnssec::DnssecDnsHandle::new(OdohHandle(Arc::clone(&transport)))
            }),
            transport,
        }
    }
}

impl crate::resolve::ResolveBackend for OdohTransport {
    fn resolve(
        &self,
        request: &Message,
        _ctx: &mut crate::trace::Ctx,
    ) -> impl std::future::Future<Output = Result<Message, ResolveError>> + Send {
        // Wie beim verschlüsselten Transport: welcher Resolver das ist, weiß
        // der Pool, und er schreibt den Schritt.
        self.send(request)
    }
}

impl crate::resolve::ResolveBackend for OdohBackend {
    async fn resolve(
        &self,
        request: &Message,
        ctx: &mut crate::trace::Ctx,
    ) -> Result<Message, ResolveError> {
        // CD im Kopf des Clients heißt: der prüft selbst, wir sollen nicht.
        let validate = !crate::dnssec::checking_disabled(request);
        let Some(handle) = self.validating.as_ref().filter(|_| validate) else {
            return self.transport.send(request).await;
        };

        use futures_util::StreamExt as _;
        use hickory_net::DnsHandle as _;
        let mut stream = handle.send(hickory_proto::op::DnsRequest::new(
            request.clone(),
            hickory_proto::op::DnsRequestOptions::default(),
        ));
        // Wie beim direkten Transport: ein Fehler aus dem validierenden Griff
        // kann in Wahrheit ein Urteil sein.
        let mut settled = None;
        let mut response = match stream.next().await {
            Some(Ok(response)) => response.into_message(),
            Some(Err(error)) => match crate::dnssec::from_error(&error) {
                Some((verdict, recovered)) => {
                    settled = Some(verdict);
                    recovered
                }
                None => return Err(ResolveError::Upstream(error.to_string())),
            },
            None => return Err(ResolveError::Timeout),
        };
        response.metadata.id = request.metadata.id;
        response.queries = request.queries.clone();

        let verdict = match settled {
            Some(verdict) => crate::dnssec::apply_verdict(&mut response, verdict)?,
            None => crate::dnssec::apply(&mut response)?,
        };
        ctx.record(crate::trace::Step::DnssecChecked { verdict });
        Ok(response)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn transport(proxy: &str) -> OdohTransport {
        OdohTransport::new(
            proxy.to_owned(),
            "odoh.example",
            "/dns-query",
            std::net::SocketAddr::from(([192, 0, 2, 1], 443)),
            well_known_config_url("odoh.example"),
            Duration::from_secs(5),
            privacy::Settings::default(),
        )
        .expect("Client baubar")
    }

    #[test]
    fn the_well_known_url_is_the_one_from_the_rfc() {
        assert_eq!(
            well_known_config_url("odoh.example"),
            "https://odoh.example/.well-known/odohconfigs"
        );
    }

    #[test]
    fn the_proxy_url_carries_host_and_path() {
        let url = transport("https://proxy.example/proxy").proxy_url();
        assert_eq!(
            url,
            "https://proxy.example/proxy?targethost=odoh.example&targetpath=%2Fdns-query"
        );
    }

    #[test]
    fn an_existing_query_string_is_extended_not_replaced() {
        // Manche Proxys tragen selbst schon Parameter in ihrer URL. Ein
        // zweites '?' würde die URL zerlegen.
        let url = transport("https://proxy.example/proxy?v=1").proxy_url();
        assert!(url.contains("?v=1&targethost="), "{url}");
        assert_eq!(url.matches('?').count(), 1, "{url}");
    }

    #[test]
    fn url_encoding_covers_what_a_path_may_contain() {
        assert_eq!(urlencode("/dns-query"), "%2Fdns-query");
        assert_eq!(urlencode("a.b-c_d~e"), "a.b-c_d~e");
        assert_eq!(urlencode("a b&c=d"), "a%20b%26c%3Dd");
        // Kein Zeichen darf unmaskiert durchrutschen, das die Parameter
        // trennen oder die URL umdeuten könnte.
        for (dangerous, escaped) in [
            ("&", "%26"),
            ("=", "%3D"),
            ("?", "%3F"),
            ("#", "%23"),
            ("%", "%25"),
            (" ", "%20"),
        ] {
            assert_eq!(urlencode(dangerous), escaped);
        }
    }

    #[test]
    fn a_config_that_is_not_odoh_is_refused_instead_of_guessed() {
        for garbage in [
            b"".as_slice(),
            b"<!DOCTYPE html>".as_slice(),
            &[0xff; 64],
            &[0x00, 0x02, 0x00, 0x01],
        ] {
            assert!(
                parse_config(garbage).is_err(),
                "Müll wurde als Konfiguration akzeptiert: {garbage:?}"
            );
        }
    }

    #[test]
    fn a_freshly_generated_config_round_trips() {
        // Die Gegenprobe: was ein Ziel veröffentlicht, muss auch gelesen
        // werden. Sonst prüft der Test darüber bloß, dass alles scheitert.
        let mut rng = Random;
        let pair = odoh_rs::ObliviousDoHKeyPair::new(&mut rng);
        let configs: ObliviousDoHConfigs =
            vec![odoh_rs::ObliviousDoHConfig::from(pair.public().clone())].into();
        let bytes = odoh_rs::compose(&configs).expect("kodierbar").freeze();

        let parsed = parse_config(&bytes).expect("lesbar");
        assert_eq!(&parsed, pair.public());
    }

    #[test]
    fn a_truncated_config_is_refused() {
        let mut rng = Random;
        let pair = odoh_rs::ObliviousDoHKeyPair::new(&mut rng);
        let configs: ObliviousDoHConfigs =
            vec![odoh_rs::ObliviousDoHConfig::from(pair.public().clone())].into();
        let bytes = odoh_rs::compose(&configs).expect("kodierbar").freeze();

        for cut in 1..bytes.len() {
            let Some(shortened) = bytes.get(..cut) else {
                continue;
            };
            assert!(
                parse_config(shortened).is_err(),
                "abgeschnitten nach {cut} Byte trotzdem akzeptiert"
            );
        }
    }

    #[test]
    fn random_bytes_are_not_all_the_same() {
        // Die zehn Zeilen Adapter sind der einzige Ort, an dem der Zufall für
        // die HPKE-Kapselung herkommt. Ein Fehler darin wäre still.
        use hpke::rand_core::RngCore as _;
        let mut first = [0_u8; 32];
        let mut second = [0_u8; 32];
        Random.fill_bytes(&mut first);
        Random.fill_bytes(&mut second);
        assert_ne!(first, second);
        assert!(first.iter().any(|&byte| byte != 0));
    }
}
