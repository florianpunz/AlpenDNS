# Roadmap: AlpenDNS

## Overview

This is a brownfield roadmap. Phases 0 to 7 are implemented and accepted;
phase 8 (detection heuristics) and phase 9 (operation and packaging) are
implemented, with their acceptance runs in progress on the real network.
Phase 10 is an optional, discardable stockpile. The roadmap therefore records
what exists and what remains instead of planning a greenfield build.

`docs/ROADMAP.md` remains canonical for phase state, acceptance history and
deviations; this file is the GSD planning view. The phase numbers, goals and
acceptance criteria are carried over from it, and requirements are derived from
the ADR intel in `.planning/intel/decisions.md` and those acceptance criteria.

## Phases

**Phase Numbering:**
- Integer phases (1, 2, 3): planned milestone work.
- Phase 0 is the pre-existing scaffolding phase from `docs/ROADMAP.md`.
- Decimal phases (2.1, 2.2): urgent insertions after planning, via `/gsd-phase --insert`.
- Phase 10 is optional and has no order or obligation.

- [x] **Phase 0: Scaffolding** - Workspace, fmt/clippy, cargo-deny and CI; the Definition of Done is green locally and in CI.
- [x] **Phase 1: A server that answers** - UDP/TCP forwarding to one upstream with answer validation, strict config and graceful shutdown.
- [x] **Phase 2: Cache** - TTL-clamped LRU cache with query coalescing, serve-stale, prefetch and a failure cache.
- [x] **Phase 3: Encrypted upstreams** - DoT/DoH/DoQ, `split_by_zone`, passive health, forward zones and privacy basics.
- [x] **Phase 4: Blocklists** - hosts/domains/wildcard parsing, a RuleRef matcher, allowlists, block modes and atomic updates.
- [x] **Phase 5: Clients and policies** - Per-client policies, schedules, temporary exemptions, regex rules and one explain path for CLI and API.
- [x] **Phase 6: Visibility: API, metrics, web UI** - Token-protected API, Prometheus metrics, four log modes, SSE and the self-served glass UI.
- [x] **Phase 7: Privacy build-out** - PSL-backed `split_by_zone` with seed rotation, privacy budget, local DNSSEC, ODoH and k-anonymity.
- [ ] **Phase 8: Heuristics without a cloud** - Five local detectors, all defaulting to `flag`; the acceptance observation period is running.
- [ ] **Phase 9: Operation and packaging** - systemd unit, `.deb`, rate limiting and safe defaults; the fresh-VM acceptance is outstanding.
- [ ] **Phase 10: Optional stockpile** - Discardable items from `docs/ROADMAP.md` and `docs/TODOS.md`, item by item, no obligation.

## Phase Details

### Phase 0: Scaffolding
**Goal**: A repository in which `cargo test` runs and CI is green.
**State**: Complete and accepted; the Definition of Done runs locally and in GitHub Actions.
**Depends on**: Nothing (first phase)
**Requirements**: SETUP-01, SETUP-02, SETUP-03, SETUP-04
**Success Criteria** (what must be TRUE):
  1. The workspace builds and `cargo test` runs.
  2. `cargo fmt --all --check` and `cargo clippy --all-targets --all-features -- -D warnings` are green.
  3. `cargo deny check` runs with `deny.toml` without errors.
  4. CI runs the four Definition-of-Done commands on every push to GitHub.
**Plans**: None — implemented in the pre-GSD history documented in `docs/ROADMAP.md`.

### Phase 1: A server that answers
**Goal**: A client gets correct answers over UDP and TCP while the server forwards to one configured upstream; no cache, no filter, no policy.
**State**: Complete; accepted on 2026-08-29 with the documented TSIG deviation (ADR-0006).
**Depends on**: Phase 0
**Requirements**: CORE-01, CORE-02, CORE-03, CORE-04, CORE-05, CORE-06, CORE-07, DEV-01
**Success Criteria** (what must be TRUE):
  1. `dig` over UDP and TCP returns correct answers from the configured upstream on a configurable port.
  2. An answer that does not match the question (ID, QNAME case-insensitive, QTYPE, QCLASS) is discarded.
  3. An oversized UDP answer sets TC, and the same query over TCP returns the full answer.
  4. A typo in a config key aborts startup with a clear message.
  5. SIGTERM/SIGINT finish an in-flight query and exit 0.
  6. A five-minute fuzz run on the request path finds no crash in the delivery configuration; the known TSIG panic is documented and tracked.
**Plans**: None — implemented in the pre-GSD history.

### Phase 2: Cache
**Goal**: Repeated queries come from memory and no longer reach the upstream.
**State**: Complete; accepted on 2026-08-29.
**Depends on**: Phase 1
**Requirements**: CACHE-01, CACHE-02, CACHE-03, CACHE-04, CACHE-05, CACHE-06, CACHE-07
**Success Criteria** (what must be TRUE):
  1. A repeated query corpus produces cache hits, and the upstream sees each name exactly once.
  2. TTLs are clamped, negative answers are cached separately (RFC 2308), and no TTL grows beyond its value at insert.
  3. The cache stays within `max_entries`, and memory does not grow linearly as new names arrive.
  4. 100 simultaneous identical queries produce exactly one upstream request.
  5. With the upstream off, an expired entry is still served; prefetch refreshes at 85% of the TTL without a client waiting.
  6. Hit, stale-hit and miss counts appear in the log while traffic exists and on shutdown, without names.
  7. A failed resolution is remembered for 5 s doubling to 60 s, capped at 1024 entries, and a stale answer wins over that memory.
**Plans**: None — implemented in the pre-GSD history.

### Phase 3: Encrypted upstreams
**Goal**: No cleartext DNS to the outside, with several upstreams behind a deterministic selection strategy.
**State**: Complete; accepted on 2026-08-29. `udp://` in an upstream pool is a startup error.
**Depends on**: Phase 2
**Requirements**: UPSTREAM-01, UPSTREAM-02, UPSTREAM-03, UPSTREAM-04, UPSTREAM-05, UPSTREAM-06, UPSTREAM-07
**Success Criteria** (what must be TRUE):
  1. The process has no cleartext DNS connection to the outside; only DoT, DoH or DoQ leave the host for non-`forward_zone` names.
  2. DoT, DoH and DoQ work, and every transport returns the client's query ID and question.
  3. Exactly one upstream is queried per request, chosen by `split_by_zone`; the removed strategy names produce a clear config error.
  4. A dead upstream is skipped after three failures, recovers after its block, and resolution is still attempted when all are down.
  5. A `forward_zone` query goes in cleartext to the LAN server while everything else stays encrypted.
  6. ECS is stripped and padding, cookies and 0x20 are applied per pool.
**Plans**: None — implemented in the pre-GSD history.

### Phase 4: Blocklists
**Goal**: Import lists, match, block and keep them current; from here AlpenDNS can replace a Pi-hole on the local network.
**State**: Complete; accepted on 2026-08-29.
**Depends on**: Phase 3
**Requirements**: FILTER-01, FILTER-02, FILTER-03, FILTER-04, FILTER-05, FILTER-06, FILTER-07, FILTER-08
**Success Criteria** (what must be TRUE):
  1. `hosts` lists with comments, CRLF, IPv6 lines and garbage lines load without failing; `domains` and `wildcard` names normalize correctly.
  2. A matched name is answered in the configured mode, and `notexample.com` is never blocked because `example.com` is on a list.
  3. An allowlisted domain is let through even when a blocklist contains it.
  4. A second fetch of an unchanged list returns 304 and is not reprocessed; an update swaps atomically with no error answers or latency spike.
  5. A first start with an unreachable list aborts; a later outage keeps the last cached version and shouts in the log.
  6. Two million entries load, p99 cache-hit latency stays under 1 ms, and RSS is documented.
**Plans**: None — implemented in the pre-GSD history.

### Phase 5: Clients and policies
**Goal**: No longer one rule for everyone: different devices get different rules, including time windows.
**State**: Complete; accepted on 2026-08-29.
**Depends on**: Phase 4
**Requirements**: POLICY-01, POLICY-02, POLICY-03, POLICY-04, POLICY-05, POLICY-06
**Success Criteria** (what must be TRUE):
  1. Two source IPs under different policies receive different verdicts for the same name.
  2. The policy model evaluates temporary grants, allowlists, blocklists, regex rules and schedules in a defined order.
  3. `alpendns policy test <domain> --client <name>` prints the verdict and the complete chain without reading the log.
  4. A temporary exemption expires after its TTL, and the block returns.
  5. A pathological regex finishes in bounded time.
  6. A scheduled policy gives different answers at two simulated times.
**Plans**: None — implemented in the pre-GSD history.

### Phase 6: Visibility: API, metrics, web UI
**Goal**: The operator sees what the server does without logging in, and the page works without internet access.
**State**: Complete; accepted on 2026-08-31 (including the human review of the rendered UI).
**Depends on**: Phase 5
**Requirements**: OBS-01, OBS-02, OBS-03, OBS-04, OBS-05, OBS-06, OBS-07, OBS-08, OBS-09, OBS-10, OBS-11
**Success Criteria** (what must be TRUE):
  1. `/api/status` without a token returns 401; with a token the API serves status, statistics, lists, policies, exemptions, recent queries, top names, history and explain.
  2. `/metrics` exposes counters without names, and only allowlisted label keys appear.
  3. In `none` and `aggregate` modes no query name appears anywhere in the process output; the automated proof runs with every `cargo test`.
  4. The UI loads with the network disconnected, makes no third-party requests, and answers "Is it running?", "What was blocked?" and "Why?" without a click.
  5. A blocked name's explanation shows list, rule, policy and timestamp and matches `alpendns policy test`.
  6. The SSE stream is capped at 25 messages per second and reports skipped events; the 24-hour history contains counters only.
**Plans**: None — implemented in the pre-GSD history.
**UI hint**: yes

### Phase 7: Privacy build-out
**Goal**: The mechanisms that distinguish AlpenDNS from "Pi-hole with DoH".
**State**: Complete; accepted on 2026-08-31. The no-name test runs permanently in CI.
**Depends on**: Phase 6
**Requirements**: PRIVACY-01, PRIVACY-02, PRIVACY-03, PRIVACY-04, PRIVACY-05, PRIVACY-06, DEV-02
**Success Criteria** (what must be TRUE):
  1. `split_by_zone` uses the compiled-in PSL, a 10,000-domain test keeps each upstream within 5%, and the seed rotates every 24 hours.
  2. Valid DNSSEC chains pass, invalid or missing signatures become SERVFAIL, and signatures go only to clients that asked with DO.
  3. ODoH traffic through a local proxy contains no labels of the query name, and DNSSEC applies over ODoH.
  4. Domains below the k threshold appear in no API response, while below-threshold sums are still reported.
  5. The privacy strip shows the mode, the k threshold and whether anything is written to disk.
  6. The automated test that greps all process output for a query name runs in CI.
**Plans**: None — implemented in the pre-GSD history.
**UI hint**: yes

### Phase 8: Heuristics without a cloud
**Goal**: Detection of patterns no list knows: everything local, everything explainable, everything by default only `flag`.
**State**: Implemented on 2026-08-30 with all seven steps. Acceptance is running: the first observation period was evaluated on 2026-09-16 (30.08. to 15.09., 220,659 queries, 4,025 distinct names); the second runs since 2026-09-16 16:24 with a filled `protect` list. Tunneling still needs a controlled test; NRD stays off without data from AlpenShield.
**Depends on**: Phase 7
**Requirements**: DETECT-01, DETECT-02, DETECT-03, DETECT-04, DETECT-05, DETECT-06, DETECT-07, DETECT-08, DETECT-09
**Success Criteria** (what must be TRUE):
  1. Each detector reports a score and a human-readable reason in the trace; all five default to `flag` and block nothing.
  2. Rebinding flags only reachable private address ranges and exempts `forward_zone` names.
  3. Tunneling and DGA stay under 0.1% false positives on the top-100k corpus; typosquat never flags the originals; NRD works from a local file and a missing file is not a startup error.
  4. The "Flagged" panel shows detector, score and reason, with one click to allow and one to block.
  5. One week of real-network operation with all detectors on `flag` has produced a reviewed false-positive list, and no detector moves to `block` before that review.
**Plans**: TBD — the remaining acceptance work can be planned.
**UI hint**: yes

### Phase 9: Operation and packaging
**Goal**: Installed and hardened on a fresh Debian VM in five minutes, then proven as the only resolver in the LAN for a week.
**State**: Implemented on 2026-08-30 with all eight steps; `systemd-analyze security` reports 1.5. Installation was verified on an Ubuntu container, not yet on a fresh Debian VM. The practical test runs since 2026-08-30 and shares its observation window with phase 8.
**Depends on**: Phase 8
**Requirements**: OPS-01, OPS-02, OPS-03, OPS-04, OPS-05, OPS-06, OPS-07, OPS-08, OPS-09, OPS-10
**Success Criteria** (what must be TRUE):
  1. The service runs as an unprivileged user with `CAP_NET_BIND_SERVICE` on port 53, and `systemd-analyze security alpendns` stays under 3.0.
  2. `alpendns check` as `ExecStartPre` refuses a broken config while the old instance keeps running, and needs no network.
  3. The `.deb` installs on a fresh Debian VM and the service starts with no manual follow-up work.
  4. One client above the rate limit is dropped while other clients are unaffected; IPv6 is keyed by /64.
  5. A fresh installation listens on every IPv4 interface, is reachable from the LAN without an edit, warns about being an open resolver on a host with a public address, and keeps per-client rate limiting on (ADR-0031).
  6. The server runs a week as the only LAN resolver without complaints, and `docs/BENCHMARKS.md` plus `docs/OPERATIONS.md` are sufficient for a second person to install and operate it.
**Plans**: TBD — the remaining acceptance work can be planned.

### Phase 10: Optional stockpile
**Goal**: Optional improvements that are taken up only when wanted; discardable at any time without weakening accepted v1 behavior.
**State**: Not scheduled. The stockpile lives in `docs/ROADMAP.md` phase 10 and in `docs/TODOS.md` (recursion behind `ResolveBackend`, DDR/DNR, blocklist diff review, two instances, local-zone build-out, own parser as a learning project, plus the open TODOS items).
**Depends on**: Nothing mandatory; every item starts from the accepted phases 0 to 9.
**Requirements**: None — optional by design.
**Success Criteria** (what must be TRUE):
  1. Work is taken item by item from `docs/TODOS.md` or `docs/ROADMAP.md`, each with its own finding, design and verify plan.
  2. Nothing in the stockpile regresses accepted v1 behavior; the four Definition-of-Done commands stay green.
  3. Recursion, if ever built, sits behind `ResolveBackend` without pre-built unused structures (ADR-0003).
**Plans**: TBD — one item at a time.

## Progress

**Execution Order:**
Phases execute in numeric order: 0 → 1 → 2 → 3 → 4 → 5 → 6 → 7 → 8 → 9.
Phase 10 has no order and no obligation.

| Phase | Plans Complete | Status | Completed |
|-------|----------------|--------|-----------|
| 0. Scaffolding | — | Complete | 2026-08-29 |
| 1. A server that answers | — | Complete | 2026-08-29 |
| 2. Cache | — | Complete | 2026-08-29 |
| 3. Encrypted upstreams | — | Complete | 2026-08-29 |
| 4. Blocklists | — | Complete | 2026-08-29 |
| 5. Clients and policies | — | Complete | 2026-08-29 |
| 6. Visibility: API, metrics, web UI | — | Complete | 2026-08-31 |
| 7. Privacy build-out | — | Complete | 2026-08-31 |
| 8. Heuristics without a cloud | — | Implemented; acceptance in progress | — |
| 9. Operation and packaging | — | Implemented; acceptance in progress | — |
| 10. Optional stockpile | — | Not scheduled | — |
