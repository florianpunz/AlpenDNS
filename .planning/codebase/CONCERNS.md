---
last_mapped_commit: 630297fc35c98188292c5305d0e279aa6039b07a
last_mapped_at: 2026-10-20
---
# Codebase Concerns

**Analysis Date:** 2026-10-20

## Tech Debt

**Example configuration drifts from the parser and does not load:**
- Issue: `config/alpendns.example.toml` is declared the "specification of the target format", is shipped in the `.deb` under `/usr/share/doc/alpendns/`, and contains keys the parser rejects. `server.max_inflight` (line 15), `server.edns.padding` (line 23) and `match = { doh_token = ... }` (line 300) are absent from `crates/alpendns/src/config.rs`. `ServerConfig` (`config.rs:635`) has no `max_inflight`, `EdnsConfig` (`config.rs:692`) has no `padding`, `ClientMatch` (`config.rs:382`) only has `ip`. Verified by starting the built binary with the file: it aborts at line 300 with `unknown field 'doh_token'`.
- Files: `config/alpendns.example.toml`, `crates/alpendns/src/config.rs`
- Impact: `serde(deny_unknown_fields)` makes the documented example unusable as a starting point; a user copying it gets a startup failure. This directly contradicts hard rule B.1.5 ("server does not start with broken configuration") in spirit — the configuration *is* rejected, but the shipped documentation promises features that do not exist.
- Fix approach: mark every unimplemented key `[PHASE n]`/comment it out, and add a test that parses the shipped example (see Test Coverage Gaps).

**Per-list `refresh` is parsed but ignored:**
- Issue: `ListConfig.refresh` (`crates/alpendns/src/config.rs:450`) is accepted, but `ListSpec` in `crates/alpendns/src/filter/source.rs` carries only `name`, `source` and `format`. `main.rs:340-347` computes one global interval as the minimum over all enabled lists, and `policy::run_updater` (`crates/alpendns/src/policy/mod.rs:572-611`) reloads *all* lists with one `tokio::time::interval`. A per-list `refresh` therefore has no effect.
- Files: `crates/alpendns/src/config.rs:441-460`, `crates/alpendns/src/filter/source.rs`, `crates/alpendns/src/main.rs:340-347`, `crates/alpendns/src/policy/mod.rs:572-611`
- Impact: the configuration suggests per-list scheduling that does not exist; one fast list forces every list to be re-fetched at that cadence (or one slow list delays the rest). Silent misconfiguration, not an error.
- Fix approach: either implement per-list scheduling (one task/interval per spec) or reject a per-list `refresh` that differs from the global one with a clear message.

**`refresh = "0s"` is accepted and panics the updater:**
- Issue: no validation of `ListConfig.refresh` in `Config::validate` (`crates/alpendns/src/config.rs`). `run_updater` passes `interval` straight to `tokio::time::interval` (`policy/mod.rs:579`), and tokio asserts `period > Duration::ZERO` (`tokio-1.53.1/src/time/interval.rs:74`). Confirmed by reading the vendored registry source.
- Files: `crates/alpendns/src/policy/mod.rs:572-611`, `crates/alpendns/src/config.rs`
- Impact: with the release profile (`panic = "abort"`, `Cargo.toml`) the process aborts at startup; in debug the spawned updater task dies and lists never refresh again. Requires an admin typo, but B.1.5 says broken configuration must not start at all.
- Fix approach: reject `refresh == 0` in validation with a message, or treat `0` as "never refresh" the way `seed_rotation` does (`pool.rs:352`).

**List reload swaps two `ArcSwap`s non-atomically:**
- Issue: `reload::PolicySource::reload` (`crates/alpendns/src/reload.rs:46-52`) stores the new blueprint first and the new lists second. A query between the two stores can see the new policy referencing a list that is not in the old `ArcSwap`. `Blueprint::build` returns `UnknownList` for a missing list (`policy/mod.rs:423`), and the documented contract is "failure leaves the old state", which is only true per-swap, not across both.
- Files: `crates/alpendns/src/reload.rs`, `crates/alpendns/src/policy/mod.rs`
- Impact: a short window after SIGHUP/reload in which policy evaluation can error or fall back. Low probability, hard to reproduce, and it undercuts the fail-closed guarantee.
- Fix approach: bundle blueprint and lists into one `Arc<PolicyState>` behind a single `ArcSwap` so readers take one consistent snapshot.

**Runtime status reports the startup list set:**
- Issue: `StatusSource::lists` in `crates/alpendns/src/main.rs` is populated from the startup `self.lists`, while `run_updater` replaces lists inside the policy source at runtime. List ages shown by `/api/lists` and the CLI are derived from the startup set with fresh ages, not from the live source.
- Files: `crates/alpendns/src/main.rs`, `crates/alpendns/src/api/mod.rs`, `crates/alpendns/src/policy/mod.rs`
- Impact: state and troubleshooting view can disagree with what the resolver actually enforces, especially after a runtime download failure where old rules are kept.
- Fix approach: feed the status source from the same `ArcSwap`/update path that the resolver uses.

**Very large single modules:**
- Issue: `crates/alpendns/src/config.rs` is 2,289 lines, `crates/alpendns/src/main.rs` is 1,464 lines, and the target crate split in `CLAUDE.md` B.3 (`alpendns-server`, `-cache`, `-upstream`, `-filter`, `-policy`, `-detect`) is not yet done — everything lives in `crates/alpendns/src/`.
- Files: `crates/alpendns/src/config.rs`, `crates/alpendns/src/main.rs`, `crates/alpendns/src/`
- Impact: review and navigation cost; hand-rolled validation in `config.rs` is where the example-config and `refresh` gaps hide. The split is explicitly deferred by B.3 ("crates only when the phase arrives"), so this is accepted debt, not an oversight.
- Fix approach: no split before its phase; extract validation helpers/tests from `config.rs` first.

**Measurement corpora and fuzz seeds are not in the repository:**
- Issue: CLAUDE.md says each fuzz target runs 120 s in CI "on the checked-in corpus", but `.gitignore` ignores `/crates/*/fuzz/corpus/` and there is no `fuzz/seeds/` directory (the `.gitignore` comment references one). Detector thresholds live in the modules, but the corpora used to measure them are also absent; measurement tests are `--ignored`.
- Files: `.gitignore`, `crates/alpendns/fuzz/`, `crates/alpendns/fuzz/fuzz_targets/`, `docs/TESTING.md` §6
- Impact: fuzzing starts from a weaker or empty corpus in CI, and the detector thresholds cannot be reproduced or challenged by a third party.
- Fix approach: commit the curated seeds/corpus (they are small text files) and document the measurement corpora location or add a download script (no network in tests).

**Temporary grants are deliberately volatile:**
- Issue: `crates/alpendns/src/policy/temporary.rs` keeps grants/denials in a `Mutex<HashMap<String, Instant>>` and does not persist them.
- Files: `crates/alpendns/src/policy/temporary.rs`
- Impact: a restart or reload drops every temporary allow/deny. Documented and intentional; listed here so operators are not surprised.
- Fix approach: none unless persistence becomes a requirement; would reintroduce state that must be cleaned up on expiry.

## Known Bugs

**False `Bogus` on NODATA after a CNAME into another zone (ADR-0028):**
- Symptoms: valid answers are turned into SERVFAIL; about 1 % of queries, 2,144 failures over 8 days in the field.
- Files: `docs/adr/0028-falsches-bogus-hickory-cname.md`, `crates/alpendns/src/dnssec.rs`, dependency `hickory-proto`/`hickory-net` 0.26.1
- Trigger: a CNAME chain crosses a zone boundary and the final name has no data (NODATA). hickory's validator returns `Bogus` instead of `NoError`/NODATA.
- Workaround: none in-process; the fix exists only in hickory PR #3595 and no stable release carries it. Accepted as a known defect.
- Impact: intermittent SERVFAILs for otherwise healthy names; hard to distinguish from real DNSSEC failures in the logs.

**TSIG parsing panic in `hickory-proto` 0.26.1 with overflow checks on:**
- Symptoms: arithmetic overflow panic on a malformed TSIG record (`RDLENGTH` underflow) when integer overflow checks are enabled.
- Files: `crates/alpendns/fuzz/known-crashes/README.md`, `crates/alpendns/fuzz/known-crashes/`, `docs/adr/0006-*.md`
- Trigger: fuzzing or a debug build receiving the crafted record. The release profile disables overflow checks, so the shipped binary is not affected.
- Workaround: documented in the known-crashes directory; upstream fix in 0.27 alpha only.
- Impact: this is the one open deviation from B.1 rule 1 (no panics on network data). It constrains build profiles and keeps a known crash in the tree.

**Multi-question queries are accepted and silently truncated:**
- Issue: `server::handle_request` rejects only `request.queries.is_empty()` (`crates/alpendns/src/server/mod.rs:193`) and rejects non-`Query` opcodes; it never rejects `QDCOUNT > 1`. `build_event` and the pipeline use `request.queries.first()`.
- Files: `crates/alpendns/src/server/mod.rs:193-198`, `crates/alpendns/src/server/mod.rs:232`
- Impact: a request with several questions is processed as if it had one; extra questions are ignored. This is at best a protocol-precision gap and at worst an inconsistency between what is answered and what was asked (also a cache-key question). No crash.
- Fix approach: return `FormErr` for `queries.len() != 1`, and add a test with QDCOUNT=2.

**Forward targets are not checked to be on the LAN:**
- Issue: `ForwardZone` (`crates/alpendns/src/config.rs:775`) holds a single `upstream`, and validation only rejects exact local/forward duplicates. It does not verify that a cleartext `forward_zone` target is a private/LAN address, although B.1 rule 7 permits cleartext only "into your own LAN". ADR-0029 also notes a forwarding loop (DC → AlpenDNS → DC) is possible.
- Files: `crates/alpendns/src/config.rs`, `docs/adr/0029-nested-forward-zone.md`
- Impact: a URL such as `udp://8.8.8.8` in a `forward_zone` would send cleartext DNS to the public internet, contrary to the stated security rule. Also, a misconfigured loop can generate unbounded upstream traffic.
- Fix approach: validate that `forward_zone` upstreams resolve to loopback/private/link-local addresses (and document the loop risk), or require an explicit opt-in flag.

## Security Considerations

**Web UI responses carry no security headers:**
- Risk: the API and UI set only `Content-Type` and `Cache-Control: no-cache` (`crates/alpendns/src/api/ui.rs:29-32`). There is no `Content-Security-Policy`, `X-Content-Type-Options: nosniff`, `X-Frame-Options`/`frame-ancestors`, or `Referrer-Policy`.
- Files: `crates/alpendns/src/api/ui.rs`, `crates/alpendns/src/api/mod.rs`
- Current mitigation: the UI uses `textContent` (no `innerHTML`) and embeds assets itself; by default the listeners are loopback (`packaging/alpendns.toml`).
- Recommendations: add a strict CSP (scripts/styles are inline, so use hashes or a nonce), `nosniff`, `frame-ancestors 'none'`, and `Referrer-Policy: no-referrer`. Extend the existing UI tests to assert the header set.

**Bearer token is stored in `localStorage` and passed as a query parameter:**
- Risk: `web/app.js` stores `alpendns.token` in `localStorage` and opens `EventSource('/api/events?token=...')`. `api::auth` (`crates/alpendns/src/api/mod.rs`) also accepts `?token=`. Query strings can end up in access logs, shell history or referrers; `localStorage` is readable by any XSS.
- Files: `web/app.js`, `crates/alpendns/src/api/mod.rs`
- Current mitigation: no external resources, no `innerHTML`, CSP absent but UI is same-origin and offline; tokens are per-client grants capped at 24 h (`MAX_GRANT`).
- Recommendations: use a cookie with `HttpOnly`/`SameSite=Strict`, or at least keep the token out of the URL by using a header for SSE via a fetch-based stream. Add `Referrer-Policy` regardless.

**Blocklist downloads follow redirects without restriction:**
- Risk: `reqwest::Client::builder().timeout(DOWNLOAD_TIMEOUT)` in `crates/alpendns/src/filter/source.rs:163` uses the default redirect policy. A configured list URL can redirect to an arbitrary host, and the final URL is not checked. This is admin-controlled, so the honest risk is a compromised list host redirecting to an internal service (SSRF) or serving oversized/changed content.
- Files: `crates/alpendns/src/filter/source.rs`
- Current mitigation: `MAX_LIST_BYTES = 64 MiB` via `read_limited`, per-list URL is explicit, HTTPS expected.
- Recommendations: limit redirects (e.g., 3) and record/log the final host; consider pinning the scheme to `https` for non-local sources.

**`time` carries an open RUSTSEC advisory in the production tree:**
- Risk: RUSTSEC-2026-0009 (stack exhaustion parsing RFC-2822 dates) applies to `time 0.3.45`, which is in the shipped binary through `hickory-proto/dnssec-ring` → `dep:time`.
- Files: `deny.toml:15-35`, `Cargo.toml`, `docs/adr/`
- Current mitigation: a documented, narrower exception — the vulnerable parse path is not reachable from AlpenDNS; `cargo deny` runs in CI.
- Recommendations: remove the exception when MSRV reaches 1.88 and `time` can be updated; keep `cargo deny` on a schedule so a new advisory is not masked by the existing exception.

**`expect()` is only fatal in CI:**
- Risk: `expect_used = "warn"` locally, fatal only through `RUSTFLAGS="-D warnings"` in CI. A locally green build can contain an `expect()` on a network path.
- Files: `Cargo.toml`, `clippy.toml`, `.github/workflows/`
- Current mitigation: CI denies warnings; the only production `unwrap()` is `ratelimit.rs:210` on `shards.first()` with `#[expect]` and an unreachable fallback.
- Recommendations: none structural; keep the CI flags in sync and avoid relaxing them.

**Cleartext client-facing DNS is the only client transport:**
- Risk: UDP/TCP on port 53; DoT/DoH/DoQ listeners for *clients* are not implemented (roadmap phase 10). On an untrusted LAN, queries and answers are visible.
- Files: `crates/alpendns/src/server/udp.rs`, `crates/alpendns/src/server/tcp.rs`, `docs/ROADMAP.md`
- Current mitigation: the shipping config listens on loopback only; encrypted upstreams are mandatory for non-`forward_zone`.
- Recommendations: document the LAN trust assumption prominently; do not expose port 53 beyond the LAN before client encryption exists.

## Performance Bottlenecks

**Logging takes global mutexes on every request:**
- Problem: in `aggregate` mode `QueryLog::record` locks `counts: Mutex<Counts>` for every query (`crates/alpendns/src/logging/mod.rs:373,464-485`); `reportable` and `ring` are also global mutexes (`logging/mod.rs:376-377`). At the measured 84k q/s this is the main serialization point.
- Files: `crates/alpendns/src/logging/mod.rs`, `crates/alpendns/src/logging/counts.rs`
- Cause: one shared counter map; `Counts` is bounded at `MAX_TRACKED = 200_000`, but the lock itself is per request.
- Improvement path: shard the counters by hash (as `ratelimit` does), or batch via per-core counters merged by the flusher. Measure with `cargo test --release --test load -- --ignored --nocapture --test-threads=1` before/after.

**UDP spawns an unbounded task per packet:**
- Problem: `crates/alpendns/src/server/udp.rs:50` calls `tracker.spawn` for every received datagram with no global in-flight cap. The rate limiter runs *inside* `handle_request` (`server/mod.rs:174`), i.e. after the task exists.
- Files: `crates/alpendns/src/server/udp.rs`, `crates/alpendns/src/server/mod.rs`, `crates/alpendns/src/config.rs:635`
- Cause: `max_inflight` is documented in the example config but not implemented; TCP has `MAX_CONNECTIONS = 64` (`tcp.rs`), UDP has no equivalent.
- Improvement path: add a semaphore/`max_inflight` around UDP tasks (and reject/drop when saturated). This also bounds memory under a spoofed flood.

**Pool health uses a mutex per upstream per cache miss:**
- Problem: `is_down` locks `health.down_until: Mutex<Option<Instant>>` (`crates/alpendns/src/upstream/pool.rs:47,368-373`) for every upstream on every miss, although it is written rarely.
- Files: `crates/alpendns/src/upstream/pool.rs`
- Cause: `Mutex` where an atomic would do; violates the spirit of B.3 rule 5.
- Improvement path: encode as `AtomicU64` monotonic microseconds, `0` = healthy (documented pending candidate 2 in `docs/PERFORMANCE_ANALYSIS.md`).

**`build_event` materializes discarded fields on every request:**
- Problem: `crates/alpendns/src/server/mod.rs:223-252` builds `query_type`, `why` (a `Vec<String>`, one `format!` per trace step) and `findings` on every request, then `aggregate`/`none` discard them.
- Files: `crates/alpendns/src/server/mod.rs`, `crates/alpendns/src/trace.rs`
- Cause: one `QueryEvent` shape regardless of log mode.
- Improvement path: build mode-dependent fields lazily (pending candidate 1); keep the privacy test green.

**Detectors cost ~10 % throughput:**
- Problem: DGA/tunneling/rebinding/typosquat/NRD run on every allowed request; tunneling uses a global `Mutex<HashMap<String, Window>>` (`crates/alpendns/src/detect/tunneling.rs:147`), DGA allocates a `Vec` per label.
- Files: `crates/alpendns/src/detect/`
- Cause: accepted design (ADR-0019); measured worst run 15 %.
- Improvement path: rejected until the documented reversal condition (throughput below two thirds) or a profiler points here; DGA stack buffers and pool mutex are the cheap candidates.

**List reload is serial:**
- Problem: `Lists::load` (`crates/alpendns/src/filter/mod.rs`) iterates specs and downloads one after another; startup and each refresh inherit the sum of list download times.
- Files: `crates/alpendns/src/filter/mod.rs`, `crates/alpendns/src/filter/source.rs`
- Cause: simple sequential loop; no concurrency limit.
- Improvement path: bounded concurrent downloads (e.g., `buffer_unordered(4)`), with per-list failure isolation preserved.

**Log buffering can lose the last interval on a crash:**
- Problem: `run_flusher` flushes a `BufWriter` once per `FLUSH_INTERVAL` (`crates/alpendns/src/logging/mod.rs:745`).
- Files: `crates/alpendns/src/logging/mod.rs`
- Cause: throughput over durability.
- Improvement path: flush on shutdown (verify) and expose the flush interval; document the loss window.

## Fragile Areas

**`reload.rs` and the policy source:**
- Files: `crates/alpendns/src/reload.rs`, `crates/alpendns/src/policy/mod.rs`
- Why fragile: two `ArcSwap`s must agree; a reload either half-applies or leaves old rules. Failure handling differs between startup (strict) and runtime (non-strict, keep old set).
- Safe modification: change the pair into one swapped struct; add a test that forces a failure mid-reload and asserts the resolver still behaves consistently.
- Test coverage: happy-path reload is tested; the interleaving window is not.

**Config validation (`config.rs`, 2,289 lines):**
- Files: `crates/alpendns/src/config.rs`
- Why fragile: many independent `validate_*` helpers, hand-maintained against the example config; already drifted (`max_inflight`, `padding`, `doh_token`) and misses `refresh == 0`.
- Safe modification: add validation and a test in the same change; parse the shipped example in a test so drift fails loudly.
- Test coverage: broad unit tests exist, but not for the shipped example or zero durations.

**The `hickory-proto` boundary:**
- Files: `Cargo.toml`, `crates/alpendns/src/dns.rs`, `crates/alpendns/src/dnssec.rs`
- Why fragile: two known upstream defects (TSIG panic, false `Bogus`) and no stable release with fixes; message parsing/serialization is entirely delegated (ADR-0002), so behavior changes arrive with dependency bumps.
- Safe modification: bump only with the fuzz corpus and DNSSEC integration tests green; keep `known-crashes/` current.
- Test coverage: fuzz targets exist; the corpus gap weakens them.

**`forward_zone` handling:**
- Files: `crates/alpendns/src/config.rs:775`, `crates/alpendns/src/zone.rs`/`ZoneRouter`, `crates/alpendns/src/upstream/`
- Why fragile: a single upstream per forward zone (no failover, TODOS item 13), no LAN-address validation, and nested zone precedence across local/forward (ADR-0029) depends on the `LocalBackend` table carrying the forward names.
- Safe modification: add failover as a list of upstreams with the same validation as pools; test nesting (local deeper than forward, forward deeper than local).
- Test coverage: precedence is tested (ADR-0029); failover and loop prevention are not.

**Rate limiter sharding:**
- Files: `crates/alpendns/src/ratelimit.rs`
- Why fragile: fixed `SHARDS = 16`, IPv6 keyed by /64, one `#[expect]`-justified `unwrap` on `shards.first()`. Behavior is "drop, not refuse" (ADR-0020), so an over-limit client sees timeouts rather than an error.
- Safe modification: keep the non-empty invariant or replace `first()` with a static array; changing the key or shard count changes fairness and must be documented.
- Test coverage: unit tests exist; no test at realistic shard contention.

**Clock and time handling:**
- Files: `crates/alpendns/src/clock.rs`, `docs/THREAT-MODEL.md`
- Why fragile: `Clock` is injectable, but DNSSEC validity and TLS depend on wall-clock correctness; a clock jump after boot can invalidate signatures or TTLs. systemd orders `After=time-sync.target` (`packaging/systemd/alpendns.service`), which is a weak guarantee.
- Safe modification: keep all time reads behind `Clock`; do not add `SystemTime::now()` in cache/policy (B.3 rule 4).
- Test coverage: TTL/schedule tests use the fake clock; clock-jump behavior is not covered.

**systemd sandbox:**
- Files: `packaging/systemd/alpendns.service`
- Why fragile: `ProtectSystem=strict`, restricted `AF_NETLINK`, `StartLimitBurst=5`, unprivileged user with only `CAP_NET_BIND_SERVICE`. A new feature that writes outside the allowed paths or needs another socket family will fail only at runtime under the unit, not in `cargo test`.
- Safe modification: run the unit in a VM/container for packaging changes; keep `docs/OPERATIONS.md` §5 in sync.
- Test coverage: no automated test boots the unit.

## Scaling Limits

**Single-process throughput:**
- Current capacity: measured 84,078 q/s, p99 674 µs, RSS ~39.7 MB (`docs/BENCHMARKS.md`).
- Limit: the logging `counts` mutex and the global tokio runtime are the ceiling; UDP task spawning is unbounded.
- Scaling path: shard counters, add `max_inflight`, then consider `SO_REUSEPORT` workers. Far beyond household needs.

**TCP accept limits:**
- Current capacity: `MAX_CONNECTIONS = 64`, `MAX_PER_CLIENT = 8`, `IDLE_TIMEOUT = 10 s`, `BODY_TIMEOUT = 5 s` (`crates/alpendns/src/server/tcp.rs`).
- Limit: a single client can hold 8 of 64 slots; 8 clients can starve TCP service.
- Scaling path: make limits configurable; ensure the UDP path stays independent (it does).

**Rate-limit table:**
- Current capacity: per-client token buckets in 16 shards; IPv6 aggregated per /64 (`crates/alpendns/src/ratelimit.rs`).
- Limit: a spoofed source flood creates many buckets; no eviction policy is documented.
- Scaling path: cap/evict idle buckets, or key IPv6 more coarsely under flood.

**Blocklist memory:**
- Current capacity: matcher ~69 bytes/entry (`docs/BENCHMARKS.md`); `MAX_LIST_BYTES = 64 MiB` per list (`crates/alpendns/src/filter/source.rs`).
- Limit: several 64 MiB lists with millions of entries can exceed the ~40 MB baseline by hundreds of MB; no global memory cap.
- Scaling path: document the per-entry cost and a recommended total; consider loading only the matcher form rather than retaining source text.

**Log counters:**
- Current capacity: `MAX_TRACKED = 200_000` distinct domains (`crates/alpendns/src/logging/counts.rs`).
- Limit: beyond that, new domains are not tracked; the top list becomes approximate.
- Scaling path: document the bound; increase only with a memory budget.

## Dependencies at Risk

**`hickory-proto` / `hickory-net` 0.26.1:**
- Risk: TSIG panic with overflow checks and false `Bogus` on CNAME→NODATA (ADR-0028). Fixes are only in 0.27 alpha.
- Impact: the known crash and ~1 % SERVFAILs; upgrading to 0.27 will be a breaking change.
- Migration plan: track hickory releases, run the fuzz corpus and DNSSEC integration tests on the bump, and drop `known-crashes/` only when a stable release contains the fix.

**`time` 0.3.45:**
- Risk: RUSTSEC-2026-0009, reachable in the dependency graph, currently judged unreachable in practice (`deny.toml`).
- Impact: a future code path that parses RFC-2822 dates could expose it; the exception hides new advisories on the same crate.
- Migration plan: raise MSRV to 1.88 and update `time`, then remove the exception.

**`reqwest`:**
- Risk: large dependency tree (TLS, HTTP/2), default redirect policy, and its own security advisories.
- Impact: blocklist downloads and DoH upstreams depend on it; behavior changes on upgrade.
- Migration plan: keep `cargo deny` current; pin redirect policy explicitly; no telemetry features.

**`odoh-rs` and `psl`:**
- Risk: smaller, less-audited crates on the encrypted-DNS and zone-routing paths.
- Impact: ODoH failures or PSL staleness affect privacy routing and `split_by_zone`.
- Migration plan: review before upgrades; PSL data freshness is a known limitation of the crate.

**MSRV 1.85:**
- Risk: `Cargo.toml` declares `rust-version = "1.85"`, but the `time` fix needs 1.88.
- Impact: the RUSTSEC exception cannot be removed until the MSRV is raised.
- Migration plan: raise MSRV once Debian/Ubuntu stable toolchains allow it, and document the floor.

## Missing Critical Features

**Encrypted client listeners (DoT/DoH/DoQ):**
- Problem: the server only listens on UDP/TCP; `[server.doh]`, `[server.dot]`, `[server.doq]` in the example config are aspirational/marked `[PHASE 3+]`.
- Blocks: private DNS on the client side; any deployment where the LAN is not trusted.

**RFC 6303 built-in local zones:**
- Problem: no built-in defaults for `localhost`, reverse zones, etc.; every deployment must configure them (TODOS item 11, ADR-0029).
- Blocks: safe out-of-the-box behavior and correct handling of private reverse lookups.

**Forward-zone failover:**
- Problem: `ForwardZone` has one `upstream` (`crates/alpendns/src/config.rs:775`); TODOS item 13 is open.
- Blocks: resilience for the LAN forward path; a dead domain controller takes the zone down.

**Operational features from the roadmap backlog:**
- Problem: block page (TODOS 8), config wizard (TODOS 9), two-instance resilience (TODOS 10), config version/aliases (TODOS 7b) are not implemented.
- Blocks: user-friendly blocking, guided setup, safe blue/green upgrades, and backward-compatible config evolution.

**Documented-but-unimplemented config knobs:**
- Problem: `max_inflight`, EDNS `padding`, and `doh_token` client matching exist only in `config/alpendns.example.toml`.
- Blocks: padding-based privacy on the wire, connection limits, and token-based client identity for future DoH.

**Recursion from the root:**
- Problem: not implemented and explicitly not a v1 goal; the `ResolveBackend` trait leaves room.
- Blocks: independence from upstreams. Deliberate.

## Test Coverage Gaps

**The shipped example config is not parsed by any test:**
- What's not tested: `config/alpendns.example.toml` round-trips through `Config::from_str`/validation.
- Files: `config/alpendns.example.toml`, `crates/alpendns/src/config.rs`
- Risk: the shipped documentation silently drifts and users hit startup failures; this gap already hides three unimplemented keys.
- Priority: High (cheap, catches a real defect).

**Zero/edge durations are not validated:**
- What's not tested: `refresh = "0s"`, absurdly small `flush`/`seed_rotation`, `tunneling.window = 0` combinations.
- Files: `crates/alpendns/src/config.rs`, `crates/alpendns/src/policy/mod.rs`
- Risk: startup abort or dead tasks from a plausible typo.
- Priority: High.

**Fuzz corpus and detector measurement corpora are absent:**
- What's not tested: fuzz targets start from an empty corpus in CI; measurement thresholds cannot be reproduced.
- Files: `crates/alpendns/fuzz/`, `.gitignore`, `docs/TESTING.md` §6
- Risk: regressions in parsers and detector accuracy go unnoticed; the 120 s CI budget is less effective.
- Priority: High for seeds, Medium for measurement corpora.

**Real encrypted transports are not exercised in integration tests:**
- What's not tested: DoT/DoH/DoQ/ODoH against a real peer; tests use a fake upstream by design (`docs/TESTING.md`).
- Files: `crates/alpendns/src/upstream/transport.rs`, `crates/alpendns/src/upstream/`, `tests/`
- Risk: interop regressions on dependency upgrades (TLS, HTTP/2, ODoH) appear only in the field.
- Priority: Medium; a local TLS test server (no internet) would close most of it.

**Reload failure and interleaving are untested:**
- What's not tested: a failing list download during reload, and the two-`ArcSwap` window.
- Files: `crates/alpendns/src/reload.rs`, `crates/alpendns/src/policy/mod.rs`
- Risk: inconsistent policy state after SIGHUP.
- Priority: Medium.

**No test boots the systemd unit or the `.deb`:**
- What's not tested: sandbox paths, `ExecStartPre = alpendns check`, file permissions, logrotate interaction.
- Files: `packaging/systemd/alpendns.service`, `packaging/debian/`, `packaging/logrotate/alpendns`
- Risk: packaging-only failures at install/upgrade time.
- Priority: Medium; a container smoke test would cover it.

**No protocol-precision tests for multi-question/edge headers:**
- What's not tested: `QDCOUNT > 1`, unusual opcodes beyond `NotImp`, EDNS edge cases.
- Files: `crates/alpendns/src/server/mod.rs`
- Risk: silent truncation and inconsistent answers.
- Priority: Low–Medium.

**Load tests are `--ignored` and Linux-only:**
- What's not tested: performance regressions in CI.
- Files: `tests/load.rs`, `docs/PERFORMANCE_ANALYSIS.md`
- Risk: the pending perf candidates cannot be evaluated automatically; no regression gate on q/s.
- Priority: Low (measurement is manual by design).

---

*Concerns audit: 2026-10-20*
