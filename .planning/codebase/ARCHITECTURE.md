---
last_mapped_commit: 630297fc35c98188292c5305d0e279aa6039b07a
last_mapped_at: 2026-10-20
---
<!-- refreshed: 2026-10-20 -->

# Architecture

**Analysis Date:** 2026-10-20

## System Overview

AlpenDNS is one process, one binary, one TOML file. The testable code lives in
the library `crates/alpendns/src/lib.rs`; `crates/alpendns/src/main.rs` only does
startup, argument parsing, signal handling and shutdown. A request is handled by
a chain of `ResolveBackend` implementations, each of which knows only the one
below it.

```text
┌──────────────────────────────────────────────────────────────┐
│ 1  Listeners                    crates/alpendns/src/server/  │
│    UDP `udp.rs`, TCP `tcp.rs`; rate limit, parse, client addr│
└───────────────────────────────┬──────────────────────────────┘
                                │ handle_request (server/mod.rs)
                                ▼
┌──────────────────────────────────────────────────────────────┐
│ 2  PolicyBackend                crates/alpendns/src/policy/  │
│    Engine: temporary allow/deny → allowlist → blocklists →   │
│    regex → schedule → name detectors; synthetic answers      │
└───────────────────────────────┬──────────────────────────────┘
                                │ allowed
                                ▼
┌──────────────────────────────────────────────────────────────┐
│ 3  CachingBackend               crates/alpendns/src/caching.rs
│    Cache (16 shards, LRU), serve-stale, prefetch,            │
│    query coalescing, short-lived failure cache                │
└───────────────────────────────┬──────────────────────────────┘
                                │ miss / stale
                                ▼
┌──────────────────────────────────────────────────────────────┐
│ 4  LocalBackend                 crates/alpendns/src/local.rs │
│    [[local_zone]] table, resolver.arpa (NODATA),             │
│    nested [[forward_zone]] precedence (ADR-0029)             │
└───────────────────────────────┬──────────────────────────────┘
                                │ not answered locally
                                ▼
┌──────────────────────────────────────────────────────────────┐
│ 5  ZoneRouter                  crates/alpendns/src/router.rs │
│    [[forward_zone]] → ForwardBackend (cleartext LAN)         │
│    everything else → Pool                                    │
└───────────────────────────────┬──────────────────────────────┘
                                ▼
┌──────────────────────────────────────────────────────────────┐
│ 6  Pool → Encrypted             crates/alpendns/src/upstream/│
│    split_by_zone selection, passive health, seed rotation    │
│    Transport (DoT/DoH/DoQ) or OdohBackend (Oblivious DoH)    │
└───────────────────────────────┬──────────────────────────────┘
                                │ answer
                                ▼
       post-processing in PolicyBackend (answer detectors)
       → server::handle_request (DNSSEC for client)
       → logging::QueryLog (the only exit for the Trace)
```

Background services run beside the request path: the periodic list updater
(`policy::run_updater`), the `SIGHUP` reload (`main::reload_on_hangup`), the seed
rotation (`upstream::pool::run_seed_rotation`), the query-log flusher
(`logging::run_flusher`), the 24-hour history sampler (`main::sample_history`),
and the two HTTP listeners (API/UI and metrics).

## Component Responsibilities

| Component | Responsibility | File |
|-----------|----------------|------|
| Binary entry | Args, config load, wiring, signals, shutdown | `crates/alpendns/src/main.rs` |
| Module index | Public modules that mirror the target crates | `crates/alpendns/src/lib.rs` |
| Listener pipeline | UDP/TCP accept loops, one task per request | `crates/alpendns/src/server/mod.rs`, `server/udp.rs`, `server/tcp.rs` |
| Rate limiting | Per-client token bucket, drops over-limit queries | `crates/alpendns/src/ratelimit.rs` |
| Policy | Client identity, lists, regex, schedules, detectors, block synthesis | `crates/alpendns/src/policy/mod.rs`, `policy/clients.rs`, `policy/rules.rs`, `policy/schedule.rs`, `policy/temporary.rs` |
| Cache | TTL clamping, serve-stale, prefetch, sharded LRU | `crates/alpendns/src/cache.rs` |
| Cache layer | Cache lookup/insert, query coalescing, failure cache | `crates/alpendns/src/caching.rs` |
| Local zones | Static table answers, `resolver.arpa`, zone precedence | `crates/alpendns/src/local.rs` |
| Zone routing | Per-zone dispatch to LAN forwarders | `crates/alpendns/src/router.rs` |
| Upstreams | Pools, selection, health, transports, ODoH | `crates/alpendns/src/upstream/mod.rs`, `upstream/pool.rs`, `upstream/strategy.rs`, `upstream/transport.rs`, `upstream/odoh.rs` |
| Blocklists | Parsing, matching, loading/updating | `crates/alpendns/src/filter/mod.rs`, `filter/parser.rs`, `filter/matcher.rs`, `filter/source.rs`, `filter/block.rs` |
| Heuristics | DGA, tunneling, rebinding, typosquat, NRD | `crates/alpendns/src/detect/mod.rs` and submodules |
| Decision trace | `Ctx` and `Step`, always produced | `crates/alpendns/src/trace.rs` |
| Logging | The only place query names may survive | `crates/alpendns/src/logging/mod.rs`, `logging/counts.rs`, `logging/ring.rs` |
| DNSSEC | Local signature-chain verdicts and client-facing stripping | `crates/alpendns/src/dnssec.rs` |
| Privacy | ECS stripping, padding, cookies, 0x20 | `crates/alpendns/src/privacy.rs` |
| DNS helpers | Response validation, truncation, error answers | `crates/alpendns/src/dns.rs` |
| HTTP API | Authenticated JSON API and SSE | `crates/alpendns/src/api/mod.rs` |
| Web UI | Embedded HTML/CSS/JS served on three routes | `crates/alpendns/src/api/ui.rs`, `web/` |
| Metrics | Prometheus text endpoint without names | `crates/alpendns/src/metrics.rs` |
| History | 24-hour counter-only time series | `crates/alpendns/src/history.rs` |
| Reload | Hot-swappable policy blueprint and list sources | `crates/alpendns/src/reload.rs` |
| Clock | Injectable monotonic and wall clocks | `crates/alpendns/src/clock.rs` |

## Pattern Overview

**Overall:** Layered forwarding resolver built as a decorator chain around a
single `ResolveBackend` trait, with a per-request decision trace threaded
through every layer.

**Key Characteristics:**
- **One trait, one chain.** `ResolveBackend` is implemented by every layer;
  `main` wires the concrete order once (`crates/alpendns/src/main.rs:421`).
- **Filtering before caching.** The cache stores unfiltered answers so all
  clients share it; policy runs in `PolicyBackend`, outside the cache
  (`crates/alpendns/src/policy/mod.rs:7`).
- **Trace always, names only via logging.** Every request has a `Ctx` with
  steps; `server::handle_request` is the single place it leaves the pipeline
  (`crates/alpendns/src/server/mod.rs:218`).
- **Module-first layout.** Everything is a module in one crate; crates are only
  split out when a phase needs it (`crates/alpendns/src/lib.rs:1`).
- **Time and state are injectable.** `Clock`/`WallClock` traits; rarely changing
  state behind `ArcSwap`; no global mutex in the request path.

## Layers

**Listener layer:**
- Purpose: bind sockets, terminate transport, enforce connection and rate
  limits, parse messages, set the client address.
- Location: `crates/alpendns/src/server/`
- Contains: `Server`, `Bound`, `handle_request`, UDP and TCP serve loops, TCP
  connection caps.
- Depends on: `config::ServerConfig`, `dns`, `logging::QueryLog`,
  `ratelimit::RateLimiter`.
- Used by: `main`.
- Note: only UDP and TCP listeners are implemented. The `[server.doh]`,
  `[server.dot]`, `[server.doq]` sections in `config/alpendns.example.toml` are
  target-format documentation and are rejected by the current parser as unknown
  fields.

**Policy layer:**
- Purpose: identify the client, choose a policy, evaluate temporary grants,
  allowlists, blocklists, regex, schedules and name detectors; synthesize block
  answers; inspect answers for rebinding.
- Location: `crates/alpendns/src/policy/`
- Contains: `Engine`, `PolicySet`, `Policy`, `Blueprint`, `Clients`, `RegexRules`,
  `Schedule`, `Temporary`, `PolicyBackend`.
- Depends on: `filter` matchers and block synthesis, `detect`, `clock`,
  `trace`, `config`.
- Used by: `CachingBackend` (wrapped by `PolicyBackend`).

**Cache layer:**
- Purpose: answer repeated questions, serve stale entries, prefetch, coalesce
  identical in-flight queries, briefly remember failures.
- Location: `crates/alpendns/src/caching.rs`, `crates/alpendns/src/cache.rs`
- Contains: `CachingBackend`, `Inflight`, `Failures`, `Cache`, `Key`, `Hit`.
- Depends on: `config::CacheConfig`, `clock`, `trace`.
- Used by: `PolicyBackend`.

**Local and routing layer:**
- Purpose: answer `[[local_zone]]` names and `resolver.arpa`; dispatch
  `[[forward_zone]]` names to LAN servers and everything else to the pool.
- Location: `crates/alpendns/src/local.rs`, `crates/alpendns/src/router.rs`
- Contains: `LocalBackend`, `LocalZone`, `LocalZoneConfig`, `ZoneRouter`.
- Depends on: `config`, `resolve`, `trace`.
- Used by: `CachingBackend`.

**Upstream layer:**
- Purpose: choose an upstream, maintain passive health, speak DoT/DoH/DoQ or
  ODoH, validate answers against questions.
- Location: `crates/alpendns/src/upstream/`
- Contains: `Pool`, `Upstream`, `Encrypted`, `Transport`, `OdohBackend`,
  `ForwardBackend`, `strategy`.
- Depends on: `hickory-net`, `rustls`, `reqwest`, `odoh-rs`, `dnssec`,
  `privacy`, `dns`.
- Used by: `ZoneRouter` and `ForwardBackend` zones.

**Observability layer:**
- Purpose: turn traces into counters, ring entries, files; expose JSON API,
  SSE, Prometheus metrics, UI and history.
- Location: `crates/alpendns/src/logging/`, `crates/alpendns/src/api/`,
  `crates/alpendns/src/metrics.rs`, `crates/alpendns/src/history.rs`
- Contains: `QueryLog`, `Mode`, `QueryEvent`, `BlockReason`, `StatusSource`,
  `ApiState`, `Snapshot`, `History`.
- Depends on: `trace::Step`, `config` logging settings.
- Used by: `main` and `server::handle_request`.

## Data Flow

### Primary Request Path

1. A listener receives bytes and gets the peer address
   (`crates/alpendns/src/server/udp.rs:32`, `crates/alpendns/src/server/tcp.rs`).
2. `handle_request` checks the rate limiter first and drops over-limit queries
   without answering (`crates/alpendns/src/server/mod.rs:174`).
3. The request is parsed with `Message::from_vec`; unparseable input becomes
   FORMERR, non-query opcodes become NOTIMP (`crates/alpendns/src/server/mod.rs:185`).
4. A fresh `Ctx` is created with the peer address
   (`crates/alpendns/src/trace.rs:203`).
5. `PolicyBackend::resolve` evaluates the question through `Engine`, writes
   trace steps, and returns a synthesized answer or continues inward
   (`crates/alpendns/src/policy/mod.rs:628`).
6. `CachingBackend::resolve` returns a cache hit (with optional background
   refresh), a short-lived cached failure, joins an in-flight identical query,
   or becomes the leader and calls inward (`crates/alpendns/src/caching.rs:76`).
7. `LocalBackend::resolve` answers table entries and `resolver.arpa`, or passes
   the request to `ZoneRouter` (`crates/alpendns/src/local.rs:254`).
8. `ZoneRouter::resolve` picks the most specific `forward_zone` backend or the
   pool default (`crates/alpendns/src/router.rs:39`).
9. `Pool::resolve` orders upstreams by `split_by_zone`, demotes down upstreams,
   and tries them in order (`crates/alpendns/src/upstream/pool.rs:377`).
10. `Encrypted::resolve` dispatches to `Transport` (DoT/DoH/DoQ) or
    `OdohBackend` (`crates/alpendns/src/upstream/mod.rs:40`).
11. On the way back, `PolicyBackend` runs answer detectors (rebinding) and may
    replace the answer with a block response
    (`crates/alpendns/src/policy/mod.rs:656`).
12. `handle_request` applies client-specific DNSSEC record stripping, sets RA,
    builds the `QueryEvent`, and hands the trace to `QueryLog`
    (`crates/alpendns/src/server/mod.rs:201`, `:218`).

### Background: List Updates and Reload

1. `SIGHUP` parses the config and `PolicySource::reload` swaps blueprint and
   list sources atomically, or keeps the old state on error
   (`crates/alpendns/src/reload.rs:47`, `crates/alpendns/src/main.rs:1360`).
2. The reload wakes `policy::run_updater` via `Notify`
   (`crates/alpendns/src/policy/mod.rs:572`).
3. The updater reloads lists non-strictly, builds a new `PolicySet`, and calls
   `Engine::replace` (`ArcSwap`), so requests never block
   (`crates/alpendns/src/policy/mod.rs:595`).

### Background: Observability

1. `sample_history` reads a non-generic `StatusSource::snapshot` every 30 s and
   stores counter-only samples (`crates/alpendns/src/main.rs:867`).
2. The API listener serves authenticated routes plus the embedded UI
   (`crates/alpendns/src/api/mod.rs:127`, `crates/alpendns/src/api/ui.rs:19`).
3. The metrics listener serves a separate tokenless Prometheus endpoint that
   contains no query names (`crates/alpendns/src/api/mod.rs:153`).

**State Management:**
- `ArcSwap` for policy sets, blueprints and list sources.
- Sharded `Mutex`-protected LRU segments in `Cache` (16 shards).
- Atomic counters for statistics, health and privacy counters.
- `Mutex` only in off-path state: query log, history, ODoH/server cookies,
  TCP connection accounting, rate-limit client table.
- `Ctx` is passed exclusively as `&mut`; there is no trace mutex
  (`crates/alpendns/src/trace.rs:1`).

## Key Abstractions

**`ResolveBackend`:**
- Purpose: the single seam for "turn a request into an answer"; the slot where a
  recursive backend would later sit.
- Examples: `crates/alpendns/src/resolve.rs:64`, implementations in
  `policy/mod.rs`, `caching.rs`, `local.rs`, `router.rs`, `upstream/pool.rs`,
  `upstream/mod.rs`, `upstream/transport.rs`, `upstream/odoh.rs`.
- Pattern: blanket impl for `Arc<B>` (`crates/alpendns/src/resolve.rs:74`).

**`Ctx` and `Step`:**
- Purpose: explain why an answer happened, independent of the log mode.
- Examples: `crates/alpendns/src/trace.rs:43` (`Step`), `:203` (`Ctx`).
- Pattern: append-only list of named steps carried by `&mut`; `Step` values
  render themselves for CLI and UI.

**`Engine` and `PolicySet`:**
- Purpose: current rule state plus counters and temporary grants/denials.
- Examples: `crates/alpendns/src/policy/mod.rs:137`, `:50`.
- Pattern: `ArcSwap<PolicySet>`; `Blueprint::build` resolves list references and
  fails on unknown lists.

**`PolicySource`:**
- Purpose: the hot-reloadable half of the configuration.
- Examples: `crates/alpendns/src/reload.rs:27`.
- Pattern: two `ArcSwap`s swapped together; failure leaves the old state.

**`Clock` / `WallClock`:**
- Purpose: make TTL, schedule and temporary-grant tests deterministic.
- Examples: `crates/alpendns/src/clock.rs:12`, `:39`.
- Pattern: generic parameter through cache, policy and pool; system and test
  implementations.

**`StatusSource`:**
- Purpose: keep the API module independent of concrete pipeline types.
- Examples: `crates/alpendns/src/api/mod.rs:35`, implemented by `Runtime` in
  `crates/alpendns/src/main.rs:760`.
- Pattern: object-safe trait returning plain serializable views.

**`NameDetector` / `AnswerDetector`:**
- Purpose: heuristics at the two points where evidence exists: the question and
  the answer.
- Examples: `crates/alpendns/src/detect/mod.rs:232`, `:246`.
- Pattern: each detector returns `Option<Finding>`; a `Finding` always carries a
  human-readable reason; all default to `flag`.

**`Lists` / `Loader`:**
- Purpose: fetch, cache and parse blocklists.
- Examples: `crates/alpendns/src/filter/mod.rs:53`, `filter/source.rs`.
- Pattern: strict load at startup, tolerant load on updates; atomic swap of the
  built rule set.

## Entry Points

**Binary:**
- Location: `crates/alpendns/src/main.rs:127`
- Triggers: `alpendns -c <file>`; subcommands `check` and `policy test`.
- Responsibilities: parse args, load and validate config, build detectors,
  pools, zones and the backend chain, bind listeners, spawn background tasks,
  handle SIGINT/SIGTERM/SIGHUP, flush the log on shutdown.

**UDP/TCP listeners:**
- Location: `crates/alpendns/src/server/udp.rs:20`,
  `crates/alpendns/src/server/tcp.rs`
- Triggers: DNS traffic on configured `server.listen_udp` / `listen_tcp`.
- Responsibilities: framing, connection limits, spawning per-request tasks.

**HTTP API:**
- Location: `crates/alpendns/src/api/mod.rs:127`
- Triggers: `api.listen` when `api.enabled`.
- Responsibilities: token auth, status, top names, history, explain, recent
  queries, lists, policies, temporary allow/deny, flags, SSE.

**Metrics:**
- Location: `crates/alpendns/src/api/mod.rs:153`
- Triggers: `metrics.listen` when `metrics.enabled`.
- Responsibilities: Prometheus text output with counters only.

**Web UI:**
- Location: `crates/alpendns/src/api/ui.rs:19`
- Triggers: `/`, `/app.css`, `/app.js` on the API listener.
- Responsibilities: serve `web/index.html`, `web/app.css`, `web/app.js` embedded
  with `include_str!`.

**CLI check:**
- Location: `crates/alpendns/src/main.rs:991`
- Triggers: `alpendns -c <file> check` / systemd `ExecStartPre`.
- Responsibilities: config validation, directory writability, hints; no network.

**CLI policy test:**
- Location: `crates/alpendns/src/main.rs:1233`
- Triggers: `alpendns -c <file> policy test <domain> [--client name]`.
- Responsibilities: evaluate a name against the configured policy without
  starting the server, using the same `policy::explain` as the API.

## Architectural Constraints

- **Threading:** Tokio multithreaded runtime, one task per request or
  connection; `TaskTracker` drains in-flight work on shutdown. The runtime is
  built by hand, not via `#[tokio::main]`, so startup errors are messages rather
  than panics (`crates/alpendns/src/main.rs:357`).
- **Global state:** process-wide atomics in statistics modules (`detect`,
  `privacy`, `dnssec`, `logging`); caches and policy state are owned by their
  layers and shared with `Arc`.
- **No `unsafe`, no panics on network data:** enforced by workspace lints in
  `Cargo.toml` (`unsafe_code = "forbid"`, `unwrap_used = "deny"`,
  `panic = "deny"`, `indexing_slicing = "deny"`); `RUSTFLAGS=-D warnings` makes
  `expect_used` fatal in CI.
- **One upstream per request:** `fanout` was removed (ADR-0012); the trace is no
  longer behind a mutex and `Ctx` is `&mut`.
- **Cache stores unfiltered answers:** filtering lives outside the cache and
  before it (`crates/alpendns/src/cache.rs:1`).
- **Time is injectable:** no `SystemTime::now()` in cache or policy; use
  `Clock`/`WallClock`.
- **Query names have one exit:** only `logging::QueryLog::record` may persist
  them (`crates/alpendns/src/server/mod.rs:218`); local answers deliberately use
  `Step::LocalAnswer`, not `Step::Synthesized`, so they are not counted as
  blocks.
- **SO_REUSEPORT is not set in the code.** Multiple UDP sockets are bound only
  if multiple addresses are configured (`crates/alpendns/src/server/mod.rs:79`);
  the target picture in `docs/ARCHITECTURE.md` §8 still describes one socket per
  worker.
- **DoT/DoH/DoQ listeners are not implemented.** Only upstream transports exist;
  the listener config sections are target format.

## Anti-Patterns

### Logging a query name outside the logging layer

**What happens:** `tracing::info!` with a name, or a metric label built from a
query.
**Why it's wrong:** it bypasses `privacy.logging.mode` and breaks the project's
central privacy promise; the test
`no_query_name_leaves_the_process_in_the_quiet_modes` catches it.
**Do this instead:** add a `Step` to `Ctx` and let `QueryLog` decide what
survives (`crates/alpendns/src/logging/mod.rs:369`).

### Bypassing the `ResolveBackend` chain

**What happens:** a layer calls an upstream or the cache directly instead of
resolving through its inner backend.
**Why it's wrong:** it breaks "each layer knows only the one below it", skips
the trace, and makes cache isolation and policy placement impossible to reason
about.
**Do this instead:** implement `ResolveBackend` and compose it in `main`
(`crates/alpendns/src/main.rs:421`).

### Returning `bool` from a matcher

**What happens:** a blocklist or policy matcher returns only whether it matched.
**Why it's wrong:** the trace cannot name the list and line, so "why was this
blocked?" becomes unanswerable.
**Do this instead:** return a reference to the rule (`RuleRef` / `Match` in
`crates/alpendns/src/filter/matcher.rs:20`) and record a `Step`.

### Holding a global lock in the request path

**What happens:** a single `Mutex` around cache, policy or trace.
**Why it's wrong:** it serializes all requests and reintroduces the
`fanout`-era trace mutex that was deliberately removed.
**Do this instead:** `ArcSwap` for rarely changing state, sharded locks for the
cache, `&mut Ctx` for the trace (`crates/alpendns/src/policy/mod.rs:139`).

### Reading wall-clock time directly in cache or policy

**What happens:** `SystemTime::now()` / `Instant::now()` inside TTL or schedule
logic.
**Why it's wrong:** tests become `sleep`-based and slow; TTL and schedule
boundaries cannot be tested deterministically.
**Do this instead:** take a `Clock`/`WallClock` and use `TestClock` / `FixedWallClock`
(`crates/alpendns/src/clock.rs:91`).

## Error Handling

**Strategy:** typed errors at layer boundaries, fail closed on policy and fail
open on availability, no panics on network input.

**Patterns:**
- `ResolveError` enumerates timeout, I/O, encode/decode, mismatch, no upstream,
  bogus DNSSEC and cached failure (`crates/alpendns/src/resolve.rs:16`).
- `handle_request` maps resolution failures to SERVFAIL and unparseable input to
  FORMERR (`crates/alpendns/src/server/mod.rs:210`, `crates/alpendns/src/dns.rs:87`).
- Upstream answers are checked against ID, name, type and class before entering
  the cache (`crates/alpendns/src/dns.rs:38`).
- Startup with unloadable lists aborts; later failures keep the last good rule
  set and log (`crates/alpendns/src/filter/mod.rs:84`,
  `crates/alpendns/src/policy/mod.rs:595`).
- Config parse failures are startup errors; reload failures keep the old config
  (`crates/alpendns/src/reload.rs:47`).
- `panic = "abort"` in release: a half-broken resolver is worse than a
  restarting one.

## Cross-Cutting Concerns

**Logging:** `tracing` with structured fields; module-level `tracing` calls never
include query names; the query log is a separate, mode-controlled sink. The
default mode is `aggregate` (`crates/alpendns/src/logging/mod.rs:37`).

**Validation:** `serde(deny_unknown_fields)` on every config struct; semantic
checks in `Config::validate` and per-section validators
(`crates/alpendns/src/config.rs:1118`); list references are checked in
`Blueprint::build`.

**Authentication:** the API uses a bearer token (or query parameter for SSE),
read from or created at `api.token_file` with mode 0600
(`crates/alpendns/src/main.rs:913`). Metrics are tokenless by design and contain
no names.

**Privacy:** ECS stripping, EDNS padding, cookies and 0x20 are applied only in
the transports where they make sense; DNSSEC is validated locally on encrypted
transports (`crates/alpendns/src/privacy.rs:1`, `crates/alpendns/src/dnssec.rs:1`).

**Time:** `Clock` for monotonic durations (cache, TTL, rate limiting, health),
`WallClock` for local-time schedules; both injectable.

---

*Architecture analysis: 2026-10-20*
