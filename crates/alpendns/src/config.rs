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

use crate::local::{LocalZone, LocalZoneConfig, build_zones};

/// Fehler beim Laden oder Validieren der Konfiguration.
#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    // Die Ursache steht nicht im Text: `main` gibt den Fehler mit `{:#}` aus und
    // hängt die Kette selbst an. Stünde sie zusätzlich hier, käme jede
    // Parse-Meldung doppelt — bei den mehrzeiligen Meldungen zu entfernten
    // Schlüsseln fällt das auf.
    #[error("configuration file {path} could not be read")]
    Read {
        path: String,
        #[source]
        source: std::io::Error,
    },
    #[error("configuration file {path} is invalid")]
    Parse {
        path: String,
        #[source]
        source: toml::de::Error,
    },
    #[error("configuration incomplete: {0}")]
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
    /// Zonen, die AlpenDNS selbst beantwortet. Siehe [`crate::local`].
    #[serde(default)]
    pub local_zone: Vec<LocalZoneConfig>,
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
    pub detection: DetectionConfig,
    #[serde(default)]
    pub api: ApiConfig,
    #[serde(default)]
    pub metrics: MetricsConfig,
}

/// Die Heuristiken aus Phase 8.
///
/// **Alle fünf stehen per Default auf `flag`** — sie melden, sie blocken nicht.
/// Das ist keine Vorsicht um der Vorsicht willen, sondern das Abnahmekriterium
/// der Phase: erst eine Woche Betrieb, dann die Fehlalarm-Liste durchsehen, und
/// erst danach darf ein Detektor auf `block` (CLAUDE.md B.8). Ein Detektor, der
/// beim ersten Start Internet kaputtmacht, wird abgeschaltet — und mit ihm alle
/// anderen.
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DetectionConfig {
    #[serde(default)]
    pub dga: DgaConfig,
    #[serde(default)]
    pub tunneling: TunnelingConfig,
    #[serde(default)]
    pub rebinding: RebindingConfig,
    #[serde(default)]
    pub typosquat: TyposquatConfig,
    #[serde(default)]
    pub nrd: NrdConfig,
}

/// Algorithmisch erzeugte Namen.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DgaConfig {
    #[serde(default)]
    pub action: crate::detect::Action,
    /// Ab diesem Score gilt ein Name als erzeugt. Der Default kommt aus dem
    /// Detektor selbst, weil er nur zusammen mit dem Modell einen Sinn ergibt.
    #[serde(default = "default_dga_threshold")]
    pub threshold: f32,
}

/// Datenexfiltration über DNS.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TunnelingConfig {
    #[serde(default)]
    pub action: crate::detect::Action,
    #[serde(default = "default_tunneling_threshold")]
    pub threshold: f32,
    /// Zeitfenster, über das je Zone gezählt wird.
    #[serde(with = "humantime_serde", default = "default_tunneling_window")]
    pub window: Duration,
    /// Zonen, die nicht bewertet werden — für Reputationsdienste und
    /// Antivirus-Produkte, die per Konstruktion wie ein Tunnel aussehen.
    #[serde(default)]
    pub allow_zones: Vec<String>,
}

/// Private Adressen als Antwort auf öffentliche Namen.
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RebindingConfig {
    #[serde(default)]
    pub action: crate::detect::Action,
    /// Zonen, in denen private Adressen erlaubt sind. Die `forward_zone`-Einträge
    /// kommen automatisch dazu (siehe [`Config::rebinding_allow_zones`]).
    #[serde(default)]
    pub allow_zones: Vec<ZoneName>,
}

/// Verwechselbare Namen relativ zu einer kleinen Schutzliste.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TyposquatConfig {
    #[serde(default)]
    pub action: crate::detect::Action,
    #[serde(default = "default_typosquat_threshold")]
    pub threshold: f32,
    /// Die Domains, die dir wichtig sind. Ohne Einträge tut der Detektor
    /// nichts — er hat dann nichts, wogegen er vergleichen könnte.
    #[serde(default)]
    pub protect: Vec<String>,
}

/// Neu registrierte Domains aus einer lokalen Datei.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NrdConfig {
    #[serde(default)]
    pub action: crate::detect::Action,
    /// Bis zu diesem Alter gilt eine Domain als neu.
    #[serde(with = "humantime_serde", default = "default_nrd_max_age")]
    pub max_age: Duration,
    /// Die Datei mit Domain und Registrierungsdatum. Fehlt sie, läuft der
    /// Detektor leer mit — der Resolver hängt nicht davon ab, dass ein
    /// zweiter Dienst gelaufen ist.
    #[serde(default)]
    pub source: Option<std::path::PathBuf>,
}

impl DetectionConfig {
    /// Eine Zeile für `alpendns check`: die Stufe jedes Detektors.
    ///
    /// Hinter einem Detektor, der eingeschaltet ist und trotzdem nichts finden
    /// *kann*, steht der Grund in Klammern — Typosquat ohne `protect`, NRD ohne
    /// lesbare Quelldatei. Das ist der eigentliche Zweck der Zeile: beide sehen
    /// am Ende einer Beobachtungswoche aus wie ein fehlerfreier Lauf, nur ohne
    /// Grundlage (OPERATIONS.md §6). Ein Detektor auf `off` bekommt keinen
    /// Hinweis; bei ihm ist das Nichtstun die Absicht.
    ///
    /// Die NRD-Datei wird dafür nicht gelesen, nur ihre Größe angesehen: ein
    /// `check` auf einem Feed mit einer Million Zeilen soll nicht dauern.
    pub fn check_line(&self) -> String {
        [
            ("dga", self.dga.action, None),
            ("tunneling", self.tunneling.action, None),
            ("rebinding", self.rebinding.action, None),
            ("typosquat", self.typosquat.action, self.protect_note()),
            ("nrd", self.nrd.action, self.nrd_note()),
        ]
        .into_iter()
        .map(|(name, action, note)| match note {
            Some(note) => format!("{name} {} ({note})", action.as_str()),
            None => format!("{name} {}", action.as_str()),
        })
        .collect::<Vec<_>>()
        .join(", ")
    }

    /// `None`, wenn der Wächter etwas zu schützen hat.
    fn protect_note(&self) -> Option<String> {
        if self.typosquat.action.is_off() || !self.typosquat.protect.is_empty() {
            return None;
        }
        Some("no protect: finds nothing".to_owned())
    }

    /// `None`, wenn die Quelldatei da ist.
    fn nrd_note(&self) -> Option<String> {
        if self.nrd.action.is_off() {
            return None;
        }
        let Some(source) = &self.nrd.source else {
            return Some("no source: runs empty".to_owned());
        };
        match std::fs::metadata(source) {
            Ok(meta) if meta.len() > 0 => None,
            Ok(_) => Some(format!("{} is empty: runs empty", source.display())),
            Err(_) => Some(format!("{} is missing: runs empty", source.display())),
        }
    }
}

const fn default_dga_threshold() -> f32 {
    crate::detect::dga::DEFAULT_THRESHOLD
}

const fn default_tunneling_threshold() -> f32 {
    crate::detect::tunneling::DEFAULT_THRESHOLD
}

/// Verwechslungen sind selten und der Vergleich ist scharf; die Schwelle darf
/// deshalb hoch liegen, ohne etwas zu verpassen.
const fn default_typosquat_threshold() -> f32 {
    0.85
}

/// Fünf Minuten. Lang genug, dass ein Tunnel auffällt, kurz genug, dass der
/// Zustand nicht mit dem Tag wächst.
const fn default_tunneling_window() -> Duration {
    Duration::from_secs(300)
}

const fn default_nrd_max_age() -> Duration {
    Duration::from_secs(30 * 24 * 60 * 60)
}

impl Default for DgaConfig {
    fn default() -> Self {
        Self {
            action: crate::detect::Action::default(),
            threshold: default_dga_threshold(),
        }
    }
}

impl Default for TunnelingConfig {
    fn default() -> Self {
        Self {
            action: crate::detect::Action::default(),
            threshold: default_tunneling_threshold(),
            window: default_tunneling_window(),
            allow_zones: Vec::new(),
        }
    }
}

impl Default for TyposquatConfig {
    fn default() -> Self {
        Self {
            action: crate::detect::Action::default(),
            threshold: default_typosquat_threshold(),
            protect: Vec::new(),
        }
    }
}

impl Default for NrdConfig {
    fn default() -> Self {
        Self {
            action: crate::detect::Action::default(),
            max_age: default_nrd_max_age(),
            source: None,
        }
    }
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
                "{kind} '{name}': url and path at once — there can only be one source"
            ))),
            (None, None) => Err(ConfigError::Invalid(format!(
                "{kind} '{name}': neither url nor path given"
            ))),
            (Some(url), None) if !url.starts_with("https://") && !url.starts_with("http://") => {
                Err(ConfigError::Invalid(format!(
                    "{kind} '{name}': '{url}' is not an http(s) URL"
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
                "Policy '{policy}', schedule '{name}': no days given"
            )));
        }
        for day in &self.days {
            parse_weekday(day).ok_or_else(|| {
                ConfigError::Invalid(format!(
                    "Policy '{policy}', schedule '{name}': '{day}' is not a weekday \
                     (expected: mon, tue, wed, thu, fri, sat, sun)"
                ))
            })?;
        }
        for (label, value) in [("from", &self.from), ("to", &self.to)] {
            parse_clock_time(value).ok_or_else(|| {
                ConfigError::Invalid(format!(
                    "Policy '{policy}', schedule '{name}': {label} = '{value}' is not a \
                     time of day in HH:MM format"
                ))
            })?;
        }
        if self.from == self.to {
            return Err(ConfigError::Invalid(format!(
                "Policy '{policy}', schedule '{name}': from and to are equal — the window \
                 would be either always or never open"
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
        .map_err(|_| format!("'{text}' is neither an address nor a network"))
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
    #[serde(default)]
    pub rate_limit: RateLimitConfig,
}

/// Drosselung pro Client.
///
/// **Per Default an** (CLAUDE.md B.5): ein Resolver ohne Limit ist ein
/// Amplification-Reflektor, sobald er nicht mehr nur sein eigenes LAN sieht,
/// und ob das so ist, weiß die Konfiguration nicht sicher.
///
/// Die Zahlen sind bewusst großzügig. Der teurere Fehler ist der Fehlalarm:
/// ein gedrosselter Browser sieht aus wie kaputtes Internet, und wer das
/// erlebt, schaltet die Drosselung ab — dann ist sie auch gegen Missbrauch weg.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RateLimitConfig {
    #[serde(default = "default_true")]
    pub enabled: bool,
    /// Anfragen je Sekunde, die einem Client dauerhaft zustehen.
    #[serde(default = "default_per_client_qps")]
    pub per_client_qps: u32,
    /// Guthaben, das ein stiller Client ansammeln darf. Eine Seite mit vielen
    /// Einbettungen löst auf einen Schlag Dutzende Anfragen aus; ohne Spitze
    /// wäre das der erste Fehlalarm.
    #[serde(default = "default_burst")]
    pub burst: u32,
    /// Wie viele Clients gleichzeitig beobachtet werden. Die Grenze ist der
    /// Schutz gegen eine Flut gefälschter Absenderadressen: ohne sie wäre die
    /// Drosselung selbst der Speicherfresser, den sie verhindern soll.
    #[serde(default = "default_max_clients")]
    pub max_clients: usize,
}

impl Default for RateLimitConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            per_client_qps: default_per_client_qps(),
            burst: default_burst(),
            max_clients: default_max_clients(),
        }
    }
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
                "strategy = \"{text}\" no longer exists; split_by_zone applies. \
                 With both removed strategies every upstream ends up seeing everything \
                 (ARCHITECTURE.md §5). Remove the key or set it to split_by_zone."
            ))),
            other => Err(serde::de::Error::custom(format!(
                "unknown strategy '{other}' — split_by_zone is the only one allowed"
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
            .map_err(|e| format!("'{value}' is not a valid zone name: {e}"))
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
            "'{host}' is not an IP address — names are not allowed here, \
                              because resolving them would need DNS again"
        )
    })?;
    Ok(SocketAddr::new(ip, default_port))
}

impl TryFrom<String> for UpstreamAddr {
    type Error = String;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        let Some((scheme, rest)) = value.split_once("://") else {
            return Err(format!(
                "'{value}' has no scheme — expected is something like 'dot://9.9.9.9:853'"
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
                "unknown transport '{other}' — the allowed ones are udp, dot, doh, doq"
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

/// 100 Anfragen je Sekunde und Client.
///
/// Gemessen an dem, was ein Gerät im Betrieb tatsächlich erzeugt, ist das viel:
/// ein Seitenaufruf mit vielen Einbettungen liegt im Bereich von Dutzenden
/// Anfragen, und die kommen aus dem Burst. Als Reflektor-Bremse reicht es
/// trotzdem — 100 Antworten je Sekunde und Quelladresse sind kein Angriff,
/// mit dem sich jemand Mühe geben würde.
/// Ist diese Adresse nur aus dem eigenen Netz erreichbar?
///
/// `false` für die Wildcard-Adressen: sie binden an alles, was da ist.
/// `Ipv6Addr::is_unique_local` und `is_unicast_link_local` sind in stable Rust
/// noch nicht verfügbar, deshalb hier von Hand — die Präfixe stehen in RFC 4193
/// und RFC 4291.
fn is_local_address(addr: std::net::IpAddr) -> bool {
    match addr {
        std::net::IpAddr::V4(v4) => {
            !v4.is_unspecified() && (v4.is_loopback() || v4.is_private() || v4.is_link_local())
        }
        std::net::IpAddr::V6(v6) => {
            if let Some(mapped) = v6.to_ipv4_mapped() {
                return is_local_address(std::net::IpAddr::V4(mapped));
            }
            if v6.is_unspecified() {
                return false;
            }
            let first = v6.segments().first().copied().unwrap_or_default();
            v6.is_loopback() || (first & 0xfe00) == 0xfc00 || (first & 0xffc0) == 0xfe80
        }
    }
}

const fn default_per_client_qps() -> u32 {
    100
}

const fn default_burst() -> u32 {
    200
}

const fn default_max_clients() -> usize {
    8192
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
                "no listener configured: server.listen_udp and server.listen_tcp are both empty"
                    .to_owned(),
            ));
        }
        if self.server.rate_limit.enabled {
            let limit = &self.server.rate_limit;
            if limit.per_client_qps == 0 {
                return Err(ConfigError::Invalid(
                    "server.rate_limit.per_client_qps = 0 does not throttle, it locks out: \
                     after the burst no credit is replenished. Either set a rate or set \
                     enabled = false."
                        .to_owned(),
                ));
            }
            if limit.burst == 0 {
                return Err(ConfigError::Invalid(
                    "server.rate_limit.burst = 0 lets not a single query through".to_owned(),
                ));
            }
            if limit.max_clients == 0 {
                return Err(ConfigError::Invalid(
                    "server.rate_limit.max_clients = 0 gives bookkeeping without room".to_owned(),
                ));
            }
        }
        if self.cache.max_entries == 0 {
            return Err(ConfigError::Invalid(
                "cache.max_entries = 0 does not switch the cache off, it yields a cache \
                 without room; set a sensible value"
                    .to_owned(),
            ));
        }
        if self.cache.min_ttl > self.cache.max_ttl {
            return Err(ConfigError::Invalid(
                "cache.min_ttl is greater than cache.max_ttl".to_owned(),
            ));
        }
        self.detection.validate()?;
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
                            "privacy.odoh is enabled, but resolver '{}' in pool '{}' \
                             speaks {}. Oblivious DoH exists only over doh://; either \
                             switch every resolver to doh:// or turn odoh off.",
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
                "cache.prefetch_threshold must lie between 0.0 and 1.0, but is {}",
                self.cache.prefetch_threshold
            )));
        }
        if self
            .upstream_pool
            .iter()
            .all(|pool| pool.resolver.is_empty())
        {
            return Err(ConfigError::Invalid(
                "no upstream configured: at least one \
                 [[upstream_pool.resolver]] is needed"
                    .to_owned(),
            ));
        }
        if self.upstream_pool.len() > 1 {
            return Err(ConfigError::Invalid(format!(
                "{} upstream pools configured. Which pool applies to which client is \
                 decided by policies, which arrive in phase 5. Until then exactly one is allowed.",
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
                    "the list '{}' is configured twice; policies refer to it by \
                     name, so the name must be unique",
                    list.name
                )));
            }
        }
        let policy_names: std::collections::HashSet<&str> =
            self.policy.iter().map(|p| p.name.as_str()).collect();
        for client in &self.client {
            if client.matches.ip.is_empty() {
                return Err(ConfigError::Invalid(format!(
                    "Client '{}': no match.ip given — it could never be recognized",
                    client.name
                )));
            }
            for address in &client.matches.ip {
                parse_net(address)
                    .map_err(|e| ConfigError::Invalid(format!("Client '{}': {e}", client.name)))?;
            }
            if !policy_names.contains(client.policy.as_str()) {
                return Err(ConfigError::Invalid(format!(
                    "Client '{}' refers to the policy '{}', which does not exist",
                    client.name, client.policy
                )));
            }
        }
        if !self.client.is_empty() && !policy_names.contains("default") {
            return Err(ConfigError::Invalid(
                "there are clients, but no policy named 'default' — for everything that \
                 matches no client entry there would then be no rule"
                    .to_owned(),
            ));
        }
        for policy in &self.policy {
            for list in policy.blocklists.iter().chain(policy.allowlists.iter()) {
                if !list_names.contains(list.as_str()) {
                    return Err(ConfigError::Invalid(format!(
                        "Policy '{}' refers to the list '{list}', which does not exist",
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
                "metrics.path = '{}' must begin with a slash",
                self.metrics.path
            )));
        }
        if self.api.enabled && self.api.listen == self.metrics.listen {
            return Err(ConfigError::Invalid(
                "api.listen and metrics.listen are the same — metrics deliberately \
                 carry no token and must not sit on the same port as the API, \
                 which shows names"
                    .to_owned(),
            ));
        }
        for zone in &self.forward_zone {
            if zone.upstream.is_encrypted() {
                return Err(ConfigError::Invalid(format!(
                    "forward_zone '{}': encrypted transports are not implemented here \
                     yet. A forward_zone points at a nameserver in your own network; \
                     udp:// is what is meant for that.",
                    zone.zone.0
                )));
            }
        }
        self.validate_local_zones()?;
        Ok(())
    }

    /// Prüft die lokalen Zonen und ihr Verhältnis zu `[[forward_zone]]`.
    ///
    /// Baut die Tabellen wirklich, statt die Konfiguration nur anzusehen: so
    /// fällt ein Tippfehler beim Start auf (`alpendns check` ruft dieselbe
    /// Validierung) und nicht erst, wenn die Zone das erste Mal greifen soll.
    fn validate_local_zones(&self) -> Result<(), ConfigError> {
        build_zones(&self.local_zone)?;
        for local in &self.local_zone {
            for forward in &self.forward_zone {
                // Eine Zone ist entweder lokal oder weitergeleitet. Beides
                // zugleich wäre eine Frage der Reihenfolge — und die soll
                // niemand raten müssen.
                if local.zone.0.zone_of(&forward.zone.0) || forward.zone.0.zone_of(&local.zone.0) {
                    return Err(ConfigError::Invalid(format!(
                        "local_zone '{}' and forward_zone '{}' overlap. local_zone \
                         answers itself, forward_zone passes on to another nameserver — \
                         for the same zone only one of them can apply. One of the two \
                         zones has to go elsewhere.",
                        local.zone.0, forward.zone.0
                    )));
                }
            }
        }
        Ok(())
    }
}

impl DetectionConfig {
    fn validate(&self) -> Result<(), ConfigError> {
        for (name, threshold) in [
            ("dga", self.dga.threshold),
            ("tunneling", self.tunneling.threshold),
            ("typosquat", self.typosquat.threshold),
        ] {
            if !(0.0..=1.0).contains(&threshold) || !threshold.is_finite() {
                return Err(ConfigError::Invalid(format!(
                    "detection.{name}.threshold must lie between 0.0 and 1.0, but is \
                     {threshold}"
                )));
            }
        }
        if self.tunneling.window.is_zero() && !self.tunneling.action.is_off() {
            return Err(ConfigError::Invalid(
                "detection.tunneling.window = 0 gives a window without duration; the \
                 detector could not count anything. Either set a duration or action = \"off\"."
                    .to_owned(),
            ));
        }
        // Eine Schutzliste ohne Einträge ist kein Fehler — sie ist der
        // Auslieferungszustand. Ein *Eintrag*, der keine Domain ist, schon:
        // sonst schützt jemand `sparkasse` und wundert sich, dass nichts
        // passiert.
        for entry in &self.typosquat.protect {
            if !entry.contains('.') || entry.trim().is_empty() {
                return Err(ConfigError::Invalid(format!(
                    "detection.typosquat.protect: '{entry}' is not a domain — expected is \
                     something like 'sparkasse.at'"
                )));
            }
        }
        Ok(())
    }
}

impl Config {
    /// Die Listener, die nicht nur im eigenen Netz erreichbar sind.
    ///
    /// Grundlage von `alpendns check` und damit des Auslieferungszustands
    /// (ROADMAP Phase 9, Schritt 6): frisch installiert soll der Server von
    /// außen nicht erreichbar sein. Die Wildcard-Adressen `0.0.0.0` und `[::]`
    /// zählen dazu — sie lauschen auf *jeder* Schnittstelle, und ob eine davon
    /// am Internet hängt, weiß die Konfiguration nicht.
    pub fn public_listeners(&self) -> Vec<SocketAddr> {
        self.server
            .listen_udp
            .iter()
            .chain(self.server.listen_tcp.iter())
            .chain(self.api.enabled.then_some(&self.api.listen))
            .chain(self.metrics.enabled.then_some(&self.metrics.listen))
            .filter(|addr| !is_local_address(addr.ip()))
            .copied()
            .collect()
    }

    /// Die Zonen und Namen, in denen private Adressen erlaubt sind.
    ///
    /// Die konfigurierten plus **alle `forward_zone`-Einträge** plus die
    /// lokalen Zonen. Ohne diese Ergänzung wäre der Rebinding-Schutz beim
    /// ersten Start eine Falle: der eigene LAN-Nameserver antwortet für
    /// `home.arpa` naturgemäß mit `192.168.x.y`, und genau das ist der Treffer,
    /// auf den der Detektor wartet. Wer eine Zone ausdrücklich ins eigene Netz
    /// leitet, hat damit schon gesagt, dass private Adressen von dort in
    /// Ordnung sind. Für eine lokale Zone gilt dasselbe — aber **je Name**,
    /// siehe [`LocalZone::rebinding_exempt`].
    pub fn rebinding_allow_zones(&self) -> Vec<Name> {
        // `build_zones` schlägt hier nicht fehl: `validate()` hat dieselbe
        // Funktion beim Laden schon laufen lassen und den Start abgebrochen,
        // wenn sie fehlschlug. Sollte es doch einmal so weit kommen, ist die
        // leere Liste die sichere Richtung — dann gilt der Schutz überall.
        let local = build_zones(&self.local_zone).unwrap_or_default();
        self.detection
            .rebinding
            .allow_zones
            .iter()
            .map(|zone| zone.0.clone())
            .chain(self.forward_zone.iter().map(|zone| zone.zone.0.clone()))
            .chain(local.iter().flat_map(LocalZone::rebinding_exempt))
            .collect()
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
                "privacy.odoh.enabled = true, but no proxy given. Without a proxy ODoH \
                 would only be a second encryption to the same destination and would \
                 hide nothing."
                    .to_owned(),
            ));
        };
        if !proxy.starts_with("https://") {
            return Err(ConfigError::Invalid(format!(
                "privacy.odoh.proxy = '{proxy}': the proxy must be reachable over \
                 https. Over http anyone listening would see the destination and the \
                 time of every query."
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
                "Pool '{pool}' has no resolver"
            )));
        }
        if self.fanout.is_some() {
            return Err(ConfigError::Invalid(format!(
                "Pool '{pool}': fanout no longer exists. Exactly one resolver is \
                 always asked, and the next one only if it fails. Parallel \
                 queries showed the same question to several providers and \
                 defeated split_by_zone. Please remove the key."
            )));
        }
        for resolver in &self.resolver {
            let name = &resolver.name;
            // B.1 Regel 7: kein Klartext-DNS nach außen. Die einzige Ausnahme
            // sind forward_zone-Einträge ins eigene LAN, und die stehen nicht
            // in einem Pool.
            if !resolver.addr.is_encrypted() {
                return Err(ConfigError::Invalid(format!(
                    "Pool '{pool}', resolver '{name}': cleartext DNS is not allowed as an \
                     upstream. Use dot://, doh:// or doq://. For a nameserver in your own \
                     network, [[forward_zone]] is the right place."
                )));
            }
            if resolver
                .tls_name
                .as_ref()
                .is_none_or(|n| n.trim().is_empty())
            {
                return Err(ConfigError::Invalid(format!(
                    "Pool '{pool}', resolver '{name}': tls_name is missing. Without it the \
                     certificate is checked against nothing and the encryption is worthless."
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
    fn the_detection_defaults_flag_and_never_block() {
        // Die Zusage aus der Roadmap und aus B.8, hier auf Ebene der
        // Konfiguration. Der Gegentest durch die Pipeline steht in
        // tests/detection.rs.
        let config = valid(MINIMAL);
        let actions = [
            config.detection.dga.action,
            config.detection.tunneling.action,
            config.detection.rebinding.action,
            config.detection.typosquat.action,
            config.detection.nrd.action,
        ];
        for action in actions {
            assert_eq!(action, crate::detect::Action::Flag, "{action:?}");
        }
        assert!(config.detection.typosquat.protect.is_empty());
        assert!(config.detection.nrd.source.is_none());
    }

    #[test]
    fn the_example_configs_detection_block_parses() {
        // Die Beispielkonfiguration ist die Spezifikation des Zielformats
        // (CLAUDE.md B.0). Wenn sie nicht parst, stimmt eine der beiden Seiten
        // nicht mehr.
        let text = format!(
            "{MINIMAL}\n{}",
            r#"
[detection]
dga = { action = "flag", threshold = 0.75 }
tunneling = { action = "flag", threshold = 0.75, window = "5m", allow_zones = [] }
rebinding = { action = "flag", allow_zones = [] }
typosquat = { action = "flag", threshold = 0.85, protect = ["sparkasse.at"] }
nrd = { action = "flag", max_age = "30d", source = "/var/lib/alpendns/nrd.txt" }
"#
        );
        let config = valid(&text);
        assert_eq!(config.detection.typosquat.protect.len(), 1);
        assert_eq!(config.detection.tunneling.window, Duration::from_secs(300));
    }

    #[test]
    fn the_detector_line_names_every_stage() {
        let dir = std::env::temp_dir().join(format!("alpendns-config-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("Testverzeichnis");
        let nrd = dir.join("nrd.txt");
        std::fs::write(&nrd, "frisch.example 2026-08-28\n").expect("schreiben");

        let block = format!(
            "[detection]\n\
             dga = {{ action = \"block\" }}\n\
             tunneling = {{ action = \"off\" }}\n\
             rebinding = {{ action = \"log\" }}\n\
             typosquat = {{ action = \"flag\", protect = [\"sparkasse.at\"] }}\n\
             nrd = {{ action = \"flag\", source = \"{}\" }}\n",
            nrd.display()
        );
        let config = valid(&format!("{MINIMAL}\n{block}"));
        assert_eq!(
            config.detection.check_line(),
            "dga block, tunneling off, rebinding log, typosquat flag, nrd flag"
        );

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_detector_that_can_find_nothing_says_so() {
        // Genau die Falle aus der ersten Beobachtungswoche: der Detektor steht
        // auf `flag` und hat nichts, wogegen er vergleichen könnte. In der
        // Auswertung sieht das aus wie "keine Fehlalarme".
        let config = valid(MINIMAL);
        assert_eq!(
            config.detection.check_line(),
            "dga flag, tunneling flag, rebinding flag, \
             typosquat flag (no protect: finds nothing), \
             nrd flag (no source: runs empty)"
        );
    }

    #[test]
    fn the_nrd_note_distinguishes_missing_from_empty() {
        let dir = std::env::temp_dir().join(format!("alpendns-config-nrd-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("Testverzeichnis");
        let leer = dir.join("leer.txt");
        std::fs::write(&leer, "").expect("schreiben");
        let voll = dir.join("voll.txt");
        std::fs::write(&voll, "frisch.example 2026-08-28\n").expect("schreiben");

        let line = |source: &std::path::Path| {
            let text = format!(
                "{MINIMAL}\n[detection]\nnrd = {{ action = \"flag\", source = \"{}\" }}\n",
                source.display()
            );
            valid(&text).detection.check_line()
        };

        let fehlt = line(&dir.join("gibts-nicht.txt"));
        assert!(
            fehlt.ends_with("gibts-nicht.txt is missing: runs empty)"),
            "{fehlt}"
        );
        let leer = line(&leer);
        assert!(leer.ends_with("leer.txt is empty: runs empty)"), "{leer}");
        // Eine Datei, die da ist: kein Hinweis. Typosquat meldet sich weiter,
        // weil sein `protect` in MINIMAL leer ist.
        assert_eq!(
            line(&voll),
            "dga flag, tunneling flag, rebinding flag, \
             typosquat flag (no protect: finds nothing), nrd flag"
        );

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_detector_that_is_off_gets_no_hint() {
        // `off` heißt: soll nichts finden. Ein Hinweis darauf wäre Rauschen.
        let text = format!(
            "{MINIMAL}\n[detection]\n\
             typosquat = {{ action = \"off\", protect = [] }}\n\
             nrd = {{ action = \"off\", source = \"/gibt/es/nicht.txt\" }}\n"
        );
        let config = valid(&text);
        assert_eq!(
            config.detection.check_line(),
            "dga flag, tunneling flag, rebinding flag, typosquat off, nrd off"
        );
    }

    #[test]
    fn an_unknown_detector_action_says_what_is_allowed() {
        let text = format!("{MINIMAL}\n[detection]\ndga = {{ action = \"warn\" }}\n");
        let error = parse(&text).expect_err("muss abbrechen").to_string();
        assert!(error.contains("off, log, flag, block"), "{error}");
    }

    #[test]
    fn a_threshold_outside_the_range_is_a_startup_error() {
        for bad in ["1.5", "-0.2"] {
            let text = format!("{MINIMAL}\n[detection]\ndga = {{ threshold = {bad} }}\n");
            let config = parse(&text).expect("parst");
            assert!(config.validate().is_err(), "threshold = {bad} akzeptiert");
        }
    }

    #[test]
    fn a_protect_entry_that_is_not_a_domain_is_a_startup_error() {
        // Sonst schützt jemand 'sparkasse' und wundert sich, dass nichts
        // passiert.
        let text = format!("{MINIMAL}\n[detection]\ntyposquat = {{ protect = [\"sparkasse\"] }}\n");
        let config = parse(&text).expect("parst");
        let error = config.validate().expect_err("muss abbrechen").to_string();
        assert!(error.contains("is not a domain"), "{error}");
    }

    #[test]
    fn forward_zones_join_the_rebinding_allow_list() {
        let text = format!(
            "{MINIMAL}\n{}",
            r#"
[[forward_zone]]
zone = "home.arpa"
upstream = "udp://10.0.0.1:53"

[detection]
rebinding = { allow_zones = ["intern.example"] }
"#
        );
        let zones: Vec<String> = valid(&text)
            .rebinding_allow_zones()
            .iter()
            .map(ToString::to_string)
            .collect();
        assert!(zones.contains(&"intern.example.".to_owned()), "{zones:?}");
        assert!(zones.contains(&"home.arpa.".to_owned()), "{zones:?}");
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
        assert!(err.contains("cleartext"), "{err}");
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
    fn a_local_zone_is_checked_when_the_config_is_loaded() {
        // Nicht erst in `main`: `alpendns check` ruft dieselbe Validierung, und
        // ein Tippfehler soll beim Start auffallen, nicht beim ersten Treffer.
        let text = format!(
            "{MINIMAL}\n[[local_zone]]\nzone = \"Miloo.AT\"\n\
             records = [{{ name = \"nas\", type = \"A\", value = \"192.168.1.5\" }}]\n"
        );
        let config = valid(&text);
        let zone = config.local_zone.first().expect("eine Zone");
        assert_eq!(zone.zone.0.to_ascii(), "miloo.at.");
        assert_eq!(zone.ttl, 300);
        assert_eq!(zone.fallback, crate::local::Fallback::Upstream);
    }

    #[test]
    fn a_local_record_that_cannot_work_is_a_startup_error() {
        let text = format!(
            "{MINIMAL}\n[[local_zone]]\nzone = \"miloo.at\"\n\
             records = [{{ name = \"mail\", type = \"MX\", value = \"10 mail.miloo.at.\" }}]\n"
        );
        let err = valid_err(&text);
        assert!(err.contains("MX"), "{err}");
        assert!(err.contains("AAAA"), "{err}");
    }

    #[test]
    fn the_root_zone_is_not_a_local_zone() {
        let text = format!("{MINIMAL}\n[[local_zone]]\nzone = \".\"\n");
        let err = valid_err(&text);
        assert!(err.contains("root zone"), "{err}");
    }

    #[test]
    fn two_local_zones_may_not_be_the_same_zone() {
        let text = format!(
            "{MINIMAL}\n[[local_zone]]\nzone = \"miloo.at\"\n\
             \n[[local_zone]]\nzone = \"miloo.at\"\n"
        );
        let err = valid_err(&text);
        assert!(err.contains("twice"), "{err}");
    }

    #[test]
    fn a_local_zone_may_not_overlap_a_forward_zone() {
        // Gleich, enthalten, enthaltend — in allen drei Fällen wäre nicht
        // vorhersagbar, wer gewinnt.
        for (local, forward) in [
            ("miloo.at", "miloo.at"),
            ("sub.miloo.at", "miloo.at"),
            ("miloo.at", "sub.miloo.at"),
        ] {
            let text = format!(
                "{MINIMAL}\n[[forward_zone]]\nzone = \"{forward}\"\n\
                 upstream = \"udp://10.0.0.1:53\"\n\
                 \n[[local_zone]]\nzone = \"{local}\"\n"
            );
            let err = valid_err(&text);
            assert!(err.contains("overlap"), "{local} / {forward}: {err}");
        }
    }

    #[test]
    fn a_local_zone_beside_a_forward_zone_is_fine() {
        // Die Gegenprobe: die Prüfung darf nicht jedes zweite Zonenpaar
        // ablehnen.
        let text = format!(
            "{MINIMAL}\n[[forward_zone]]\nzone = \"home.arpa\"\n\
             upstream = \"udp://10.0.0.1:53\"\n\
             \n[[local_zone]]\nzone = \"miloo.at\"\n"
        );
        let config = valid(&text);
        assert_eq!(config.local_zone.len(), 1);
        assert_eq!(config.forward_zone.len(), 1);
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
        assert!(err.contains("IP address"), "{err}");
    }

    #[test]
    fn address_without_scheme_is_rejected() {
        let err = UpstreamAddr::try_from("9.9.9.9:853".to_owned()).expect_err("kein Schema");
        assert!(err.contains("scheme"), "{err}");
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
        assert!(err.contains("Exactly one"), "{err}");
    }

    #[test]
    fn config_without_listener_is_rejected() {
        let text = MINIMAL
            .replace("listen_udp = [\"127.0.0.1:5353\"]", "listen_udp = []")
            .replace("listen_tcp = [\"127.0.0.1:5353\"]", "listen_tcp = []");
        let err = valid_err(&text);
        assert!(err.contains("listener"), "{err}");
    }

    /// Die Datei, die das Debian-Paket nach `/etc/alpendns` legt. Sie ist der
    /// Auslieferungszustand, und der ist ein Abnahmekriterium (ROADMAP Phase 9,
    /// Schritt 6): frisch installiert darf der Server von außen nicht
    /// erreichbar sein.
    const PACKAGED: &str = include_str!("../../../packaging/alpendns.toml");

    #[test]
    fn the_packaged_configuration_is_valid() {
        let config = parse(PACKAGED).expect("die ausgelieferte Datei muss parsen");
        config
            .validate()
            .expect("die ausgelieferte Datei muss gültig sein");
    }

    /// Die Unit-Datei, die neben der Konfiguration ausgeliefert wird.
    const UNIT: &str = include_str!("../../../packaging/systemd/alpendns.service");

    /// `alpendns check` läuft als `ExecStartPre` und verlangt, dass die
    /// Verzeichnisse **schon da** sind. Angelegt werden sie von systemd, nicht
    /// vom Paket — also müssen beide Dateien dieselben Pfade meinen. Taten sie
    /// einmal nicht: `CacheDirectory=alpendns` legt `/var/cache/alpendns` an,
    /// die Konfiguration zeigte auf `/var/cache/alpendns/lists`, und der erste
    /// Start nach `apt install` scheiterte.
    #[test]
    fn systemd_creates_every_directory_the_packaged_configuration_needs() {
        let mut created: Vec<String> = Vec::new();
        for line in UNIT.lines().map(str::trim) {
            for (key, root) in [
                ("StateDirectory=", "/var/lib/"),
                ("CacheDirectory=", "/var/cache/"),
                ("LogsDirectory=", "/var/log/"),
            ] {
                if let Some(rest) = line.strip_prefix(key) {
                    created.extend(rest.split_whitespace().map(|d| format!("{root}{d}")));
                }
            }
        }

        let config = valid(PACKAGED);
        let mut needed = vec![config.blocking.cache_dir.clone()];
        if config.api.enabled {
            needed.push(
                config
                    .api
                    .token_file
                    .parent()
                    .expect("token_file hat ein Verzeichnis")
                    .to_path_buf(),
            );
        }

        for dir in needed {
            let dir = dir.display().to_string();
            assert!(
                created.contains(&dir),
                "{dir} braucht die Konfiguration, aber keine *Directory=-Zeile der Unit \
                 legt es an: {created:?}"
            );
        }
    }

    #[test]
    fn the_packaged_configuration_listens_nowhere_public() {
        let config = valid(PACKAGED);
        assert!(
            config.public_listeners().is_empty(),
            "das Paket liefert einen von außen erreichbaren Listener aus: {:?}",
            config.public_listeners()
        );
    }

    /// Ohne Drosselung wäre der erste Handgriff nach der Installation — den
    /// Listener auf die LAN-Adresse setzen — zugleich der Schritt, der einen
    /// Amplification-Reflektor aufmacht (CLAUDE.md B.5).
    #[test]
    fn the_packaged_configuration_throttles_per_client() {
        let config = valid(PACKAGED);
        assert!(config.server.rate_limit.enabled);
        assert!(config.server.rate_limit.per_client_qps > 0);
    }

    #[test]
    fn rate_limiting_is_on_unless_it_is_switched_off() {
        let config = valid(MINIMAL);
        assert!(
            config.server.rate_limit.enabled,
            "die Drosselung war ohne Zutun aus"
        );
    }

    #[test]
    fn a_wildcard_listener_counts_as_public() {
        let text = MINIMAL.replace("127.0.0.1:5353", "0.0.0.0:5353");
        let config = valid(&text);
        assert_eq!(
            config.public_listeners().len(),
            2,
            "0.0.0.0 galt als privat; es bindet an jede Schnittstelle"
        );
    }

    #[test]
    fn private_ranges_do_not_count_as_public() {
        for address in ["10.1.2.3:53", "192.168.1.10:53", "172.16.0.1:53"] {
            let text = MINIMAL.replace("127.0.0.1:5353", address);
            let config = valid(&text);
            assert!(
                config.public_listeners().is_empty(),
                "{address} galt als öffentlich"
            );
        }
    }

    #[test]
    fn a_routable_address_counts_as_public() {
        let text = MINIMAL.replace("127.0.0.1:5353", "203.0.113.7:53");
        let config = valid(&text);
        assert_eq!(config.public_listeners().len(), 2);
    }

    #[test]
    fn a_rate_limit_without_refill_is_rejected() {
        let text = format!("{MINIMAL}\n[server.rate_limit]\nper_client_qps = 0\n");
        let err = valid_err(&text);
        assert!(err.contains("per_client_qps"), "{err}");
    }

    #[test]
    fn config_without_upstream_is_rejected() {
        let text = MINIMAL
            .split("[[upstream_pool.resolver]]")
            .next()
            .expect("Kopf")
            .to_owned();
        let err = valid_err(&text);
        assert!(err.contains("upstream"), "{err}");
    }
}
