---
last_mapped_commit: 630297fc35c98188292c5305d0e279aa6039b07a
last_mapped_at: 2026-10-20
---
# Testing Patterns

**Analysis Date:** 2026-10-20

## Test Framework

**Runner:**
- `cargo test` (standard test harness), stable toolchain from `rust-toolchain.toml`.
- No test configuration file: settings live in `.github/workflows/ci.yml` and `crates/alpendns/Cargo.toml`.
- Async tests use `#[tokio::test]` with the workspace tokio features (`rt-multi-thread`, `time`, `test-util`, `process`) declared in `Cargo.toml` and `crates/alpendns/Cargo.toml`.

**Assertion Library:**
- Standard library assertions (`assert!`, `assert_eq!`, `matches!`, `assert!(x.is_err())`). No `assert_cmd`, `predicates`, `insta` or snapshot library is used.
- Assertion messages are frequently German and explain the expected invariant, e.g. `expect("gültiger Name")` (`crates/alpendns/tests/integration.rs`), `"der Hinweis nennt {missing_path} nicht: {hint}"` (`crates/alpendns/src/clock.rs`).
- Property tests use `proptest` (`proptest::proptest!`, `proptest::prop_assert_eq!`), declared in `Cargo.toml`.

**Run Commands:**

```bash
cargo fmt --all --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test --all-features
cargo deny check
```

These four commands are the canonical definition of done (`docs/TESTING.md`, "Definition of Done"). CI runs the first three plus a separate `cargo deny` job (`.github/workflows/ci.yml`).

Focused runs during development:

```bash
cargo test -p alpendns name_of_the_test
cargo test -p alpendns --lib parser::
cargo test -p alpendns -- --nocapture
```

Ignored measurement suites:

```bash
cargo test --release --test load -- --ignored --nocapture --test-threads=1
cargo test --release --test detect_corpus -- --ignored measure --nocapture
```

## Test File Organization

**Location:**
- Unit tests are co-located in the module they test, in `#[cfg(test)] mod tests { use super::*; ... }`. This is done in 37 modules, including `crates/alpendns/src/cache.rs`, `crates/alpendns/src/config.rs`, `crates/alpendns/src/policy/mod.rs`, `crates/alpendns/src/detect/*.rs`, `crates/alpendns/src/api/ui.rs`, and even `crates/alpendns/src/main.rs`.
- Integration tests live in `crates/alpendns/tests/`, one file per feature area:
  - `integration.rs` — server pipeline over UDP/TCP on loopback
  - `blocklists.rs` — list parsing, loading, HTTP update
  - `policies.rs` — clients, policies, schedules, temporary grants
  - `local_zones.rs` — local-zone answers through the pipeline
  - `detection.rs` — heuristics through the pipeline and the trace
  - `encrypted.rs` — DoT/DoH/DoQ transports
  - `dnssec.rs` — signature validation vectors
  - `odoh.rs` — Oblivious DoH proxy/target chain
  - `ratelimit.rs` — per-client throttling against a real process
  - `observability.rs` — logging modes, API, history, live stream
  - `reload.rs` — SIGHUP config reload against the real binary
  - `language.rs` — output-language guard
  - `load.rs` — load measurement (`#[ignore]`)
  - `detect_corpus.rs` — corpus measurement and DGA training (`#[ignore]`)
- Fuzz targets are separate binaries in `crates/alpendns/fuzz/fuzz_targets/`: `parse_request.rs`, `parse_blocklist.rs`, `parse_config.rs`, `parse_upstream_addr.rs`.

**Naming:**
- Files: feature nouns in plural, or the subject under test (`blocklists.rs`, `observability.rs`, `dnssec.rs`).
- Test functions: full English sentences describing the expected behavior, not the method name: `a_broken_signature_is_bogus_and_is_dropped` (`crates/alpendns/tests/dnssec.rs`), `the_page_references_nothing_from_outside` (`crates/alpendns/src/api/ui.rs`), `rebinding_sees_the_answer_not_the_question` (`crates/alpendns/tests/detection.rs`).
- Ignored tests carry a reason string in the attribute: `#[ignore = "Lastmessung: maschinenabhängig, gehört nicht in die Definition of Done"]` (`crates/alpendns/tests/load.rs`), `#[ignore = "braucht einen Korpus"]` (`crates/alpendns/tests/detect_corpus.rs`).
- Helper functions are short nouns/verbs (`fn name`, `fn config`, `fn cache`, `fn key`, `fn answer`, `fn fake_upstream`, `fn start_server_with`, `fn ask_udp`).

**Structure:**

```text
crates/alpendns/
├── src/<module>.rs              # code + #[cfg(test)] mod tests at the bottom
├── tests/<feature>.rs           # integration crate, own crate root per file
├── fuzz/fuzz_targets/*.rs       # cargo-fuzz targets
├── fuzz/corpus/<target>/        # checked-in fuzz corpus
├── fuzz/known-crashes/          # documented panics that are not corpus (ADR-0006)
└── proptest-regressions/        # checked-in regression cases from proptest
```

## Test Structure

**Suite Organization:**

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::clock::TestClock;
    use hickory_proto::rr::Record;

    fn name(text: &str) -> Name {
        Name::from_ascii(text).expect("gültiger Name")
    }

    fn cache() -> Cache<Arc<TestClock>> {
        Cache::new(&config(), Arc::new(TestClock::new()))
    }

    #[test]
    fn entry_expires_and_is_served_stale_then_dropped() {
        let clock = Arc::new(TestClock::new());
        let cache = Cache::new(&config(), Arc::clone(&clock));
        cache.insert(key("example.com."), &answer("example.com.", 300));

        clock.advance(Duration::from_secs(301));
        let hit = cache.get(&key("example.com.")).expect("stale erwartet");
        assert!(hit.stale);
    }
}
```

Source: `crates/alpendns/src/cache.rs`.

**Patterns:**
- Setup is done by small factory functions at the top of the test module (`config()`, `cache()`, `key()`, `answer()`), not by a framework or `#[fixture]`.
- Teardown uses RAII: a `TempDir` struct with `Drop` removing the directory (`crates/alpendns/tests/observability.rs`, `crates/alpendns/tests/language.rs`), and `Running` values that own a `CancellationToken` in `crates/alpendns/tests/integration.rs`.
- Tests are written around observable invariants and counts, not internal calls: the fake upstream's `hits` counter must be exactly one after two identical queries, and exactly zero for a local-zone answer (`crates/alpendns/tests/integration.rs`, `crates/alpendns/tests/local_zones.rs`).
- Each integration test file defines its own helpers; there is no shared `tests/common/mod.rs`. Duplication is accepted to keep every file a self-contained crate root.

## Mocking

**Framework:** None. No `mockall`, `mockito`, `wiremock` or similar dependency.

**Patterns:**

```rust
// A complete in-process fake upstream; tests never leave loopback.
async fn fake_upstream<F>(delay: Duration, respond: F) -> Fake
where
    F: Fn(&Message) -> Option<Message> + Send + Sync + 'static,
{
    let socket = UdpSocket::bind("127.0.0.1:0").await.expect("bind");
    // ... spawn a loop, count hits, optionally stay silent
}
```

Source: `crates/alpendns/tests/integration.rs`. Similar hand-written fakes exist for:
- encrypted transports with self-signed certificates generated via `rcgen` (`crates/alpendns/tests/encrypted.rs`)
- an HTTP server for blocklist downloads (`crates/alpendns/tests/blocklists.rs`)
- an ODoH proxy and target (`crates/alpendns/tests/odoh.rs`)
- a signed zone with real cryptography for DNSSEC (`crates/alpendns/tests/dnssec.rs`)

**What to Mock:**
- Every external system: upstream resolvers, blocklist URLs, HTTP endpoints, TLS peers, the system clock, the wall clock, signals/processes where testable.
- Time is injected, not mocked: `TestClock` and `FixedWallClock` from `crates/alpendns/src/clock.rs` replace `SystemClock`/`SystemWallClock` in constructors. This is why TTL and schedule tests do not sleep.

**What NOT to Mock:**
- DNS message parsing/serialization is real `hickory-proto`; DNSSEC tests use real signatures and real crypto (`crates/alpendns/tests/dnssec.rs`).
- The pipeline itself is exercised through real server processes and real sockets (`crates/alpendns/tests/integration.rs`, `crates/alpendns/tests/ratelimit.rs`).
- Internal modules are never mocked; they are called directly from unit tests.

## Fixtures and Factories

**Test Data:**
- Inline builders in each test module: `fn query_message(qname, qtype)`, `fn answer_with_a(request)`, `fn name(text)`, `fn nxdomain(text, soa_ttl, minimum)` (`crates/alpendns/tests/integration.rs`, `crates/alpendns/src/cache.rs`).
- Config fixtures are written as inline TOML strings parsed with `toml::from_str` (`crates/alpendns/tests/integration.rs`) or as typed struct literals (`crates/alpendns/tests/observability.rs`).
- The secret-name fixture `SECRET: &str = "sehr-verraeterisch.example.com"` is used by the privacy leak test (`crates/alpendns/tests/observability.rs`).
- Test-only randomness is deterministic where the test needs it: `RandomState` in `TempDir::new` only separates parallel temp directories; `FixedWallClock` pins calendar time.

**Location:**
- Fixtures are local to the test module or integration file. There is no shared fixture directory.
- Persistent regression artifacts: `crates/alpendns/proptest-regressions/filter/matcher.txt`, `crates/alpendns/fuzz/corpus/<target>/`, and documented known crashes in `crates/alpendns/fuzz/known-crashes/`.
- Corpus data for measurement runs is intentionally absent from the repo; `corpus/` is gitignored and set up as described in `docs/TESTING.md` §6 and `crates/alpendns/tests/detect_corpus.rs`.

## Coverage

**Requirements:** No numeric coverage target; no coverage tool is configured (`tarpaulin`, `llvm-cov`, Codecov are absent from `.github/`, `scripts/` and docs). The enforced bar is the definition of done plus the rule that every change brings a test or a justification (`docs/TESTING.md`).

**View Coverage:**

```bash

# Not configured in this repository. The analogous measure is the test inventory:

cargo test --all-features -- --list
```

Current inventory at this analysis: 457 `#[test]`/`#[tokio::test]` functions in `crates/alpendns/src/` and 154 in `crates/alpendns/tests/` (611 total), plus property tests, four fuzz targets and the ignored measurement suites.

## Test Types

**Unit Tests:**
- Live beside the code in `#[cfg(test)] mod tests`. Scope: parsers (blocklist formats, config), TTL/cache logic, matcher invariants, upstream selection, heuristic scoring, trace construction, CLI helpers.
- Parser tests must include broken, truncated and malicious input in addition to the happy path (`docs/TESTING.md` §1; see `crates/alpendns/src/filter/parser.rs`, `crates/alpendns/src/config.rs`).

**Property Tests:**
- `proptest` for invariants over all inputs: wildcard matching equals label-suffix matching (`crates/alpendns/src/filter/matcher.rs`), TTL never increases (`crates/alpendns/src/cache.rs`), 0x20 randomization round-trips and name normalization is idempotent (`crates/alpendns/src/privacy.rs`).
- Failures are persisted automatically to `crates/alpendns/proptest-regressions/`.

**Fuzz Tests:**
- `cargo-fuzz` (nightly only, the project's sole nightly exception). Targets: `parse_request`, `parse_blocklist`, `parse_config`, `parse_upstream_addr` (`crates/alpendns/fuzz/fuzz_targets/`).
- CI runs each target for 120 s on the checked-in corpus with `-O`, because overflow checks trigger the known `hickory-proto` TSIG panic documented in ADR-0006 (`.github/workflows/ci.yml`).
- Success criteria: no panic, no endless loop, no unbounded growth (`docs/TESTING.md` §3).

**Integration Tests:**
- Real AlpenDNS server on a loopback port, real UDP/TCP sockets, fake upstream in the same process (`crates/alpendns/tests/integration.rs`, `ratelimit.rs`, `local_zones.rs`, `detection.rs`).
- Process-level tests spawn the actual binary: SIGHUP reload (`crates/alpendns/tests/reload.rs`) and the English-output guard (`crates/alpendns/tests/language.rs`).
- Encrypted-transport tests run DoT, DoH and DoQ against in-process TLS/QUIC fakes with `rcgen` certificates (`crates/alpendns/tests/encrypted.rs`), and ODoH end-to-end through a dumb proxy (`crates/alpendns/tests/odoh.rs`).
- Rule: tests never contact real upstream resolvers or real blocklist URLs. `docs/TESTING.md` §4 makes this explicit; every integration file repeats it in its module doc.

**E2E / Measurement:**
- No browser E2E suite. The web UI is verified by CSS/HTML assertions in `crates/alpendns/src/api/ui.rs` (no external references, semantic colors only, contrast computation, shared grid tracks).
- Load measurement is `#[ignore]`d and machine-dependent (`crates/alpendns/tests/load.rs`); corpus measurement and DGA training are `#[ignore]`d and require external corpora (`crates/alpendns/tests/detect_corpus.rs`). Neither belongs to the definition of done.

## Common Patterns

**Async Testing:**

```rust
#[tokio::test]
async fn a_second_query_is_answered_from_the_cache() {
    let fake = fake_upstream(Duration::ZERO, answer_with_a).await;
    let server = start_server(fake.addr, Duration::from_secs(2)).await;
    // ask twice over a real socket ...
    assert_eq!(fake.hits.load(Ordering::SeqCst), 1);
    server.shutdown.cancel();
}
```

Source pattern: `crates/alpendns/tests/integration.rs`.

**Waiting for Background Work:**
- Poll for the event instead of sleeping a fixed time: `wait_until("prefetch", || ...)` polls every 5 ms up to 2 s and panics with a diagnostic message (`crates/alpendns/tests/integration.rs`).
- Network reads are wrapped in `tokio::time::timeout` (14 uses under `crates/alpendns/tests/`), so a hung fake fails the test instead of blocking CI.

**Error Testing:**

```rust
let result = backend.resolve(&request, &mut ctx).await;
assert!(matches!(result, Err(ResolveError::Timeout)));
```

Pattern used across `crates/alpendns/tests/encrypted.rs`, `crates/alpendns/tests/policies.rs` and the unit tests in `crates/alpendns/src/config.rs`.

**Guard Tests (project-specific):**
- Privacy: `no_query_name_leaves_the_process_in_the_quiet_modes` aggregates every output surface (stats, top domains, recent log, API JSON) and asserts the secret name is absent (`crates/alpendns/tests/observability.rs`).
- Language: `the_command_line_speaks_english` and `the_running_process_speaks_english` scan CLI output, log lines and Prometheus HELP text against a German-word list (`crates/alpendns/tests/language.rs`).
- UI: tests assert the absence of external URLs, the four semantic colors, the 8-based spacing scale and the single 1400 px container (`crates/alpendns/src/api/ui.rs`).

**CI Wiring:**
- `.github/workflows/ci.yml` has three jobs: `check` (`fmt`, `clippy`, `test`), `deny` (cargo-deny, opens/comments a single advisory issue on main), and `fuzz` (four targets, 120 s each, nightly).
- `RUSTFLAGS: "-D warnings"` is global, so clippy warnings are errors in CI even when they are only `warn` in `Cargo.toml`.
- CI runs on push to `main`, every pull request, and weekly (`cron: '17 6 * * 1'`) because the advisory database changes without a commit.

---

*Testing analysis: 2026-10-20*
