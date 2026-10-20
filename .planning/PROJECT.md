# AlpenDNS

## What This Is

AlpenDNS is a privacy-focused DNS forwarding resolver for Linux, written in Rust.
It accepts queries from the local network over UDP/TCP, filters them against
blocklists and per-client policies, and forwards what remains over encrypted
transports (DoT, DoH, DoQ or Oblivious DoH). It is one binary and one TOML file,
operated as a systemd service on Debian/Ubuntu; recursion from the root servers
is deliberately not part of v1.

## Core Value

Queries are filtered on the operator's own machine and forwarded encrypted: no
query name leaves the process unless the configured logging mode explicitly
allows it.

## Success Metric

The four Definition-of-Done commands in `docs/TESTING.md` are green
(`cargo fmt --all --check`, `cargo clippy --all-targets --all-features -- -D warnings`,
`cargo test --all-features`, `cargo deny check`), and the forwarding resolver
passes acceptance in the real network as documented in `docs/ROADMAP.md`: the
phase 8 observation period with all detectors on `flag` and the phase 9
practical test and fresh-VM installation.

## Requirements

Summary only. The checkable set with IDs and traceability is
`.planning/REQUIREMENTS.md`.

### Validated

- Phase 0 — workspace, formatting, clippy, cargo-deny, CI and the Definition of Done.
- Phase 1 — a forwarding resolver over UDP/TCP with answer validation, strict config and graceful shutdown.
- Phase 2 — a TTL-clamped cache with LRU bound, query coalescing, serve-stale, prefetch and failure memory.
- Phase 3 — encrypted upstreams (DoT/DoH/DoQ), `split_by_zone`, passive health, forward zones and privacy basics.
- Phase 4 — blocklists (hosts/domains/wildcard), a `RuleRef` matcher, allowlists, block modes and atomic updates.
- Phase 5 — per-client policies, schedules, temporary exemptions, regex rules and the explain path shared by CLI and API.
- Phase 6 — token-protected API, Prometheus metrics, four log modes, SSE live stream and the self-served web UI.
- Phase 7 — PSL-backed `split_by_zone` with seed rotation, privacy budget, local DNSSEC validation, ODoH and k-anonymity.

### Active

- [ ] Phase 8 acceptance — a week of real-network operation with all five detectors on `flag`, review of the false-positive list, and only then a decision on detector defaults (ADR-0025 and ADR-0026 remain proposed).
- [ ] Phase 9 acceptance — the practical week as the only LAN resolver and installation on a fresh Debian VM.
- [ ] Phase 10 — optional, discardable stockpile from `docs/ROADMAP.md` and `docs/TODOS.md`, taken item by item when wanted.

### Out of Scope

- Recursion from the root servers in v1 — the door stays open behind `ResolveBackend`, but nothing is pre-built (ADR-0003).
- Cleartext DNS outbound, except explicitly configured `forward_zone` entries into the LAN (B.1 rule 7).
- Telemetry, update checks, crash reporting or any outbound contact beyond configured upstreams, blocklist URLs and ODoH key fetches (B.1 rule 4).
- Adblock and RPZ blocklist parsers — removed; `hosts`, `domains` and `wildcard` remain (ADR-0014).
- Block mode `refused` — removed; `nxdomain`, `zero_ip` and `sinkhole` remain (ADR-0013).
- Upstream strategies `fastest`, `round_robin` and `fanout` — removed; `split_by_zone` is the only strategy (ADR-0011, ADR-0012).
- Bloom filter, label reversal and inverted trie for blocklist matching — measurements reject them; memory footprint is the reversal criterion (ADR-0008).
- A web UI build step, external fonts, CDN resources or client-side analytics (ADR-0010, CLAUDE.md B.6).
- Automatic config migration or a config version field (ADR-0024).
- Moving a detector default from `flag` to `block` without the documented observation and false-positive review (ADR-0019, B.8).

## Context

This is a brownfield project. Phases 1 to 9 are implemented in
`crates/alpendns`; phases 1 to 7 are accepted, while the acceptance run for
phases 8 and 9 is in progress on the real network. `docs/ROADMAP.md` is the
canonical phase state and carries the acceptance narrative; this planning view
must not diverge from it.

The request path is a decorator chain of `ResolveBackend` implementations:
`PolicyBackend` -> `CachingBackend` -> `LocalBackend` -> `ZoneRouter` -> `Pool`
-> `Encrypted` (DoT/DoH/DoQ) or `OdohBackend`, with `ForwardBackend` only for
`forward_zone` names. A `Ctx` carries the client address and an always-produced
decision trace; the trace leaves the pipeline at exactly one place,
`server::handle_request` -> `logging::QueryLog`.

Everything testable lives in the library (`crates/alpendns/src/lib.rs` and its
modules); `main.rs` only does startup, signals and shutdown. The target crate
split in CLAUDE.md B.3 is deliberately deferred until a phase needs it. The
observability surface is in `api/`, `metrics.rs`, `history.rs` and `logging/`.

Known technical debt, defects and open concerns are catalogued in
`.planning/codebase/CONCERNS.md`; open ideas with implementation plans are in
`docs/TODOS.md` (phase 10 material). Architecture and rationale live in
`docs/ARCHITECTURE.md` and `docs/adr/`.

## Constraints

- **Runtime**: Linux (Debian/Ubuntu) as a systemd service; Rust 2024 edition on a
  stable toolchain; tokio multithreaded; a single binary crate
  `crates/alpendns` until a phase requires a split.
- **Hostile network input**: every byte from the network is hostile. No
  `unwrap()`, `expect()`, `panic!()` or slice indexing on network paths;
  `unsafe` is forbidden (B.1 rules 1 and 2, enforced by workspace lints).
- **Privacy**: no query names in the log without explicit configuration; the
  default logging mode is `aggregate`. Only `logging::QueryLog` may persist
  names (B.1 rule 3, ADR-0004).
- **No unsolicited outbound**: the process contacts configured upstreams,
  configured blocklist URLs and ODoH key endpoints, nothing else (B.1 rule 4).
- **Configuration**: `serde(deny_unknown_fields)` everywhere; the server does not
  start with broken configuration; renamed keys keep a `serde(alias)` for at
  least one major version (B.1 rule 5, ADR-0024).
- **Availability**: fail closed on policy, fail open on availability; a broken
  list keeps the last cached version, a dead upstream means the next one (B.1
  rule 6).
- **DNS stack**: wire format from `hickory-proto`; transports from `hickory-net`;
  no cleartext DNS outbound except LAN `forward_zone` (ADR-0002, ADR-0007).
- **Testing**: the four Definition-of-Done commands are the gate; every bugfix
  starts with a reproducing test; parsers get broken, truncated and malicious
  input tests; integration tests use loopback and never contact real upstreams
  or real blocklist URLs; fuzzing needs nightly (B.4, `docs/TESTING.md`).
- **Security**: unprivileged service with `CAP_NET_BIND_SERVICE`; per-client rate
  limiting before any public listener; every upstream answer is validated
  against its question before it enters the cache (B.5).
- **UI**: CLAUDE.md B.6 binds the implementation — semantic colour only, computed
  contrast, glass surface per ADR-0022, no external resources, no build step.
- **Git**: Conventional Commits; one logical change per branch and commit;
  version raised once per release before the push; no Claude attribution
  anywhere (B.7).
- **Dependencies**: a new dependency needs one line of justification; `cargo deny`
  runs in CI and no GPL-incompatible licence or open RUSTSEC advisory is
  accepted (B.2).

## Key Decisions

The ADR set in `docs/adr/` is the source. Full text and consequences live there;
the entries below are the locked constraints that planning must not contradict.
Where an ADR's own status line is not "accepted", that is stated explicitly.

<decisions>
### Resolution and architecture

- **D-01:** Every hard-to-reverse or externally surprising decision is written as a short, numbered ADR in `docs/adr/`; an ADR is never edited, only replaced by a new one. (locked, ADR-0001)
- **D-02:** DNS messages are parsed and serialized exclusively with `hickory-proto`; everything above it is our code, `hickory-server` is not used, and v1 does no recursion. (not locked as a whole: accepted in substance, but its status line says partly superseded by D-04; ADR-0002)
- **D-03:** v1 is a forwarder; resolution sits behind `ResolveBackend` with exactly one implementation; no root hints, recursion keys or other single-implementation traits are built in advance. (locked, ADR-0003)
- **D-04:** Upstream transports come from `hickory-net` (DoT/DoH/DoQ), not `hickory-resolver`; connection lifecycle, selection, failure detection, cache and privacy stay ours; use the `-ring` crypto variants. (locked, ADR-0007)
- **D-05:** The TSIG panic in `hickory-proto 0.26.1` is a documented known crash; acceptance is proven by a five-minute fuzz run in the delivery configuration (release profile, overflow checks off), the case stays in `fuzz/known-crashes/`, no upstream issue is opened, and 0.27 is awaited without moving to an alpha. (locked, ADR-0006)
- **D-06:** `split_by_zone` is the only upstream strategy; `fastest` and `round_robin` are removed; the EWMA remains only for metrics and status display. (locked, ADR-0011)
- **D-07:** `fanout` is removed; exactly one upstream is queried per request; the pool walks `split_by_zone` order and stops at the first success; the trace mutex is gone and `Ctx` is passed as `&mut`. (locked, ADR-0012)
- **D-08:** The registrable domain for `split_by_zone` comes from the compiled-in Public Suffix List; the seed rotates every 24 hours by default via `seed_rotation`, and `"0s"` disables rotation. (locked, ADR-0018)
- **D-09:** Failed resolutions are cached per `(name, type, class)` for 5 s, doubling to 60 s, capped at 1024 entries; a stale answer wins over the failure memory; there is no configuration key. (locked, ADR-0027)

### Filtering and policy

- **D-10:** Blocklist matching stays a `HashMap` with suffix lookup; Bloom filter, label reversal and inverted trie are not built; memory footprint, not latency, is the reversal criterion. (locked, ADR-0008)
- **D-11:** Block mode `refused` is removed; `nxdomain` (default), `zero_ip` and `sinkhole` remain; no block mode produces REFUSED. (locked, ADR-0013)
- **D-12:** The Adblock and RPZ parsers are removed; `hosts`, `domains` and `wildcard` remain; old format names are recognized with an explanation pointing to `wildcard` and `hosts`. (locked, ADR-0014)
- **D-13:** For local and forward zones the longest (most specific) match wins regardless of type; a deeper forward zone is delegated from `LocalBackend` to `ZoneRouter`; duplicate zone names are a startup error. (locked, ADR-0029)
- **D-14:** On a config rename the old key remains as a `serde(alias)` for at least one major version with a deprecation warning; there is no automatic migration and no version field; `deny_unknown_fields` stays and unknown keys get Levenshtein-based suggestions. (locked, ADR-0024)

### Privacy and logging

- **D-15:** Logging has four modes `none`, `aggregate` (default), `ring` and `full`; in `aggregate` a domain appears only from `aggregate_k` hits (default 5); counting is exact under a salted hash; the active mode is always visible in the UI. (locked, ADR-0004)
- **D-16:** DNSSEC is validated locally from compiled-in root keys with `privacy.dnssec = true` by default; a chain that does not close yields SERVFAIL; one `Bogus` record makes the whole answer `Bogus`; `Bogus` is terminal and not an upstream failure; signatures go only to clients with DO and are trimmed behind the cache; the CD bit does not disable validation. (locked, ADR-0016)
- **D-17:** Oblivious DoH uses `odoh-rs` with `reqwest`; the target key is fetched directly from `/.well-known/odohconfigs` once per process start; with ODoH every pool resolver must speak `doh://` and an `http://` proxy is a startup error; DNSSEC applies over ODoH. (locked, ADR-0017)
- **D-18:** The false `Bogus` verdict for a NODATA answer after a CNAME into another zone is accepted as a known false-alarm source; DNSSEC stays on and `Bogus` stays terminal; the class is named in `docs/OPERATIONS.md`; the state ends with hickory PR #3595 or an equivalent change. (locked, ADR-0028)

### Detection

- **D-19:** All five detectors (DGA, tunneling, rebinding, typosquat, NRD) default to `flag`, not `block`; levels are `off`/`log`/`flag`/`block`; thresholds live in code and are measured; heuristics run last; a detector without its data is `off`; `forward_zone` is exempt from rebinding. (locked, ADR-0019)
- **D-20:** Rebinding flags only addresses where a local device can sit: loopback, RFC 1918, link-local, broadcast, CGNAT 100.64/10 and benchmark 198.18/15; `0.0.0.0`, `::` and `192.0.0.0/16` are removed. (locked, ADR-0021)
- **D-21:** Proposed, not decided: rebinding would default to `block` after ADR-0021's corrected address list produced zero findings in the second observation period; `allow_zones` is the escape hatch; the code default stays `flag` until the maintainer decides. (proposed, ADR-0025)
- **D-22:** Proposed, not decided: DGA, tunneling, typosquat and NRD stay on `flag` until 1 to 2 weeks of real traffic, a false-positive review and a rate below 0.1% or an equivalent per detector; the code default stays `flag`. (proposed, ADR-0026)

### Operations and interface

- **D-23:** Above the per-client rate limit packets are dropped silently, never refused; IPv6 aggregates to /64; token buckets live in 16 sharded, mutex-protected drawers with an LRU client cap (default 8192); throttling runs before message parsing. (locked, ADR-0020)
- **D-24:** Two HTTP listeners: the token-protected API and the tokenless metrics endpoint; the same port is refused; the token is generated at startup with mode 0600 and allowed as a query parameter for SSE; the UI is compiled in via `include_str!` with no build step; Prometheus text is produced by hand; a domain name must never become a Prometheus label. (locked, ADR-0010)
- **D-25:** The UI background may carry colour without meaning and is the only place that is allowed: four deeply desaturated colour fields below the text threshold. Everything else follows B.6: four semantic colours plus `--brand`, greyscale ramp for areas, one centred 1400 px container, 8-based spacing, one glass container with 18 px radius, `data-theme` on the root, computed 4.5:1 contrast, and every surface with text is a pane. (locked, ADR-0022)
</decisions>

---
*Last updated: 2026-10-20 after brownfield ingest and initial GSD roadmap creation.*
