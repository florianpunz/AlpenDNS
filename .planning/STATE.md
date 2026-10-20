---
gsd_state_version: '1.0'
status: executing
progress:
  total_phases: 11
  completed_phases: 8
  total_plans: 0
  completed_plans: 0
  percent: 73
---

# Project State

## Project Reference

See: .planning/PROJECT.md (updated 2026-10-20)

**Core value:** Queries are filtered on the operator's own machine and forwarded
encrypted: no query name leaves the process unless the configured logging mode
explicitly allows it.
**Current focus:** Acceptance of phases 8 and 9 on the real network.

## Current Position

Phase: 8 of 11 (Heuristics without a cloud), with 9 of 11 (Operation and
packaging) running its acceptance in the same observation window
Plan: none — implementation is complete; the remaining acceptance work is not
yet planned
Status: Verifying / acceptance in progress
Last activity: 2026-10-20 — brownfield ingest, PROJECT/REQUIREMENTS/ROADMAP/STATE
created from the ADR intel and `docs/ROADMAP.md`

Progress: 73% — 8 of 11 phases accepted (phases 8 and 9 implemented, acceptance
running; phase 10 optional).

## Performance Metrics

**Velocity:**
- Total plans completed: not tracked for the pre-GSD phases
- Average duration: not tracked
- Total execution time: not tracked

**By Phase:**

| Phase | Plans | Total | Avg/Plan |
|-------|-------|-------|----------|
| 0-7 | — | — | — |
| 8 | — | — | — |
| 9 | — | — | — |

**Recent Trend:**
- Last 5 plans: not tracked
- Trend: unknown

*The pre-GSD history and its acceptance dates are in `docs/ROADMAP.md`.*

## Accumulated Context

### Decisions

Decisions are logged in PROJECT.md as D-01 to D-25, derived from the ADR intel
in `.planning/intel/decisions.md`; full text and consequences live in
`docs/adr/`.

Recent decisions affecting current work:
- D-19 (ADR-0019): all five detectors default to `flag` and block nothing.
- D-21 and D-22 (ADR-0025, ADR-0026): both are proposed, not decided; the code
  default stays `flag` until the maintainer decides after the false-positive
  review.
- D-18 (ADR-0028): the false `Bogus` for a CNAME crossing into NODATA is a
  known, accepted false-alarm source until hickory PR #3595 or equivalent.
- D-05 (ADR-0006): the TSIG panic is a known crash in the delivery
  configuration; move only with a stable `hickory-proto 0.27`.

### Pending Todos

- Phase 8: evaluate the second observation period (started 2026-09-16 16:24);
  run a controlled tunneling test instead of another passive run; NRD remains
  unevaluated without data from AlpenShield; decide on ADR-0025 and ADR-0026
  after the false-positive review.
- Phase 9: install the `.deb` on a fresh Debian VM (the Ubuntu container run is
  a documented deviation); finish the practical week; record anything notable
  as a false-alarm list or configuration change.
- Phase 10: optional items in `docs/TODOS.md` and `docs/ROADMAP.md` phase 10.

### Blockers/Concerns

- No blocker for the roadmap. Acceptance of phases 8 and 9 depends on elapsed
  real-network time and a second machine, not on code.
- Known upstream defects: TSIG panic (ADR-0006) and false `Bogus` on
  CNAME-to-NODATA (ADR-0028). Both are accepted with documented conditions.
- Open concerns without a phase assignment are catalogued in
  `.planning/codebase/CONCERNS.md`: the shipped example config contains keys the
  parser rejects, `refresh = "0s"` reaches `tokio::time::interval`, the reload
  swaps two `ArcSwap`s non-atomically, multi-question queries are silently
  truncated, `forward_zone` targets are not checked to be on the LAN, UI
  security headers are absent, UDP task spawning is unbounded, and `QueryLog`
  takes global mutexes per request. They are not requirements until triaged;
  do not invent phase scope from them.
- Missing features are listed rather than promised: encrypted client-facing
  listeners and forward-zone failover exist only as target/backlog material;
  recursion is explicitly out of v1.

## Deferred Items

Items acknowledged and deferred, most recent first:

| Category | Item | Status | Deferred At | Milestone |
|----------|------|--------|-------------|-----------|
| Scope | Encrypted client listeners (DoT/DoH/DoQ) | Not implemented; client traffic is cleartext UDP/TCP | Pre-GSD | v1 |
| Scope | Forward-zone failover | Open (TODOS item 13) | Pre-GSD | v1 |
| Scope | RFC 6303 built-in local zones | Open (TODOS item 11) | Pre-GSD | v1 |
| Scope | Recursion, DDR/DNR, blocklist diff review, two instances, local-zone build-out, own parser as learning project | Optional stockpile | 2026-10-20 | Phase 10 |
| Config | Per-list `refresh` is parsed but ignored; `max_inflight`, EDNS `padding`, `doh_token` are documented but unimplemented | Open (CONCERNS.md) | Pre-GSD | unassigned |

## Session Continuity

Last session: 2026-10-20
Stopped at: Brownfield GSD initialization — PROJECT.md, REQUIREMENTS.md,
ROADMAP.md and STATE.md written from the intel and `docs/ROADMAP.md`.
Resume file: None — the next action is a fresh planning pass over the remaining
phase 8/9 acceptance work, not a restored session.
