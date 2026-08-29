//! AlpenDNS — privacy-fokussierter DNS-Server.
//!
//! Der gesamte testbare Code liegt in dieser Library; das Binary
//! (`main.rs`) macht nur Start, Signale und Shutdown. Die Module hier
//! entsprechen den Crates aus CLAUDE.md B.3: sobald eines groß genug ist, wird
//! aus dem Modul ein eigenes Crate, ohne dass sich die Aufrufwege ändern.

pub mod cache;
pub mod caching;
pub mod clock;
pub mod config;
pub mod dns;
pub mod privacy;
pub mod resolve;
pub mod router;
pub mod server;
pub mod upstream;
