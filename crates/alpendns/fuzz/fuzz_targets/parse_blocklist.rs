//! Fuzzt das Parsen von Blocklist-Zeilen, in allen drei Formaten.
//!
//! Jede Zeile einer Blocklist ist fremde Eingabe (docs/TESTING.md §3). Die
//! Bytes gehen über `from_utf8_lossy` hinein, wie beim Herunterladen.

#![no_main]

use alpendns::filter::parser::{Format, parse};
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let text = String::from_utf8_lossy(data);
    for format in [Format::Hosts, Format::Domains, Format::Wildcard] {
        let _ = parse(&text, format);
    }
});
