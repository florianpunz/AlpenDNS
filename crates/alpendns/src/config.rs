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

use serde::Deserialize;

/// Fehler beim Laden oder Validieren der Konfiguration.
#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("Konfigurationsdatei {path} konnte nicht gelesen werden: {source}")]
    Read {
        path: String,
        #[source]
        source: std::io::Error,
    },
    #[error("Konfigurationsdatei {path} ist ungültig: {source}")]
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

/// Eine Menge gleichwertiger Upstream-Resolver.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UpstreamPool {
    pub name: String,
    #[serde(default)]
    pub resolver: Vec<ResolverConfig>,
}

/// Ein einzelner Upstream-Resolver.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ResolverConfig {
    pub name: String,
    pub addr: UpstreamAddr,
}

/// Adresse eines Upstreams, geparst aus `schema://host:port`.
///
/// Phase 1 kann ausschließlich `udp://`. Das verletzt B.1 Regel 7 ("kein
/// Klartext-DNS nach außen") mit Absicht und nur so lange, bis Phase 3 die
/// verschlüsselten Transporte bringt — die Roadmap schneidet das bewusst so.
/// Jede andere Angabe wird deshalb mit einem Hinweis auf Phase 3 abgelehnt,
/// statt stillschweigend auf Klartext zurückzufallen.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(try_from = "String")]
pub struct UpstreamAddr(pub SocketAddr);

impl TryFrom<String> for UpstreamAddr {
    type Error = String;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        let Some((scheme, rest)) = value.split_once("://") else {
            return Err(format!(
                "'{value}' hat kein Schema — erwartet wird 'udp://adresse:port'"
            ));
        };
        match scheme {
            "udp" => rest
                .parse::<SocketAddr>()
                .map(Self)
                .map_err(|e| format!("'{rest}' ist keine gültige Adresse: {e}")),
            "dot" | "doh" | "doq" | "tls" | "https" | "quic" => Err(format!(
                "Transport '{scheme}' kommt erst in Phase 3; Phase 1 kann nur 'udp://'"
            )),
            other => Err(format!("unbekannter Transport '{other}'")),
        }
    }
}

const fn default_query_timeout() -> Duration {
    Duration::from_secs(3)
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
        let resolvers: usize = self.upstream_pool.iter().map(|p| p.resolver.len()).sum();
        match resolvers {
            0 => Err(ConfigError::Invalid(
                "kein Upstream konfiguriert: es braucht genau einen [[upstream_pool.resolver]]"
                    .to_owned(),
            )),
            1 => Ok(()),
            n => Err(ConfigError::Invalid(format!(
                "{n} Upstreams konfiguriert, Phase 1 kann genau einen; \
                 Pools und Auswahlstrategien kommen in Phase 3"
            ))),
        }
    }

    /// Der eine Upstream dieser Phase.
    pub fn single_upstream(&self) -> Option<&ResolverConfig> {
        self.upstream_pool.iter().flat_map(|p| &p.resolver).next()
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
name = "fake"
addr = "udp://127.0.0.1:5300"
"#;

    fn parse(text: &str) -> Result<Config, toml::de::Error> {
        toml::from_str(text)
    }

    #[test]
    fn minimal_config_parses_with_defaults() {
        let config = parse(MINIMAL).expect("minimale Konfiguration muss parsen");
        config.validate().expect("und gültig sein");
        assert_eq!(config.server.query_timeout, Duration::from_secs(3));
        assert_eq!(config.server.edns.udp_payload_size, 1232);
        assert_eq!(
            config.single_upstream().map(|r| r.addr),
            Some(UpstreamAddr("127.0.0.1:5300".parse().expect("gültig")))
        );
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
    fn encrypted_transports_are_rejected_with_a_hint_to_phase_3() {
        let text = MINIMAL.replace("udp://127.0.0.1:5300", "dot://9.9.9.9:853");
        let err = parse(&text).expect_err("dot:// kann Phase 1 nicht");
        assert!(err.to_string().contains("Phase 3"), "{err}");
    }

    #[test]
    fn address_without_scheme_is_rejected() {
        let text = MINIMAL.replace("udp://127.0.0.1:5300", "127.0.0.1:5300");
        let err = parse(&text).expect_err("Adresse ohne Schema ist ungültig");
        assert!(err.to_string().contains("Schema"), "{err}");
    }

    #[test]
    fn more_than_one_upstream_is_rejected_in_phase_1() {
        let text = format!(
            "{MINIMAL}\n[[upstream_pool.resolver]]\nname = \"zweiter\"\naddr = \"udp://127.0.0.1:5301\"\n"
        );
        let config = parse(&text).expect("parst syntaktisch");
        let err = config.validate().expect_err("zwei Upstreams sind Phase 3");
        assert!(err.to_string().contains("Phase 3"), "{err}");
    }

    #[test]
    fn config_without_listener_is_rejected() {
        let text = MINIMAL
            .replace("listen_udp = [\"127.0.0.1:5353\"]", "listen_udp = []")
            .replace("listen_tcp = [\"127.0.0.1:5353\"]", "listen_tcp = []");
        let config = parse(&text).expect("parst syntaktisch");
        let err = config.validate().expect_err("ohne Listener kein Server");
        assert!(err.to_string().contains("Listener"), "{err}");
    }
}
