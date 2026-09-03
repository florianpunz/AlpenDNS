//! Der hot-reloadbare Teil der Konfiguration: die Policy-Schicht.
//!
//! Listener-Adressen, TLS-Material, Cache, Drosselung, Block-Modus und
//! Detektoren werden beim Start fest eingebaut und sind nicht hot-reloadbar
//! (siehe `docs/ARCHITECTURE.md` §7). Was hier liegt — der Blueprint (Clients,
//! Policies, Regex, Zeitpläne) und die Listenquellen — lässt sich per `SIGHUP`
//! tauschen, ohne den Prozess neu zu starten.

use std::sync::Arc;

use arc_swap::ArcSwap;

use crate::config::{Config, ConfigError};
use crate::filter::Lists;
use crate::filter::source::{Loader, specs_from_config};
use crate::policy::Blueprint;

/// Der Teil der Konfiguration, den ein Reload austauschen darf.
///
/// Beide Hälften sitzen hinter `ArcSwap`: Anfragen lesen ohne Sperre, ein
/// Reload tauscht den Zeiger atomar (CLAUDE.md B.3 Regel 5). Der Fehlerfall —
/// eine Konfiguration, die sich nicht bauen lässt — lässt die alten Werte
/// stehen.
#[derive(Debug)]
pub struct PolicySource {
    blueprint: ArcSwap<Blueprint>,
    lists: ArcSwap<Lists>,
}

impl PolicySource {
    pub fn new(blueprint: Blueprint, lists: Lists) -> Self {
        Self {
            blueprint: ArcSwap::from_pointee(blueprint),
            lists: ArcSwap::from_pointee(lists),
        }
    }

    /// Baut die Policy-Schicht aus einer neuen Konfiguration neu und tauscht sie ein.
    ///
    /// Schlägt das fehl, bleibt der alte Stand unangetastet — ein Reload darf
    /// nie dazu führen, dass gefiltert werden sollte, aber nicht gefiltert wird
    /// (ARCHITECTURE.md §7). Dass eine Policy auf eine Liste verweist, die nicht
    /// geladen werden kann, fällt hier noch nicht auf; das prüft
    /// [`crate::policy::run_updater`] beim Bauen des Regelstands und lässt dann
    /// ebenfalls den alten Stand gelten.
    pub fn reload(&self, config: &Config, loader: &Loader) -> Result<(), ConfigError> {
        let blueprint = Blueprint::from_config(config)?;
        let specs = specs_from_config(&config.blocklist, &config.allowlist);
        let lists = Lists::new(loader.clone(), specs);
        self.blueprint.store(Arc::new(blueprint));
        self.lists.store(Arc::new(lists));
        Ok(())
    }

    /// Der aktuelle Blueprint — für `run_updater`, das daraus den Regelstand baut.
    pub fn blueprint(&self) -> Arc<Blueprint> {
        self.blueprint.load_full()
    }

    /// Die aktuellen Listenquellen — für `run_updater`, das sie periodisch lädt.
    pub fn lists(&self) -> Arc<Lists> {
        self.lists.load_full()
    }
}

#[cfg(test)]
mod tests {
    use super::PolicySource;
    use crate::clock::{SystemClock, SystemWallClock};
    use crate::config::{BlockingConfig, Config};
    use crate::filter::Lists;
    use crate::filter::source::{Loader, specs_from_config};
    use crate::policy::{Blueprint, Decision, Engine};
    use crate::trace::Ctx;
    use hickory_proto::rr::Name;
    use std::net::{IpAddr, SocketAddr};
    use std::path::{Path, PathBuf};

    /// Ein frisches Verzeichnis für die Listen-Dateien des Tests.
    fn dir() -> PathBuf {
        let path = std::env::temp_dir().join(format!(
            "alpendns-reload-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("Uhr nach 1970")
                .as_nanos()
        ));
        std::fs::create_dir_all(&path).expect("Testverzeichnis");
        path
    }

    fn write(dir: &Path, name: &str, content: &str) -> String {
        let path = dir.join(name);
        std::fs::write(&path, content).expect("Liste schreiben");
        path.to_string_lossy().into_owned()
    }

    /// Eine Konfiguration mit einer Policy `kids` für den Client 10.0.0.1.
    ///
    /// `more` hängt eine zweite Liste an; `regex` ein (ggf. absichtlich kaputtes)
    /// Muster. Ohne `more` blockt die Policy nur die Liste `ads`.
    fn config(ads: &str, more: Option<&str>, regex: &str) -> String {
        let mut text = String::from("[server]\n\n[[blocklist]]\n");
        text.push_str(&format!(
            "name = \"ads\"\npath = \"{ads}\"\nformat = \"wildcard\"\n"
        ));
        if let Some(more) = more {
            text.push_str(&format!(
                "\n[[blocklist]]\nname = \"more\"\npath = \"{more}\"\nformat = \"wildcard\"\n"
            ));
        }
        text.push_str("\n[[policy]]\nname = \"kids\"\n");
        text.push_str(&format!(
            "blocklists = [\"ads\"{}]\n",
            if more.is_some() { ", \"more\"" } else { "" }
        ));
        if !regex.is_empty() {
            text.push_str(&format!("regex = [\"{regex}\"]\n"));
        }
        text.push_str(
            "\n[[client]]\nname = \"kids-tablet\"\nmatch = { ip = [\"10.0.0.1\"] }\npolicy = \"kids\"\n",
        );
        text
    }

    /// Baut aus dem aktuellen Stand des `source` eine Engine und wertet den Namen aus.
    async fn decide(source: &PolicySource, name: &str) -> Decision {
        let loaded = source.lists().load(true).await.expect("Listen laden");
        let engine = Engine::new(
            source.blueprint().build(&loaded).expect("Regelstand"),
            loaded.total_entries(),
            &BlockingConfig::default(),
            SystemClock,
            SystemWallClock,
        );
        let peer: IpAddr = "10.0.0.1".parse().expect("gültige Adresse");
        let name = Name::from_ascii(name).expect("gültiger Name");
        let mut ctx = Ctx::new(SocketAddr::new(peer, 4242));
        engine.evaluate(&name, peer, &mut ctx)
    }

    #[tokio::test]
    async fn reload_swaps_the_policy_source() {
        let dir = dir();
        let ads = write(&dir, "ads.list", "spiel.example\n");
        let more = write(&dir, "more.list", "banned.example\n");
        let loader = Loader::new(dir).expect("Loader");

        let config0: Config = toml::from_str(&config(&ads, None, "")).expect("Config 0");
        let source = PolicySource::new(
            Blueprint::from_config(&config0).expect("Blueprint"),
            Lists::new(
                loader.clone(),
                specs_from_config(&config0.blocklist, &config0.allowlist),
            ),
        );

        assert_eq!(decide(&source, "spiel.example").await, Decision::Block);
        assert_eq!(decide(&source, "banned.example").await, Decision::Allow);

        let config1: Config = toml::from_str(&config(&ads, Some(&more), "")).expect("Config 1");
        source.reload(&config1, &loader).expect("Reload");

        assert_eq!(decide(&source, "banned.example").await, Decision::Block);
        assert_eq!(decide(&source, "spiel.example").await, Decision::Block);
    }

    #[tokio::test]
    async fn reload_rejects_a_broken_config_and_keeps_the_old_state() {
        let dir = dir();
        let ads = write(&dir, "ads.list", "spiel.example\n");
        let loader = Loader::new(dir).expect("Loader");

        let config0: Config = toml::from_str(&config(&ads, None, "")).expect("Config 0");
        let source = PolicySource::new(
            Blueprint::from_config(&config0).expect("Blueprint"),
            Lists::new(
                loader.clone(),
                specs_from_config(&config0.blocklist, &config0.allowlist),
            ),
        );

        // TOML parst, aber der Blueprint nicht: das Muster ist kein gültiges Regex.
        let broken: Config =
            toml::from_str(&config(&ads, None, "([ungeschlossen")).expect("Config parst");
        assert!(source.reload(&broken, &loader).is_err());

        // Der alte Stand gilt unverändert weiter.
        assert_eq!(decide(&source, "spiel.example").await, Decision::Block);
    }
}
