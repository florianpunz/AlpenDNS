//! Konfiguration: Einlesen und Validieren der TOML-Datei.
//!
//! Der Server startet nicht mit einer Konfiguration, die er nicht vollständig
//! versteht (CLAUDE.md B.1, Regel 5). Deshalb steht auf jeder Struktur
//! `deny_unknown_fields`: ein Tippfehler in einem Schlüssel ist ein Startfehler,
//! kein stilles Ignorieren.
//!
//! Abgebildet ist bewusst nur die Teilmenge, die Phase 1 wirklich umsetzt. Die
//! Namen folgen `config/alpendns.example.toml`, damit spätere Phasen Felder
//! ergänzen statt das Format zu brechen.

use std::net::SocketAddr;
use std::path::Path;
use std::time::Duration;

use hickory_proto::rr::Name;
use serde::Deserialize;

/// Fehler beim Laden oder Validieren der Konfiguration.
#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    // Die Ursache steht nicht im Text: `main` gibt den Fehler mit `{:#}` aus und
    // hängt die Kette selbst an. Stünde sie zusätzlich hier, käme jede
    // Parse-Meldung doppelt — bei den mehrzeiligen Meldungen zu entfernten
    // Schlüsseln fällt das auf.
    #[error("Konfigurationsdatei {path} konnte nicht gelesen werden")]
    Read {
        path: String,
        #[source]
        source: std::io::Error,
    },
    #[error("Konfigurationsdatei {path} ist ungültig")]
    Parse {
        path: String,
        #[source]
        source: toml::de::Error,
    },
    #[error("Konfiguration unvollständig: {0}")]
    Invalid(String),
}

/// Die vollständige Konfiguration des Servers.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    pub server: ServerConfig,
    /// In Phase 1 genau ein Pool mit genau einem Resolver. Die Liste steht
    /// trotzdem schon hier, weil `config/alpendns.example.toml` das Zielformat
    /// so beschreibt — Phase 3 füllt sie, ohne das Format zu ändern.
    #[serde(default)]
    pub upstream_pool: Vec<UpstreamPool>,
    #[serde(default)]
    pub cache: CacheConfig,
    /// Zonen, die an einen Server im eigenen Netz gehen statt ins Internet.
    #[serde(default)]
    pub forward_zone: Vec<ForwardZone>,
    #[serde(default)]
    pub privacy: PrivacyConfig,
    #[serde(default)]
    pub blocking: BlockingConfig,
    #[serde(default)]
    pub blocklist: Vec<ListConfig>,
    #[serde(default)]
    pub allowlist: Vec<ListConfig>,
    #[serde(default)]
    pub client: Vec<ClientEntry>,
    #[serde(default)]
    pub policy: Vec<PolicyEntry>,
    #[serde(default)]
    pub api: ApiConfig,
    #[serde(default)]
    pub metrics: MetricsConfig,
}

/// HTTP-API und Web-UI.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ApiConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default = "default_api_listen")]
    pub listen: SocketAddr,
    /// Datei mit dem Token. Fehlt sie, wird beim Start einer erzeugt — sonst
    /// müsste man vor dem ersten Start von Hand etwas anlegen, um die UI
    /// überhaupt sehen zu können.
    #[serde(default = "default_token_file")]
    pub token_file: std::path::PathBuf,
}

/// Prometheus-Endpunkt.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MetricsConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default = "default_metrics_listen")]
    pub listen: SocketAddr,
    #[serde(default = "default_metrics_path")]
    pub path: String,
}

fn default_api_listen() -> SocketAddr {
    SocketAddr::from(([127, 0, 0, 1], 8053))
}

fn default_token_file() -> std::path::PathBuf {
    std::path::PathBuf::from("/etc/alpendns/api.token")
}

fn default_metrics_listen() -> SocketAddr {
    SocketAddr::from(([127, 0, 0, 1], 9153))
}

fn default_metrics_path() -> String {
    "/metrics".to_owned()
}

impl Default for ApiConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            listen: default_api_listen(),
            token_file: default_token_file(),
        }
    }
}

impl Default for MetricsConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            listen: default_metrics_listen(),
            path: default_metrics_path(),
        }
    }
}

/// Ein Gerät oder eine Gruppe von Geräten.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ClientEntry {
    pub name: String,
    #[serde(rename = "match")]
    pub matches: ClientMatch,
    pub policy: String,
}

/// Woran ein Client erkannt wird.
///
/// In Phase 5 nur die Adresse. `doh_token` und mTLS stehen in
/// ARCHITECTURE.md §6 und brauchen verschlüsselte Listener, die es noch nicht
/// gibt — sie hier schon entgegenzunehmen würde eine Wirkung versprechen, die
/// nicht eintritt.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ClientMatch {
    /// Einzeladressen oder Netze in CIDR-Schreibweise.
    #[serde(default)]
    pub ip: Vec<String>,
}

/// Was für einen Client gilt.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PolicyEntry {
    pub name: String,
    #[serde(default)]
    pub blocklists: Vec<String>,
    #[serde(default)]
    pub allowlists: Vec<String>,
    /// Zusätzliche Muster, die als Block gelten.
    #[serde(default)]
    pub regex: Vec<String>,
    #[serde(default)]
    pub schedule: Vec<ScheduleEntry>,
}

/// Ein Zeitfenster innerhalb einer Policy.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ScheduleEntry {
    pub name: String,
    /// `mon` bis `sun`.
    pub days: Vec<String>,
    /// `HH:MM` in Ortszeit.
    pub from: String,
    pub to: String,
    pub action: ScheduleAction,
}

/// Was ein Zeitfenster anordnet.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ScheduleAction {
    BlockAllExceptAllowlist,
}

/// Wie geblockt wird.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BlockingConfig {
    #[serde(default)]
    pub mode: crate::filter::block::BlockMode,
    #[serde(default = "default_sinkhole_v4")]
    pub sinkhole_ipv4: std::net::Ipv4Addr,
    #[serde(default = "default_sinkhole_v6")]
    pub sinkhole_ipv6: std::net::Ipv6Addr,
    /// Wohin heruntergeladene Listen geschrieben werden, damit ein Neustart
    /// ohne Internet gefiltert startet (ARCHITECTURE.md §9).
    #[serde(default = "default_list_cache_dir")]
    pub cache_dir: std::path::PathBuf,
}

/// Eine Block- oder Allowlist.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ListConfig {
    pub name: String,
    /// Entweder `url` oder `path`, nicht beides.
    #[serde(default)]
    pub url: Option<String>,
    #[serde(default)]
    pub path: Option<std::path::PathBuf>,
    pub format: crate::filter::parser::Format,
    #[serde(with = "humantime_serde", default = "default_refresh")]
    pub refresh: Duration,
    #[serde(default = "default_true")]
    pub enabled: bool,
}

impl ListConfig {
    fn validate(&self, kind: &str) -> Result<(), ConfigError> {
        let name = &self.name;
        match (&self.url, &self.path) {
            (Some(_), Some(_)) => Err(ConfigError::Invalid(format!(
                "{kind} '{name}': url und path zugleich — es kann nur eine Quelle geben"
            ))),
            (None, None) => Err(ConfigError::Invalid(format!(
                "{kind} '{name}': weder url noch path angegeben"
            ))),
            (Some(url), None) if !url.starts_with("https://") && !url.starts_with("http://") => {
                Err(ConfigError::Invalid(format!(
                    "{kind} '{name}': '{url}' ist keine http(s)-URL"
                )))
            }
            _ => Ok(()),
        }
    }
}

impl ScheduleEntry {
    fn validate(&self, policy: &str) -> Result<(), ConfigError> {
        let name = &self.name;
        if self.days.is_empty() {
            return Err(ConfigError::Invalid(format!(
                "Policy '{policy}', Zeitplan '{name}': keine Tage angegeben"
            )));
        }
        for day in &self.days {
            parse_weekday(day).ok_or_else(|| {
                ConfigError::Invalid(format!(
                    "Policy '{policy}', Zeitplan '{name}': '{day}' ist kein Wochentag \
                     (erwartet: mon, tue, wed, thu, fri, sat, sun)"
                ))
            })?;
        }
        for (label, value) in [("from", &self.from), ("to", &self.to)] {
            parse_clock_time(value).ok_or_else(|| {
                ConfigError::Invalid(format!(
                    "Policy '{policy}', Zeitplan '{name}': {label} = '{value}' ist keine \
                     Uhrzeit im Format HH:MM"
                ))
            })?;
        }
        if self.from == self.to {
            return Err(ConfigError::Invalid(format!(
                "Policy '{policy}', Zeitplan '{name}': from und to sind gleich — das Fenster \
                 wäre entweder immer oder nie offen"
            )));
        }
        Ok(())
    }
}

/// Eine Adresse oder ein Netz. Eine nackte Adresse wird zum Netz mit voller Länge.
pub fn parse_net(text: &str) -> Result<ipnet::IpNet, String> {
    if let Ok(net) = text.parse::<ipnet::IpNet>() {
        return Ok(net);
    }
    text.parse::<std::net::IpAddr>()
        .map(ipnet::IpNet::from)
        .map_err(|_| format!("'{text}' ist weder eine Adresse noch ein Netz"))
}

pub fn parse_weekday(text: &str) -> Option<jiff::civil::Weekday> {
    use jiff::civil::Weekday;
    match text.trim().to_ascii_lowercase().as_str() {
        "mon" | "monday" => Some(Weekday::Monday),
        "tue" | "tuesday" => Some(Weekday::Tuesday),
        "wed" | "wednesday" => Some(Weekday::Wednesday),
        "thu" | "thursday" => Some(Weekday::Thursday),
        "fri" | "friday" => Some(Weekday::Friday),
        "sat" | "saturday" => Some(Weekday::Saturday),
        "sun" | "sunday" => Some(Weekday::Sunday),
        _ => None,
    }
}

/// `HH:MM` in Ortszeit.
pub fn parse_clock_time(text: &str) -> Option<jiff::civil::Time> {
    let (hour, minute) = text.trim().split_once(':')?;
    jiff::civil::Time::new(hour.parse().ok()?, minute.parse().ok()?, 0, 0).ok()
}

/// Einmal am Tag. Oft genug, dass kein Anbieter über Wochen dasselbe Bild
/// sieht; selten genug, dass die Zuordnung innerhalb eines Surftags stabil
/// bleibt und nicht mitten in einer Sitzung ein zweiter Anbieter dieselbe
/// Domain zu sehen bekommt.
const fn default_seed_rotation() -> Duration {
    Duration::from_secs(24 * 60 * 60)
}

fn default_sinkhole_v4() -> std::net::Ipv4Addr {
    std::net::Ipv4Addr::LOCALHOST
}

fn default_sinkhole_v6() -> std::net::Ipv6Addr {
    std::net::Ipv6Addr::LOCALHOST
}

fn default_list_cache_dir() -> std::path::PathBuf {
    std::path::PathBuf::from("/var/cache/alpendns/lists")
}

const fn default_refresh() -> Duration {
    Duration::from_secs(24 * 60 * 60)
}

impl Default for BlockingConfig {
    fn default() -> Self {
        Self {
            mode: crate::filter::block::BlockMode::default(),
            sinkhole_ipv4: default_sinkhole_v4(),
            sinkhole_ipv6: default_sinkhole_v6(),
            cache_dir: default_list_cache_dir(),
        }
    }
}

/// Antwort-Cache.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CacheConfig {
    /// Obergrenze für die Zahl gehaltener Antworten. Darüber wird die am
    /// längsten nicht benutzte verdrängt.
    #[serde(default = "default_max_entries")]
    pub max_entries: usize,
    /// Untergrenze für die TTL. Schützt vor Zonen, die mit TTL 0 arbeiten und
    /// den Cache damit wirkungslos machen würden.
    #[serde(with = "humantime_serde", default = "default_min_ttl")]
    pub min_ttl: Duration,
    #[serde(with = "humantime_serde", default = "default_max_ttl")]
    pub max_ttl: Duration,
    /// Eigener Deckel für negative Antworten (RFC 2308).
    #[serde(with = "humantime_serde", default = "default_max_negative_ttl")]
    pub max_negative_ttl: Duration,
    /// Abgelaufene Antworten weiter ausliefern und parallel auffrischen
    /// (RFC 8767). Erhöht die Verfügbarkeit, kann veraltete Adressen liefern.
    #[serde(default = "default_true")]
    pub serve_stale: bool,
    #[serde(with = "humantime_serde", default = "default_serve_stale_max")]
    pub serve_stale_max: Duration,
    /// Oft gefragte Einträge kurz vor Ablauf im Hintergrund erneuern.
    #[serde(default = "default_true")]
    pub prefetch: bool,
    /// Anteil der TTL, ab dem aufgefrischt wird.
    #[serde(default = "default_prefetch_threshold")]
    pub prefetch_threshold: f32,
}

/// Listener und Grenzwerte des Servers.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ServerConfig {
    #[serde(default)]
    pub listen_udp: Vec<SocketAddr>,
    #[serde(default)]
    pub listen_tcp: Vec<SocketAddr>,
    /// Zeitbudget für eine komplette Anfrage inklusive Upstream.
    #[serde(with = "humantime_serde", default = "default_query_timeout")]
    pub query_timeout: Duration,
    #[serde(default)]
    pub edns: EdnsConfig,
}

/// EDNS(0)-Parameter.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EdnsConfig {
    /// Ab dieser Antwortgröße wird über UDP das TC-Flag gesetzt und der Client
    /// auf TCP geschickt. 1232 Byte ist die Empfehlung des DNS Flag Day 2020.
    #[serde(default = "default_udp_payload_size")]
    pub udp_payload_size: u16,
}

/// Eine Menge gleichwertiger Upstream-Resolver plus die Regel, wie ausgewählt wird.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UpstreamPool {
    pub name: String,
    #[serde(default)]
    pub strategy: Strategy,
    /// Abstand, in dem der Seed von `split_by_zone` neu gezogen wird.
    ///
    /// Bis Phase 7 galt der Seed vom Start bis zum Neustart; wer nie neu
    /// startet, gibt jedem Anbieter dauerhaft dasselbe Drittel seiner Domains
    /// zu sehen. `"0s"` stellt das alte Verhalten wieder her.
    #[serde(with = "humantime_serde", default = "default_seed_rotation")]
    pub seed_rotation: Duration,
    /// Entfernt. Der Schlüssel steht nur noch hier, damit
    /// [`UpstreamPool::validate`] sagen kann, was stattdessen gilt — ohne ihn
    /// meldete `deny_unknown_fields` bloß "unknown field" (ADR-0012).
    #[serde(default)]
    fanout: Option<toml::Value>,
    #[serde(default)]
    pub resolver: Vec<ResolverConfig>,
}

/// Wie ein Resolver aus dem Pool ausgewählt wird.
///
/// Es gibt nur noch eine Strategie. `fastest` und `round_robin` sind entfernt,
/// weil beide dazu führen, dass am Ende jeder Upstream alles sieht
/// ([ADR-0011](../../../docs/adr/0011-eine-upstream-strategie.md)).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Strategy {
    /// Der Upstream wird über `hash(seed, registrierbare Domain)` bestimmt.
    /// Derselbe Name geht immer zum selben Resolver — der Cache bleibt
    /// wirksam — aber jeder sieht nur einen Bruchteil der Domains, und
    /// welchen, ist nach jedem Neustart anders. Siehe FEATURES.md P2.
    #[default]
    SplitByZone,
}

impl<'de> Deserialize<'de> for Strategy {
    /// Von Hand statt abgeleitet, damit eine entfernte Strategie in der
    /// Konfiguration sagt, was stattdessen gilt — `unknown variant` allein
    /// erklärt nicht, warum sie weg ist.
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let text = String::deserialize(deserializer)?;
        match text.as_str() {
            "split_by_zone" => Ok(Self::SplitByZone),
            "fastest" | "round_robin" => Err(serde::de::Error::custom(format!(
                "strategy = \"{text}\" gibt es nicht mehr; es gilt split_by_zone. \
                 Bei beiden entfernten Strategien sieht am Ende jeder Upstream alles \
                 (ARCHITECTURE.md §5). Schlüssel entfernen oder auf split_by_zone setzen."
            ))),
            other => Err(serde::de::Error::custom(format!(
                "unbekannte Strategie '{other}' — erlaubt ist split_by_zone"
            ))),
        }
    }
}

/// Ein einzelner Upstream-Resolver.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ResolverConfig {
    pub name: String,
    pub addr: UpstreamAddr,
    /// Name im Zertifikat des Servers. Pflicht für alle verschlüsselten
    /// Transporte: ohne ihn wird nicht geprüft, mit wem man spricht.
    #[serde(default)]
    pub tls_name: Option<String>,
}

/// Eine Zone, die nicht ins Internet geht.
///
/// Der einzige Ort, an dem Klartext-DNS nach außen erlaubt ist (B.1 Regel 7):
/// der Nameserver im eigenen LAN spricht in aller Regel kein DoT.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ForwardZone {
    pub zone: ZoneName,
    pub upstream: UpstreamAddr,
}

/// Ein Zonenname aus der Konfiguration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ZoneName(pub Name);

impl TryFrom<String> for ZoneName {
    type Error = String;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Name::from_str_relaxed(&value)
            .map(|name| {
                // Absolut machen: eine Zone aus der Konfiguration ist immer
                // absolut gemeint, und `zone_of` vergleicht sonst einen
                // relativen mit einem absoluten Namen.
                let mut name = name.to_lowercase();
                name.set_fqdn(true);
                Self(name)
            })
            .map_err(|e| format!("'{value}' ist kein gültiger Zonenname: {e}"))
    }
}

impl<'de> Deserialize<'de> for ZoneName {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let text = String::deserialize(deserializer)?;
        Self::try_from(text).map_err(serde::de::Error::custom)
    }
}

/// Transport und Adresse eines Upstreams, geparst aus `schema://host[:port][/pfad]`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UpstreamAddr {
    /// Klartext über UDP. Nur in `forward_zone` erlaubt.
    Udp(SocketAddr),
    /// DNS over TLS (RFC 7858), Standardport 853.
    Dot(SocketAddr),
    /// DNS over HTTPS (RFC 8484), Standardport 443.
    Doh { addr: SocketAddr, path: String },
    /// DNS over QUIC (RFC 9250), Standardport 853.
    Doq(SocketAddr),
}

impl UpstreamAddr {
    /// Ob dieser Transport die Anfrage verschlüsselt überträgt.
    pub const fn is_encrypted(&self) -> bool {
        !matches!(self, Self::Udp(_))
    }

    pub const fn socket_addr(&self) -> SocketAddr {
        match self {
            Self::Udp(addr) | Self::Dot(addr) | Self::Doq(addr) => *addr,
            Self::Doh { addr, .. } => *addr,
        }
    }

    /// Kurzform für Logs und Fehlermeldungen. Enthält keine Query-Namen.
    pub const fn scheme(&self) -> &'static str {
        match self {
            Self::Udp(_) => "udp",
            Self::Dot(_) => "dot",
            Self::Doh { .. } => "doh",
            Self::Doq(_) => "doq",
        }
    }
}

/// Hängt einen Standardport an, wenn keiner angegeben ist.
fn parse_host_port(host: &str, default_port: u16) -> Result<SocketAddr, String> {
    if let Ok(addr) = host.parse::<SocketAddr>() {
        return Ok(addr);
    }
    let ip: std::net::IpAddr = host.parse().map_err(|_| {
        format!(
            "'{host}' ist keine IP-Adresse — Namen sind hier nicht erlaubt, \
                              weil ihre Auflösung wieder DNS bräuchte"
        )
    })?;
    Ok(SocketAddr::new(ip, default_port))
}

impl TryFrom<String> for UpstreamAddr {
    type Error = String;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        let Some((scheme, rest)) = value.split_once("://") else {
            return Err(format!(
                "'{value}' hat kein Schema — erwartet wird etwa 'dot://9.9.9.9:853'"
            ));
        };
        match scheme {
            "udp" => parse_host_port(rest, 53).map(Self::Udp),
            "dot" | "tls" => parse_host_port(rest, 853).map(Self::Dot),
            "doq" | "quic" => parse_host_port(rest, 853).map(Self::Doq),
            "doh" | "https" => {
                let (host, path) = match rest.split_once('/') {
                    Some((host, path)) => (host, format!("/{path}")),
                    None => (rest, "/dns-query".to_owned()),
                };
                parse_host_port(host, 443).map(|addr| Self::Doh { addr, path })
            }
            other => Err(format!(
                "unbekannter Transport '{other}' — erlaubt sind udp, dot, doh, doq"
            )),
        }
    }
}

impl<'de> Deserialize<'de> for UpstreamAddr {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let text = String::deserialize(deserializer)?;
        Self::try_from(text).map_err(serde::de::Error::custom)
    }
}

/// Privacy-Mechanismen auf dem Weg zum Upstream.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PrivacyConfig {
    /// EDNS Client Subnet niemals weitergeben. Es verrät dem Upstream das
    /// Subnetz des Clients und ist für einen Heimanschluss nutzlos.
    #[serde(default = "default_true")]
    pub strip_ecs: bool,
    /// EDNS-Padding (RFC 7830/8467). Wirkt nur auf verschlüsselten Transporten,
    /// wo sonst die Nachrichtenlänge den Namen verrät.
    #[serde(default = "default_true")]
    pub padding: bool,
    /// DNS Cookies (RFC 7873) gegen Off-Path-Spoofing. Wie 0x20 nur auf dem
    /// Klartext-Weg sinnvoll.
    #[serde(default = "default_true")]
    pub cookies: bool,
    #[serde(default)]
    pub logging: LoggingConfig,
    /// Zufällige Groß-/Kleinschreibung im QNAME (0x20). Erschwert Spoofing.
    /// Nicht jeder Upstream verträgt es, deshalb pro Pool abschaltbar und im
    /// Fehlerfall automatisch aus.
    #[serde(default = "default_true")]
    pub dns0x20: bool,
    /// Die Signaturkette selbst nachrechnen, statt dem AD-Bit des Upstreams zu
    /// glauben. Eine Antwort, die sich als signiert ausgibt und deren Kette
    /// nicht schließt, wird verworfen (SERVFAIL) — Standardverhalten nach
    /// RFC 4035. Kostet je neuer Zone ein paar zusätzliche Anfragen an
    /// denselben Upstream.
    #[serde(default = "default_true")]
    pub dnssec: bool,
    /// Oblivious DoH (RFC 9230): die Anfrage wird für den Zielresolver
    /// verschlüsselt und über einen Proxy geschickt.
    #[serde(default)]
    pub odoh: OdohConfig,
}

/// Oblivious DoH.
///
/// Der Schutz steht und fällt damit, dass Proxy und Ziel **nicht demselben
/// Betreiber gehören** — sonst kennt einer beides. Prüfen kann das niemand
/// außer dem Betreiber; die Konfiguration kann nur sicherstellen, dass
/// überhaupt beides da ist.
#[derive(Debug, Clone, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct OdohConfig {
    #[serde(default)]
    pub enabled: bool,
    /// Vollständige URL des Proxys, etwa `https://odoh-proxy.example/proxy`.
    #[serde(default)]
    pub proxy: Option<String>,
}

impl Default for PrivacyConfig {
    fn default() -> Self {
        Self {
            strip_ecs: default_true(),
            padding: default_true(),
            cookies: default_true(),
            logging: LoggingConfig::default(),
            dns0x20: default_true(),
            dnssec: default_true(),
            odoh: OdohConfig::default(),
        }
    }
}

/// Was eine Anfrage hinterlässt. Begründung der Modi: ADR-0004.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LoggingConfig {
    #[serde(default)]
    pub mode: crate::logging::Mode,
    /// Zeitfenster des Ringpuffers im Modus `ring`.
    #[serde(with = "humantime_serde", default = "default_ring_seconds")]
    pub ring_seconds: Duration,
    /// Unter diesem Zählerstand taucht eine Domain in keiner Statistik auf.
    #[serde(default = "default_aggregate_k")]
    pub aggregate_k: u32,
    /// Nur im Modus `full`.
    #[serde(default = "default_query_log_path")]
    pub path: std::path::PathBuf,
}

const fn default_ring_seconds() -> Duration {
    Duration::from_secs(300)
}

const fn default_aggregate_k() -> u32 {
    5
}

fn default_query_log_path() -> std::path::PathBuf {
    std::path::PathBuf::from("/var/log/alpendns/queries.jsonl")
}

impl Default for LoggingConfig {
    fn default() -> Self {
        Self {
            mode: crate::logging::Mode::default(),
            ring_seconds: default_ring_seconds(),
            aggregate_k: default_aggregate_k(),
            path: default_query_log_path(),
        }
    }
}

const fn default_query_timeout() -> Duration {
    Duration::from_secs(3)
}

const fn default_max_entries() -> usize {
    100_000
}

const fn default_min_ttl() -> Duration {
    Duration::from_secs(10)
}

const fn default_max_ttl() -> Duration {
    Duration::from_secs(24 * 60 * 60)
}

const fn default_max_negative_ttl() -> Duration {
    Duration::from_secs(15 * 60)
}

const fn default_serve_stale_max() -> Duration {
    Duration::from_secs(60 * 60)
}

const fn default_true() -> bool {
    true
}

const fn default_prefetch_threshold() -> f32 {
    0.85
}

impl Default for CacheConfig {
    fn default() -> Self {
        Self {
            max_entries: default_max_entries(),
            min_ttl: default_min_ttl(),
            max_ttl: default_max_ttl(),
            max_negative_ttl: default_max_negative_ttl(),
            serve_stale: default_true(),
            serve_stale_max: default_serve_stale_max(),
            prefetch: default_true(),
            prefetch_threshold: default_prefetch_threshold(),
        }
    }
}

const fn default_udp_payload_size() -> u16 {
    1232
}

impl Default for EdnsConfig {
    fn default() -> Self {
        Self {
            udp_payload_size: default_udp_payload_size(),
        }
    }
}

impl Config {
    /// Liest und validiert eine Konfigurationsdatei.
    pub fn load(path: &Path) -> Result<Self, ConfigError> {
        let text = std::fs::read_to_string(path).map_err(|source| ConfigError::Read {
            path: path.display().to_string(),
            source,
        })?;
        let config: Self = toml::from_str(&text).map_err(|source| ConfigError::Parse {
            path: path.display().to_string(),
            source,
        })?;
        config.validate()?;
        Ok(config)
    }

    /// Prüft, was `serde` allein nicht prüfen kann.
    fn validate(&self) -> Result<(), ConfigError> {
        if self.server.listen_udp.is_empty() && self.server.listen_tcp.is_empty() {
            return Err(ConfigError::Invalid(
                "kein Listener konfiguriert: server.listen_udp und server.listen_tcp sind beide leer"
                    .to_owned(),
            ));
        }
        if self.cache.max_entries == 0 {
            return Err(ConfigError::Invalid(
                "cache.max_entries = 0 schaltet den Cache nicht ab, sondern ergibt einen \
                 Cache ohne Platz; setze einen sinnvollen Wert"
                    .to_owned(),
            ));
        }
        if self.cache.min_ttl > self.cache.max_ttl {
            return Err(ConfigError::Invalid(
                "cache.min_ttl ist größer als cache.max_ttl".to_owned(),
            ));
        }
        self.privacy.odoh.validate()?;
        if self.privacy.odoh.enabled {
            // Ein Pool mit einem DoT-Resolver und eingeschaltetem ODoH sähe aus
            // wie Schutz und wäre keiner: ODoH gibt es nur über HTTP. Lieber
            // ein Startfehler als ein Versprechen, das die Hälfte der Anfragen
            // nicht einlöst (B.1 Regel 5).
            for pool in &self.upstream_pool {
                for resolver in &pool.resolver {
                    if !matches!(resolver.addr, UpstreamAddr::Doh { .. }) {
                        return Err(ConfigError::Invalid(format!(
                            "privacy.odoh ist eingeschaltet, aber Resolver '{}' in Pool '{}'                              spricht {}. Oblivious DoH gibt es nur über doh://; entweder alle                              Resolver auf doh:// umstellen oder odoh abschalten.",
                            resolver.name,
                            pool.name,
                            resolver.addr.scheme()
                        )));
                    }
                }
            }
        }
        if !(0.0..=1.0).contains(&self.cache.prefetch_threshold) {
            return Err(ConfigError::Invalid(format!(
                "cache.prefetch_threshold muss zwischen 0.0 und 1.0 liegen, ist aber {}",
                self.cache.prefetch_threshold
            )));
        }
        if self
            .upstream_pool
            .iter()
            .all(|pool| pool.resolver.is_empty())
        {
            return Err(ConfigError::Invalid(
                "kein Upstream konfiguriert: es braucht mindestens einen \
                 [[upstream_pool.resolver]]"
                    .to_owned(),
            ));
        }
        if self.upstream_pool.len() > 1 {
            return Err(ConfigError::Invalid(format!(
                "{} Upstream-Pools konfiguriert. Welcher Pool für welchen Client gilt, \
                 entscheiden Policies — die kommen in Phase 5. Bis dahin ist genau einer erlaubt.",
                self.upstream_pool.len()
            )));
        }
        for pool in &self.upstream_pool {
            pool.validate()?;
        }
        for list in &self.blocklist {
            list.validate("blocklist")?;
        }
        for list in &self.allowlist {
            list.validate("allowlist")?;
        }
        let mut list_names: std::collections::HashSet<&str> = std::collections::HashSet::new();
        for list in self.blocklist.iter().chain(self.allowlist.iter()) {
            if !list_names.insert(list.name.as_str()) {
                return Err(ConfigError::Invalid(format!(
                    "die Liste '{}' ist zweimal konfiguriert; Policies verweisen über den \
                     Namen, er muss eindeutig sein",
                    list.name
                )));
            }
        }
        let policy_names: std::collections::HashSet<&str> =
            self.policy.iter().map(|p| p.name.as_str()).collect();
        for client in &self.client {
            if client.matches.ip.is_empty() {
                return Err(ConfigError::Invalid(format!(
                    "Client '{}': kein match.ip angegeben — er wäre nie erkennbar",
                    client.name
                )));
            }
            for address in &client.matches.ip {
                parse_net(address)
                    .map_err(|e| ConfigError::Invalid(format!("Client '{}': {e}", client.name)))?;
            }
            if !policy_names.contains(client.policy.as_str()) {
                return Err(ConfigError::Invalid(format!(
                    "Client '{}' verweist auf die Policy '{}', die es nicht gibt",
                    client.name, client.policy
                )));
            }
        }
        if !self.client.is_empty() && !policy_names.contains("default") {
            return Err(ConfigError::Invalid(
                "es gibt Clients, aber keine Policy namens 'default' — für alles, was \
                 keinem Client-Eintrag entspricht, gäbe es dann keine Regel"
                    .to_owned(),
            ));
        }
        for policy in &self.policy {
            for list in policy.blocklists.iter().chain(policy.allowlists.iter()) {
                if !list_names.contains(list.as_str()) {
                    return Err(ConfigError::Invalid(format!(
                        "Policy '{}' verweist auf die Liste '{list}', die es nicht gibt",
                        policy.name
                    )));
                }
            }
            for entry in &policy.schedule {
                entry.validate(&policy.name)?;
            }
        }
        if self.metrics.enabled && !self.metrics.path.starts_with('/') {
            return Err(ConfigError::Invalid(format!(
                "metrics.path = '{}' muss mit einem Schrägstrich beginnen",
                self.metrics.path
            )));
        }
        if self.api.enabled && self.api.listen == self.metrics.listen {
            return Err(ConfigError::Invalid(
                "api.listen und metrics.listen sind gleich — die Metriken haben \
                 bewusst keinen Token und dürfen nicht auf demselben Port liegen wie \
                 die API, die Namen zeigt"
                    .to_owned(),
            ));
        }
        for zone in &self.forward_zone {
            if zone.upstream.is_encrypted() {
                return Err(ConfigError::Invalid(format!(
                    "forward_zone '{}': verschlüsselte Transporte sind hier noch nicht \
                     umgesetzt. Eine forward_zone zeigt auf einen Nameserver im eigenen Netz; \
                     dafür ist udp:// vorgesehen.",
                    zone.zone.0
                )));
            }
        }
        Ok(())
    }
}

impl OdohConfig {
    fn validate(&self) -> Result<(), ConfigError> {
        if !self.enabled {
            return Ok(());
        }
        let Some(proxy) = self
            .proxy
            .as_deref()
            .map(str::trim)
            .filter(|p| !p.is_empty())
        else {
            return Err(ConfigError::Invalid(
                "privacy.odoh.enabled = true, aber kein proxy angegeben. Ohne Proxy wäre                  ODoH nur eine zweite Verschlüsselung zum selben Ziel und würde nichts                  verbergen."
                    .to_owned(),
            ));
        };
        if !proxy.starts_with("https://") {
            return Err(ConfigError::Invalid(format!(
                "privacy.odoh.proxy = '{proxy}': der Proxy muss über https erreichbar sein.                  Über http sähe ein Mitleser Zieladresse und Zeitpunkt jeder Anfrage."
            )));
        }
        Ok(())
    }
}

impl UpstreamPool {
    fn validate(&self) -> Result<(), ConfigError> {
        let pool = &self.name;
        if self.resolver.is_empty() {
            return Err(ConfigError::Invalid(format!(
                "Pool '{pool}' hat keinen Resolver"
            )));
        }
        if self.fanout.is_some() {
            return Err(ConfigError::Invalid(format!(
                "Pool '{pool}': fanout gibt es nicht mehr. Es wird immer genau ein \
                 Resolver gefragt und erst beim Ausfall der nächste. Parallele \
                 Anfragen zeigten dieselbe Frage mehreren Anbietern und hoben \
                 split_by_zone auf. Bitte den Schlüssel entfernen."
            )));
        }
        for resolver in &self.resolver {
            let name = &resolver.name;
            // B.1 Regel 7: kein Klartext-DNS nach außen. Die einzige Ausnahme
            // sind forward_zone-Einträge ins eigene LAN, und die stehen nicht
            // in einem Pool.
            if !resolver.addr.is_encrypted() {
                return Err(ConfigError::Invalid(format!(
                    "Pool '{pool}', Resolver '{name}': Klartext-DNS ist als Upstream nicht \
                     erlaubt. Benutze dot://, doh:// oder doq://. Für einen Nameserver im \
                     eigenen Netz ist [[forward_zone]] der richtige Ort."
                )));
            }
            if resolver
                .tls_name
                .as_ref()
                .is_none_or(|n| n.trim().is_empty())
            {
                return Err(ConfigError::Invalid(format!(
                    "Pool '{pool}', Resolver '{name}': tls_name fehlt. Ohne ihn wird das \
                     Zertifikat gegen nichts geprüft und die Verschlüsselung ist wertlos."
                )));
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MINIMAL: &str = r#"
[server]
listen_udp = ["127.0.0.1:5353"]
listen_tcp = ["127.0.0.1:5353"]

[[upstream_pool]]
name = "default"

[[upstream_pool.resolver]]
name = "quad9"
addr = "dot://9.9.9.9:853"
tls_name = "dns.quad9.net"
"#;

    fn parse(text: &str) -> Result<Config, toml::de::Error> {
        toml::from_str(text)
    }

    fn valid(text: &str) -> Config {
        let config = parse(text).expect("muss parsen");
        config.validate().expect("muss gültig sein");
        config
    }

    #[test]
    fn the_new_privacy_defaults_are_on() {
        // Phase 7: beide sind bewusst Default-an bzw. Default-aus. Eine
        // Änderung daran soll hier auffallen und nicht im Betrieb.
        let config = valid(MINIMAL);
        assert!(config.privacy.dnssec, "DNSSEC ist nicht Default");
        assert!(!config.privacy.odoh.enabled, "ODoH ist Default an");
        assert_eq!(
            config
                .upstream_pool
                .first()
                .expect("ein Pool")
                .seed_rotation,
            Duration::from_secs(24 * 60 * 60)
        );
    }

    #[test]
    fn the_seed_rotation_can_be_switched_off() {
        // "0s" heißt: der Seed gilt bis zum Neustart, wie in Phase 3.
        let text = r#"
[server]
listen_udp = ["127.0.0.1:5353"]

[[upstream_pool]]
name = "default"
seed_rotation = "0s"

[[upstream_pool.resolver]]
name = "quad9"
addr = "dot://9.9.9.9:853"
tls_name = "dns.quad9.net"
"#;
        assert_eq!(
            valid(text)
                .upstream_pool
                .first()
                .expect("ein Pool")
                .seed_rotation,
            Duration::ZERO
        );
    }

    #[test]
    fn odoh_without_a_proxy_is_a_startup_error() {
        let text = format!("{MINIMAL}\n[privacy]\nodoh = {{ enabled = true }}\n");
        let config = parse(&text).expect("parst");
        let error = config.validate().expect_err("muss abbrechen");
        assert!(error.to_string().contains("proxy"), "{error}");
    }

    #[test]
    fn an_odoh_proxy_without_tls_is_a_startup_error() {
        let text = format!(
            "{MINIMAL}\n[privacy]\nodoh = {{ enabled = true, proxy = \"http://proxy.example/p\" }}\n"
        );
        let config = parse(&text).expect("parst");
        assert!(config.validate().is_err(), "http-Proxy wurde akzeptiert");
    }

    #[test]
    fn odoh_with_a_non_doh_resolver_is_a_startup_error() {
        // Der Pool aus MINIMAL spricht DoT. Mit ODoH wäre das ein Versprechen,
        // das für jede Anfrage nicht eingelöst würde.
        let text = format!(
            "{MINIMAL}\n[privacy]\nodoh = {{ enabled = true, proxy = \"https://proxy.example/p\" }}\n"
        );
        let config = parse(&text).expect("parst");
        let error = config.validate().expect_err("muss abbrechen");
        assert!(error.to_string().contains("doh://"), "{error}");
    }

    #[test]
    fn odoh_with_doh_resolvers_is_accepted() {
        let text = r#"
[server]
listen_udp = ["127.0.0.1:5353"]

[[upstream_pool]]
name = "default"

[[upstream_pool.resolver]]
name = "mullvad"
addr = "doh://194.242.2.4/dns-query"
tls_name = "dns.mullvad.net"

[privacy]
odoh = { enabled = true, proxy = "https://proxy.example/p" }
"#;
        let config = valid(text);
        assert!(config.privacy.odoh.enabled);
    }

    #[test]
    fn minimal_config_parses_with_defaults() {
        let config = valid(MINIMAL);
        assert_eq!(config.server.query_timeout, Duration::from_secs(3));
        assert_eq!(config.server.edns.udp_payload_size, 1232);
        assert_eq!(config.cache.max_entries, 100_000);
        let pool = config.upstream_pool.first().expect("ein Pool");
        assert_eq!(pool.strategy, Strategy::SplitByZone, "Default-Strategie");
        assert!(config.privacy.strip_ecs);
        assert!(config.privacy.dns0x20);
    }

    #[test]
    fn typo_in_key_is_an_error_naming_the_key() {
        // Der Fall aus der Roadmap: ein Tippfehler darf nicht dazu führen, dass
        // der Server mit halber Konfiguration startet.
        let text = MINIMAL.replace("listen_udp", "listen_udb");
        let err = parse(&text).expect_err("unbekannter Schlüssel muss ein Fehler sein");
        assert!(
            err.to_string().contains("listen_udb"),
            "Fehlermeldung nennt den Schlüssel nicht: {err}"
        );
    }

    #[test]
    fn unknown_key_in_nested_table_is_an_error() {
        let text = format!("{MINIMAL}\n[server.edns]\nudp_payload_size = 512\npaddng = true\n");
        let err = parse(&text).expect_err("unbekannter Schlüssel muss ein Fehler sein");
        assert!(err.to_string().contains("paddng"), "{err}");
    }

    #[test]
    fn duration_strings_are_parsed() {
        let text = MINIMAL.replace("[server]", "[server]\nquery_timeout = \"250ms\"");
        let config = parse(&text).expect("Dauer als String muss parsen");
        assert_eq!(config.server.query_timeout, Duration::from_millis(250));
    }

    #[test]
    fn plaintext_upstream_in_a_pool_is_rejected() {
        // B.1 Regel 7. Der Hinweis muss den richtigen Ort nennen.
        let text = MINIMAL.replace("dot://9.9.9.9:853", "udp://9.9.9.9:53");
        let err = valid_err(&text);
        assert!(err.contains("Klartext"), "{err}");
        assert!(
            err.contains("forward_zone"),
            "kein Hinweis auf die Ausnahme: {err}"
        );
    }

    #[test]
    fn encrypted_upstream_without_tls_name_is_rejected() {
        let text = MINIMAL.replace("tls_name = \"dns.quad9.net\"\n", "");
        let err = valid_err(&text);
        assert!(err.contains("tls_name"), "{err}");
    }

    fn valid_err(text: &str) -> String {
        parse(text)
            .expect("parst syntaktisch")
            .validate()
            .expect_err("muss abgelehnt werden")
            .to_string()
    }

    #[test]
    fn plaintext_is_allowed_in_a_forward_zone() {
        let text = format!(
            "{MINIMAL}\n[[forward_zone]]\nzone = \"home.arpa\"\nupstream = \"udp://10.0.0.1:53\"\n"
        );
        let config = valid(&text);
        let zone = config.forward_zone.first().expect("eine Zone");
        assert_eq!(zone.zone.0.to_ascii(), "home.arpa.");
        assert_eq!(
            zone.upstream,
            UpstreamAddr::Udp("10.0.0.1:53".parse().expect("gültig"))
        );
    }

    #[test]
    fn zone_names_are_lowercased() {
        let text = format!(
            "{MINIMAL}\n[[forward_zone]]\nzone = \"HOME.Arpa\"\nupstream = \"udp://10.0.0.1:53\"\n"
        );
        let config = valid(&text);
        assert_eq!(
            config
                .forward_zone
                .first()
                .expect("eine Zone")
                .zone
                .0
                .to_ascii(),
            "home.arpa."
        );
    }

    #[test]
    fn default_ports_are_filled_in_per_transport() {
        assert_eq!(
            UpstreamAddr::try_from("dot://9.9.9.9".to_owned()),
            Ok(UpstreamAddr::Dot("9.9.9.9:853".parse().expect("gültig")))
        );
        assert_eq!(
            UpstreamAddr::try_from("doq://9.9.9.9".to_owned()),
            Ok(UpstreamAddr::Doq("9.9.9.9:853".parse().expect("gültig")))
        );
        assert_eq!(
            UpstreamAddr::try_from("udp://10.0.0.1".to_owned()),
            Ok(UpstreamAddr::Udp("10.0.0.1:53".parse().expect("gültig")))
        );
    }

    #[test]
    fn doh_url_splits_into_address_and_path() {
        assert_eq!(
            UpstreamAddr::try_from("doh://194.242.2.4/dns-query".to_owned()),
            Ok(UpstreamAddr::Doh {
                addr: "194.242.2.4:443".parse().expect("gültig"),
                path: "/dns-query".to_owned(),
            })
        );
        assert_eq!(
            UpstreamAddr::try_from("doh://194.242.2.4:8443".to_owned()),
            Ok(UpstreamAddr::Doh {
                addr: "194.242.2.4:8443".parse().expect("gültig"),
                path: "/dns-query".to_owned(),
            }),
            "ohne Pfad wird /dns-query angenommen"
        );
    }

    #[test]
    fn hostnames_as_upstream_are_rejected() {
        // Einen Namen aufzulösen, um den Resolver zu erreichen, ist ein
        // Henne-Ei-Problem.
        let err = UpstreamAddr::try_from("dot://dns.quad9.net:853".to_owned())
            .expect_err("Name statt IP");
        assert!(err.contains("IP-Adresse"), "{err}");
    }

    #[test]
    fn address_without_scheme_is_rejected() {
        let err = UpstreamAddr::try_from("9.9.9.9:853".to_owned()).expect_err("kein Schema");
        assert!(err.contains("Schema"), "{err}");
    }

    #[test]
    fn unknown_transport_is_rejected() {
        let err = UpstreamAddr::try_from("gopher://9.9.9.9".to_owned()).expect_err("unbekannt");
        assert!(err.contains("gopher"), "{err}");
    }

    #[test]
    fn strategy_is_parsed_from_snake_case() {
        let config = valid(&MINIMAL.replace(
            "name = \"default\"",
            "name = \"default\"\nstrategy = \"split_by_zone\"",
        ));
        assert_eq!(
            config.upstream_pool.first().expect("Pool").strategy,
            Strategy::SplitByZone
        );
    }

    #[test]
    fn a_removed_strategy_says_what_applies_instead() {
        // Eine Konfiguration von gestern soll nicht mit "unknown variant"
        // abbrechen, sondern sagen, was jetzt gilt.
        for removed in ["fastest", "round_robin"] {
            let text = MINIMAL.replace(
                "name = \"default\"",
                &format!("name = \"default\"\nstrategy = \"{removed}\""),
            );
            let err = parse(&text)
                .expect_err("{removed} muss abgelehnt werden")
                .to_string();
            assert!(err.contains(removed), "{err}");
            assert!(err.contains("split_by_zone"), "{err}");
        }
    }

    #[test]
    fn the_shipped_minimal_config_still_parses() {
        // Ohne diesen Test treibt die ausgelieferte Konfiguration von der
        // Implementierung weg, und es merkt erst der, der sie benutzt.
        let config = valid(include_str!("../../../config/alpendns.minimal.toml"));
        assert_eq!(
            config.upstream_pool.first().expect("Pool").strategy,
            Strategy::SplitByZone
        );
        assert_eq!(
            config.blocking.mode,
            crate::filter::block::BlockMode::Nxdomain
        );
        assert_eq!(
            config.blocklist.first().expect("Blockliste").format,
            crate::filter::parser::Format::Hosts
        );
    }

    #[test]
    fn the_removed_list_format_says_what_applies_instead() {
        let text = format!(
            "{MINIMAL}\n[[blocklist]]\nname = \"x\"\nurl = \"https://liste.example/l\"\n\
             format = \"adblock\"\n"
        );
        let err = parse(&text).expect_err("adblock ist weg").to_string();
        assert!(err.contains("adblock"), "{err}");
        assert!(err.contains("wildcard"), "{err}");
    }

    #[test]
    fn the_removed_block_mode_says_what_applies_instead() {
        // REFUSED schickte den Client zum nächsten Resolver seiner Liste. Wer
        // es konfiguriert hatte, soll das lesen, statt "unknown variant".
        let text = format!("{MINIMAL}\n[blocking]\nmode = \"refused\"\n");
        let err = parse(&text).expect_err("refused ist weg").to_string();
        assert!(err.contains("refused"), "{err}");
        assert!(err.contains("nxdomain"), "{err}");
    }

    #[test]
    fn the_remaining_block_modes_still_parse() {
        for (text, expected) in [
            ("nxdomain", crate::filter::block::BlockMode::Nxdomain),
            ("zero_ip", crate::filter::block::BlockMode::ZeroIp),
            ("sinkhole", crate::filter::block::BlockMode::Sinkhole),
        ] {
            let config = valid(&format!("{MINIMAL}\n[blocking]\nmode = \"{text}\"\n"));
            assert_eq!(config.blocking.mode, expected);
        }
    }

    #[test]
    fn a_removed_fanout_says_what_applies_instead() {
        // Nicht nur "unbekanntes Feld": wer fanout gesetzt hatte, soll lesen,
        // was der Server jetzt tut.
        let text = MINIMAL.replace("name = \"default\"", "name = \"default\"\nfanout = 2");
        let err = valid_err(&text);
        assert!(err.contains("fanout"), "{err}");
        assert!(err.contains("genau ein"), "{err}");
    }

    #[test]
    fn config_without_listener_is_rejected() {
        let text = MINIMAL
            .replace("listen_udp = [\"127.0.0.1:5353\"]", "listen_udp = []")
            .replace("listen_tcp = [\"127.0.0.1:5353\"]", "listen_tcp = []");
        let err = valid_err(&text);
        assert!(err.contains("Listener"), "{err}");
    }

    #[test]
    fn config_without_upstream_is_rejected() {
        let text = MINIMAL
            .split("[[upstream_pool.resolver]]")
            .next()
            .expect("Kopf")
            .to_owned();
        let err = valid_err(&text);
        assert!(err.contains("Upstream"), "{err}");
    }
}
