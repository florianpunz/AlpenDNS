//! Fuzzt das Parsen einer Upstream-Adresse (`dot://…`, `doh://host/pfad`, …).
//!
//! Die Zeichenkette kommt aus der Konfiguration; `UpstreamAddr::try_from` ist
//! dieselbe Funktion, die serde beim Laden aufruft. Das Target zielt direkt auf
//! sie, statt darauf zu hoffen, dass `parse_config` erst gültiges TOML findet.

#![no_main]

use alpendns::config::UpstreamAddr;
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let _ = UpstreamAddr::try_from(String::from_utf8_lossy(data).into_owned());
});
