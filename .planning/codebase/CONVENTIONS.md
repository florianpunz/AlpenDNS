---
last_mapped_commit: 630297fc35c98188292c5305d0e279aa6039b07a
last_mapped_at: 2026-10-20
---
# Coding Conventions

**Analysis Date:** 2026-10-20

## Naming Patterns

**Files:**
- Modules are lowercase `snake_case`, one concept per file: `crates/alpendns/src/logging/mod.rs`, `crates/alpendns/src/filter/matcher.rs`, `crates/alpendns/src/upstream/pool.rs`.
- A directory module uses `mod.rs` (`crates/alpendns/src/api/mod.rs`, `crates/alpendns/src/detect/mod.rs`, `crates/alpendns/src/policy/mod.rs`).
- Integration tests are named after the feature area, usually plural, in `crates/alpendns/tests/`: `blocklists.rs`, `policies.rs`, `local_zones.rs`, `observability.rs`, `ratelimit.rs`.
- Test-only files understood by tooling keep tool names: `crates/alpendns/proptest-regressions/filter/matcher.txt`, `crates/alpendns/fuzz/fuzz_targets/parse_request.rs`.

**Functions:**
- `snake_case`, verb-first for actions: `Config::load`, `validate`, `lookup`, `resolve`, `build_zones`, `install_crypto_provider`.
- Constructors are `new`; variants that expose more knobs use `with_*`: `Cache::new` (`crates/alpendns/src/cache.rs`), `Pool::with_seed` (`crates/alpendns/src/upstream/pool.rs`), `start_server_with` (`crates/alpendns/tests/integration.rs`).
- Test names are complete English sentences in `snake_case` describing the invariant, not the function under test: `entry_expires_and_is_served_stale_then_dropped` (`crates/alpendns/src/cache.rs`), `no_query_name_leaves_the_process_in_the_quiet_modes` (`crates/alpendns/tests/observability.rs`), `a_detector_that_blocks_names_itself_as_the_reason` (`crates/alpendns/tests/detection.rs`).

**Variables:**
- Short, domain-specific, `snake_case`: `ctx`, `peer`, `req`, `response`, `cache`, `clock`, `limiter`.
- Names/identifiers travelling through the trace are `Arc<str>`, not `String` IDs: `crates/alpendns/src/trace.rs`.
- Config values mirror the TOML key names exactly, using `snake_case`, so the format is grep-able across code and `config/alpendns.example.toml` (`crates/alpendns/src/config.rs`).

**Types:**
- `PascalCase`. Config structs carry the `Config` suffix (`ServerConfig`, `CacheConfig`, `DetectionConfig`, `DgaConfig`); errors carry `Error` (`ConfigError`, `ResolveError`, `LoadError`); traits are bare nouns or verb phrases (`Clock`, `WallClock`, `ResolveBackend`, `NameDetector`, `AnswerDetector`).
- Enums with alternatives that are data-carrying use struct variants: `Step::ClientMatched { client, by }`, `Step::UpstreamUsed { resolver, rtt }` (`crates/alpendns/src/trace.rs`).

**Constants:**
- `SCREAMING_SNAKE_CASE`, often with units or meaning in the name: `STALE_TTL`, `SHARDS`, `FAILURE_TTL_MAX`, `MAX_LIST_BYTES`, `PADDING_BLOCK`, `TIME_SYNC_MARKER` (`crates/alpendns/src/cache.rs`, `crates/alpendns/src/ratelimit.rs`, `crates/alpendns/src/filter/source.rs`, `crates/alpendns/src/privacy.rs`, `crates/alpendns/src/main.rs`).

## Code Style

**Formatting:**
- `cargo fmt` with default settings. There is no `rustfmt.toml`; the canonical command is `cargo fmt --all --check` in `.github/workflows/ci.yml`.
- Edition 2024, stable toolchain pinned in `rust-toolchain.toml`, `rust-version = "1.85"` in `Cargo.toml`.
- This is a single-crate workspace today (`members = ["crates/alpendns"]`); the module layout in `crates/alpendns/src/lib.rs` mirrors the target crate split from CLAUDE.md B.3.

**Linting:**
- Workspace lints in `Cargo.toml`: `unsafe_code = "forbid"`, `unwrap_used = "deny"`, `panic = "deny"`, `indexing_slicing = "deny"`, `expect_used = "warn"`, `integer_division = "warn"`, `todo = "warn"`, `missing_debug_implementations = "warn"`.
- CI makes warnings fatal: `RUSTFLAGS: "-D warnings"` plus `cargo clippy --all-targets --all-features` (`.github/workflows/ci.yml`), so `expect_used` is fatal there although only a warning locally.
- `clippy.toml` permits `unwrap`, `expect`, `panic` and indexing inside `#[cfg(test)]` modules only.
- Integration tests are a separate crate and must opt in explicitly at the top of the file, e.g. `crates/alpendns/tests/integration.rs`:
  ```rust
  #![allow(clippy::expect_used, clippy::indexing_slicing, clippy::panic)]
  ```
  `crates/alpendns/tests/observability.rs` additionally allows `clippy::integer_division`.

**Language policy (verified in code and docs):**
- Identifiers, error strings, log messages and program output are English. Examples: `ResolveError` messages in `crates/alpendns/src/resolve.rs`, log lines in `crates/alpendns/src/server/mod.rs`, CLI output in `crates/alpendns/src/main.rs`.
- Source comments, module docs, test assertion messages and commit messages are German. Examples: module docs in `crates/alpendns/src/config.rs`, assertion text `expect("gültiger Name")` in tests.
- The policy is stated in `docs/OPERATIONS.md` ("Prose and program output are English; only the code comments and the commit messages are still German") and enforced for output by `crates/alpendns/tests/language.rs`.
- Commit history uses Conventional Commits with German bodies (`git log`: `feat(config): let a more specific zone win across local and forward`).

## Import Organization

**Order:**
1. Standard library (`std::...`), including `std::sync::atomic::{...}`.
2. External crates (`hickory_proto`, `tokio`, `rustls`, `jiff`, ...).
3. Crate-internal paths: `crate::...` in library code, `alpendns::...` in integration tests.
4. `use ... as _;` only for trait-method availability, e.g. `use alpendns::resolve::ResolveBackend as _;` in `crates/alpendns/tests/encrypted.rs`.

Groups are separated by blank lines; within a group rustfmt's default alphabetical ordering applies. This pattern is consistent across `crates/alpendns/src/server/mod.rs`, `crates/alpendns/tests/integration.rs` and `crates/alpendns/tests/observability.rs`.

**Path Aliases:**
- None. Modules are referenced absolutely (`crate::dnssec`, `crate::logging::Flagged`, `alpendns::clock::TestClock`). There is no `prelude` module.

## Error Handling

**Patterns:**
- Library errors are hand-written `thiserror` enums, one per module concern: `ConfigError` (`crates/alpendns/src/config.rs`), `ResolveError` (`crates/alpendns/src/resolve.rs`), `filter::source::LoadError` (`crates/alpendns/src/filter/source.rs`), `detect::nrd::LoadError` (`crates/alpendns/src/detect/nrd.rs`), policy errors in `crates/alpendns/src/policy/rules.rs` and `crates/alpendns/src/policy/mod.rs`.
- Error text in `#[error(...)]` is English and explains the consequence, not just the cause: `"No upstream in the pool could answer"`, `"DNSSEC validation failed, the answer was dropped"` (`crates/alpendns/src/resolve.rs`).
- Cause chains use `#[from]` for trivial wrapping and `#[source]` for explicit context; large sources are boxed (`source: Box<toml::de::Error>` in `crates/alpendns/src/config.rs`) to satisfy `result_large_err`.
- `anyhow` is used only in the binary: `fn main() -> ExitCode`, `fn parse_args() -> anyhow::Result<Args>` in `crates/alpendns/src/main.rs`. Library code never returns `anyhow::Error`.
- No `unwrap()`/`expect()`/`panic!()`/`unsafe` on production paths. Fallible operations return `Result` or `Option` and are handled with `match`, `let ... else`, `unwrap_or_else` or `unwrap_or_default`; e.g. `dns::format_error` returns `Option<Message>` (`crates/alpendns/src/dns.rs`), and `build_zones(...).unwrap_or_default()` deliberately falls back to the safe empty list (`crates/alpendns/src/config.rs`).
- Resolution failures are translated once at the edge into DNS errors: `handle_request` logs `tracing::warn!(%error, "resolution failed")` and returns `ServFail` (`crates/alpendns/src/server/mod.rs`).
- Config validation is separated from parsing: `Config::load` parses with `serde(deny_unknown_fields)` then calls `fn validate(&self) -> Result<(), ConfigError>` (`crates/alpendns/src/config.rs`). A broken config is a startup error, never a warning.

## Logging

**Framework:** `tracing` with structured fields.

**Patterns:**
- Always structured, never interpolated into the message: `tracing::debug!(%peer, "query dropped for exceeding the rate limit")`, `tracing::warn!(%error, "resolution failed")`, `tracing::info!(seconds = ttl.as_secs(), "temporary grant issued")` (`crates/alpendns/src/server/mod.rs`, `crates/alpendns/src/api/mod.rs`).
- Messages are English, lowercase, no trailing period: `"stopped"`, `"query not decodable"`, `"NRD list loaded"` (`crates/alpendns/src/main.rs`, `crates/alpendns/src/server/mod.rs`, `crates/alpendns/src/detect/nrd.rs`).
- Level usage: `warn`/`error` for operational failures, `info` for lifecycle and configuration events, `debug` for per-request events that must not be noisy in normal operation.
- `println!`/`eprintln!` are allowed only for CLI output in `crates/alpendns/src/main.rs` (`check`, `paths`, status report); never in library code.
- Query names may only reach the log through `logging::QueryLog`; `server::handle_request` hands the `Trace` to it exactly once (`crates/alpendns/src/server/mod.rs`). Direct logging of a name elsewhere violates CLAUDE.md B.1 rule 3 and is caught by `crates/alpendns/tests/observability.rs`.

## Comments

**When to Comment:**
- Comments explain *why*, especially where the code looks arbitrary: rate-limit drops instead of refusals (`crates/alpendns/src/server/mod.rs`), why the cache sits behind policy (`crates/alpendns/src/policy/mod.rs`), why a field is boxed (`crates/alpendns/src/config.rs`).
- Module headers are `//!` and state the responsibility, the invariant and the ADR/document reference, e.g. `crates/alpendns/src/trace.rs`, `crates/alpendns/src/resolve.rs`, `crates/alpendns/src/clock.rs`.
- Non-obvious test constructions are documented with the invariant they pin, often citing the source rule: `crates/alpendns/src/filter/matcher.rs`, `crates/alpendns/tests/dnssec.rs`, `crates/alpendns/src/api/ui.rs`.
- Historical rationale is kept deliberately as part of the design record (for example the fan-out removal note in `crates/alpendns/src/trace.rs`), because it references the ADR that explains the current shape.

**JSDoc/TSDoc:**
- Rustdoc `///` on public items; the convention is a one-line summary followed by a rationale paragraph where the *why* is not obvious (`crates/alpendns/src/clock.rs`, `crates/alpendns/src/resolve.rs`).
- Cross-references use rustdoc links: `[`Clock`]`, `[`crate::local`]`, `[ADR-0012](../../../docs/adr/0012-fanout-entfaellt.md)`.
- `missing_docs` is not denied by lint; public documentation is maintained by convention across the library modules in `crates/alpendns/src/lib.rs`.

## Function Design

**Size:** Functions are kept focused; complex paths are decomposed into named helpers (`build_event` in `crates/alpendns/src/server/mod.rs`, `split_by_zone` in `crates/alpendns/src/upstream/pool.rs`). Large configuration files are long because the data model is long, not because functions are.

**Parameters:**
- Dependencies that tests must control are passed in, not looked up: clocks as `Arc<TestClock>` / `Arc<FixedWallClock>` (`crates/alpendns/src/clock.rs`), backends as generic `B: ResolveBackend` (`crates/alpendns/src/server/mod.rs`).
- `&mut Ctx` is threaded explicitly through resolution instead of hidden in a lock (`crates/alpendns/src/resolve.rs`, `crates/alpendns/src/trace.rs`).

**Return Values:**
- Pure helpers are annotated `#[must_use]` where ignoring the value is a bug (`crates/alpendns/src/dns.rs`, `crates/alpendns/src/config.rs`).
- Async trait methods return `impl Future<Output = ...> + Send` rather than using `async fn` in traits, so the future can be spawned: `crates/alpendns/src/resolve.rs`. Blanket impls forward for `Arc<B>`.
- Testable public types that integration tests need live in the library even though only tests construct them, e.g. `TestClock` (`crates/alpendns/src/clock.rs`, with a comment explaining why it is public).

## Module Design

**Exports:**
- `crates/alpendns/src/lib.rs` declares the public modules; `crates/alpendns/src/main.rs` contains only startup, config loading, signals, shutdown and CLI reporting. No business logic in the binary.
- Internal helpers are `pub(crate)` or private: `pub(crate) async fn handle_request` (`crates/alpendns/src/server/mod.rs`), `fn validate` (`crates/alpendns/src/config.rs`).
- Types are imported by full module path; there are no broad re-export facades beyond `pub mod` declarations.

**Barrel Files:**
- `mod.rs` files re-export the directory's own types only; they do not aggregate unrelated modules. Compare `crates/alpendns/src/filter/mod.rs`, `crates/alpendns/src/logging/mod.rs`, `crates/alpendns/src/upstream/mod.rs`.

## Project-Specific Conventions

- **Rarely changing state uses `ArcSwap`, never a global mutex in the request path:** `PolicyBackend` (`crates/alpendns/src/policy/mod.rs`) and `reload::PolicySource` (`crates/alpendns/src/reload.rs`). Comments on the fields state this explicitly.
- **Clocks are injectable:** no `SystemTime::now()` or `Instant::now()` in cache or policy; use `Clock`/`WallClock` (`crates/alpendns/src/clock.rs`). Test doubles are `TestClock` and `FixedWallClock`.
- **The decision trace is always produced:** matchers return evidence (e.g. list + line + matched name), not `bool`; see `Step` in `crates/alpendns/src/trace.rs` and the policy engine in `crates/alpendns/src/policy/mod.rs`.
- **Config structures mirror the documented target format** and use `#[serde(deny_unknown_fields)]` plus `#[serde(default)]` for optional tables (`crates/alpendns/src/config.rs`). New keys belong in `config/alpendns.example.toml` first.
- **No `unsafe`, no runtime other than tokio, no DNS parsing by hand:** DNS wire format comes from `hickory-proto` (`Cargo.toml`, `crates/alpendns/src/dns.rs`).
- **Dependencies are workspace-declared with a one-line justification comment** in `Cargo.toml`; do not add a crate version directly to `crates/alpendns/Cargo.toml`.

---

*Convention analysis: 2026-10-20*
