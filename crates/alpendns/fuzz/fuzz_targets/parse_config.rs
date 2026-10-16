//! Fuzzt das Laden der Konfiguration.
//!
//! Die Datei kommt vom Admin, kann aber kaputt sein. Geprüft wird der Weg, den
//! der Server beim Start nimmt — `Config::load`, also derselbe TOML-Parser und
//! dieselbe Validierung —, nicht eine Kopie davon. Deshalb der Umweg über eine
//! Datei: `load` nimmt einen Pfad.

#![no_main]

use std::path::PathBuf;
use std::sync::OnceLock;

use alpendns::config::Config;
use libfuzzer_sys::fuzz_target;

fn scratch() -> &'static PathBuf {
    static PATH: OnceLock<PathBuf> = OnceLock::new();
    PATH.get_or_init(|| {
        std::env::temp_dir().join(format!("alpendns-fuzz-config-{}.toml", std::process::id()))
    })
}

fuzz_target!(|data: &[u8]| {
    if std::fs::write(scratch(), data).is_ok() {
        let _ = Config::load(scratch());
    }
});
