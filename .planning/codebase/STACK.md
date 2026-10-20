---
last_mapped_commit: 630297fc35c98188292c5305d0e279aa6039b07a
last_mapped_at: 2026-10-20
---
# Technology Stack

**Analysis Date:** 2026-10-20

## Languages

**Primary:**
- Rust, Edition 2024, `rust-version = "1.85"` — all production code. Single workspace member: `crates/alpendns`.
- TOML — configuration format, parsed with `serde` + `toml` (`crates/alpendns/src/config.rs`).

**Secondary:**
- POSIX shell / Bash — service packaging and release tooling: `packaging/debian/postinst`, `packaging/debian/postrm`, `packaging/debian/prerm`, `scripts/bump-version.sh`, `web/demo/build.sh`.
- HTML/CSS/JavaScript (vanilla, no framework, no bundler) — web UI served by the process: `web/index.html`, `web/app.css`, `web/app.js`. Embedded into the binary with `include_str!` in `crates/alpendns/src/api/ui.rs`.
- Python — acceptance helper only: `packaging/abnahme.py`.

**Toolchain:**
- `rust-toolchain.toml` pins `channel = "stable"` with `rustfmt` and `clippy`; no nightly except `cargo fuzz`.
- Local compiler observed: `rustc 1.98.0`; MSRV remains 1.85 because Debian 13 ships that version (`deny.toml` explains the constraint).

## Runtime

**Environment:**
- Linux only (Debian 12/13, Ubuntu 24.04+ per `docs/OPERATIONS.md`).
- Single process, single binary, one configuration file. No database server, no Redis, no sidecar (`docs/ARCHITECTURE.md` §1).
- Async runtime: **tokio 1.53.x** (`rt-multi-thread`, `net`, `signal`, `time`, `io-util`, `sync`, `fs`).
- Service runtime: **systemd**, unit at `packaging/systemd/alpendns.service`; port 53 obtained through `AmbientCapabilities=CAP_NET_BIND_SERVICE`, not root.

**Package Manager:**
- Cargo (workspace resolver 3).
- Lockfile: `Cargo.lock` present (committed), 322 locked packages.
- Separate fuzz workspace lockfile: `crates/alpendns/fuzz/Cargo.lock`.
- No npm/yarn/pnpm project; `web/` has no `package.json`.

## Frameworks

**Core:**
- `hickory-proto` 0.26.1 — DNS wire format and message types, with `dnssec-ring` (`Cargo.toml`, ADR-0002). No hand-written DNS parser.
- `hickory-net` 0.26.1 — encrypted transports DoT/DoH/DoQ (`tls-ring`, `https-ring`, `quic-ring`, `webpki-roots`, `dnssec-ring`); used in `crates/alpendns/src/upstream/transport.rs`.
- `tokio` 1.53.1 + `tokio-util` 0.7.19 — runtime, cancellation token, graceful shutdown.
- `axum` 0.8.9 — HTTP API, web UI and Prometheus endpoint (`crates/alpendns/src/api/mod.rs`, `crates/alpendns/src/metrics.rs`).
- `rustls` 0.23.46 with the `ring` backend — TLS for upstreams, blocklist downloads and ODoH. `aws-lc-rs` is deliberately excluded because of its OpenSSL licence (`Cargo.toml`, `deny.toml`).
- `reqwest` 0.13.1 (`rustls-no-provider`, `webpki-roots`, `http2`, `gzip`) — blocklist downloads and ODoH HTTP; no native TLS, no system proxy.
- `serde` 1.0.229 + `toml` 1.1.4 — strict configuration deserialization (`deny_unknown_fields`).
- `thiserror` 2.0.20 for library errors; `anyhow` 1.0.104 only in `crates/alpendns/src/main.rs`.
- `tracing` 0.1.44 + `tracing-subscriber` 0.3.23 (`env-filter`) — structured logging to journald/stdout.

**Testing:**
- Built-in `cargo test` (unit tests in-module, integration tests in `crates/alpendns/tests/`).
- `proptest` 1.11.0 — property tests (e.g. `crates/alpendns/src/privacy.rs`, `crates/alpendns/src/filter/parser.rs`, `crates/alpendns/src/cache.rs`).
- `cargo-fuzz` (nightly only) — four targets in `crates/alpendns/fuzz/fuzz_targets/`: `parse_request`, `parse_blocklist`, `parse_config`, `parse_upstream_addr`.
- Test-only TLS/HTTP tooling: `rcgen` 0.14.7, `tokio-rustls` 0.26.4, `quinn` 0.11.11, `h2` 0.4.19, `http` 1.5.0, `time` 0.3.45 (`crates/alpendns/Cargo.toml` dev-dependencies).
- Integration tests run a real process on loopback with an in-process fake upstream; they never contact the internet (`crates/alpendns/tests/integration.rs`, `docs/TESTING.md` §4).

**Build/Dev:**
- `cargo fmt` (default settings), `cargo clippy --all-targets --all-features -- -D warnings`.
- `cargo-deny` with configuration in `deny.toml` (licences, advisories, sources, wildcards).
- `clippy.toml` permits `unwrap`/`expect`/`panic`/indexing in `#[cfg(test)]` code only.
- `cargo-deb` for the Debian package (`[package.metadata.deb]` in `crates/alpendns/Cargo.toml`); not a build dependency.
- Release profile: `lto = "thin"`, `codegen-units = 1`, `panic = "abort"`, `strip = "symbols"`. Dev profile: `debug = "line-tables-only"`.
- CI: `.github/workflows/ci.yml` (fmt, clippy, test, cargo-deny, 120 s per fuzz target).
- Release: `.github/workflows/release.yml` builds `.deb` for amd64 + arm64 and publishes a GitHub Release.
- Demo: `.github/workflows/pages.yml` builds `web/dist/alpendns-demo.html` via `web/demo/build.sh` and publishes it to GitHub Pages.

## Key Dependencies

**Critical:**
- `hickory-proto` 0.26.1 — DNS message model and validation; `dnssec-ring` pulls `time` into the production tree (RUSTSEC-2026-0009 exception documented in `deny.toml`).
- `hickory-net` 0.26.1 — DoT/DoH/DoQ clients and DNSSEC-validating handle.
- `tokio` 1.53.1 — sole async runtime.
- `axum` 0.8.9 — API/UI/metrics HTTP listeners.
- `rustls` 0.23.46 + `ring` 0.17.14 — all TLS; Mozilla roots compiled in via `webpki-roots` 1.0.9.
- `reqwest` 0.13.1 — HTTP(S) fetching of lists and ODoH messages.
- `odoh-rs` 1.0.4 + `hpke` 0.13.0 — Oblivious DoH wire format and HPKE (ADR-0017).
- `lru` 0.18.3, `arc-swap` 1.9.2 — cache eviction and lock-free state swaps.
- `psl` 2.1.226 — compiled-in Public Suffix List for `split_by_zone` (no runtime download).
- `jiff` 0.2.35 — time zones and schedules; deliberately not `time`.
- `idna` 1.1.0 — IDN/homograph handling in detection.
- `regex` 1.13.1 — policy regex rules, linear-time engine.

**Infrastructure:**
- `serde_json` 1.0.151 — API responses and `full` query log (JSON Lines).
- `async-stream` 0.3.6 — Server-Sent Events stream (`/api/events`).
- `humantime-serde` 1.1.1 — `"3s"`/`"24h"` duration syntax in config.
- `ipnet` 2.12.1 — client subnet matching.
- `futures-util` 0.3.34 — stream handling for DNS exchanges.
- `bytes` 1.12.1 — byte buffers.
- `rand` 0.10.2 (direct) and `rand` 0.9.5 (transitive) — query IDs, ODoH randomness, zone seed.

## Configuration

**Environment:**
- Configuration file passed with `-c <file>` (default is used by the systemd unit: `/etc/alpendns/alpendns.toml`). Files:
  - `config/alpendns.example.toml` — target-format reference, 392 lines.
  - `config/alpendns.minimal.toml` — minimal runnable configuration.
  - `config/local.dev.toml`, `config/local.test.toml` — local experiments (gitignored).
  - `packaging/alpendns.toml` — shipped Debian configuration (loopback only).
- Strict parsing: unknown keys are startup errors via `#[serde(deny_unknown_fields)]`.
- `alpendns check` validates configuration, paths and permissions before start; used as `ExecStartPre`.
- Only environment variable in use: `RUST_LOG` through `tracing_subscriber::EnvFilter` (`crates/alpendns/src/main.rs`). No `.env` file.
- API token file: `/etc/alpendns/api.token` (default, generated with owner-only permissions if absent).
- Blocklist cache: `/var/cache/alpendns/lists/`.
- Query log (only `privacy.logging.mode = "full"`): `/var/log/alpendns/queries.jsonl`.
- State directory: `/var/lib/alpendns/` (systemd `StateDirectory`).
- NRD feed (optional): `/var/lib/alpendns/nrd.txt`.

**Build:**
- `Cargo.toml` (workspace manifest, versions and lints), `Cargo.lock`.
- `rust-toolchain.toml`, `clippy.toml`, `deny.toml`.
- `crates/alpendns/Cargo.toml` (package, dev-dependencies, Debian metadata).
- `crates/alpendns/fuzz/Cargo.toml`.
- Packaging: `packaging/systemd/alpendns.service`, `packaging/debian/{postinst,prerm,postrm}`, `packaging/logrotate/alpendns`.
- Web demo build: `web/demo/build.sh` (shell only).

## Platform Requirements

**Development:**
- Linux with Rust stable >= 1.85 (`rust-toolchain.toml`), `rustfmt`, `clippy`.
- `cargo-deny` and `cargo-deb` installed separately for the definition of done / packaging.
- Nightly toolchain only for `cargo fuzz`.
- No system OpenSSL required: TLS is statically linked against `ring`/`rustls`.

**Production:**
- Debian 12/13 or Ubuntu 24.04+, systemd.
- Unprivileged `alpendns` user; `CAP_NET_BIND_SERVICE` for port 53.
- Deployment target: Debian package (`.deb`) for amd64 and arm64, published from GitHub Releases; unit hardened with `ProtectSystem=strict`, `SystemCallFilter`, `RestrictAddressFamilies`, etc.
- Plain binary can also run on non-privileged ports (5353) for smoke testing.

**Licence:** AGPL-3.0-or-later (`Cargo.toml`, `LICENSE`); `deny.toml` allowlist permits only AGPL-compatible licences.

---

*Stack analysis: 2026-10-20*
