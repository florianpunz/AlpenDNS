//! Fuzzt den Anfragepfad: alles, was mit Bytes direkt vom Netzwerk passiert.
//!
//! Nicht `hickory-proto` selbst wird geprüft, sondern *unsere Verwendung* davon
//! (docs/TESTING.md §3). Erfolgskriterium: kein Panic, keine Endlosschleife.

#![no_main]

use alpendns::dns;
use hickory_proto::op::{Message, ResponseCode};
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    match Message::from_vec(data) {
        Ok(request) => {
            // Der Weg, den eine geparste Anfrage im Server nimmt.
            let _ = dns::check_response(&request, &request);
            let response = dns::error_response(&request, ResponseCode::ServFail);
            let _ = dns::encode_for_udp(&response, 1232);
            let _ = dns::encode_for_udp(&request, 1232);
        }
        Err(_) => {
            // Der Weg, den nicht dekodierbare Bytes nehmen.
            let _ = dns::format_error(data);
        }
    }
});
