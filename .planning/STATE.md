---
gsd_state_version: "1.0"
current_phase: 10
current_phase_name: closed; phase 10 stockpile discarded, phases 0 to 9 accepted
status: complete
stopped_at: Phase 10 closed; wizard and two-instance plan discarded, item 3 finished
last_updated: "2026-10-31T12:00:00.000Z"
last_activity: 2026-10-31
last_activity_desc: "Closed phase 10: configuration wizard and two-instance plan discarded, item 3 finished"
state_head: 281aa96a38ccc62f6449535c2ad1c741be52fcd1
progress:
  total_phases: 11
  completed_phases: 11
  total_plans: 0
  completed_plans: 0
  percent: 100
---

# Project State

## Project Reference

See: .planning/PROJECT.md (updated 2026-10-20)

**Core value:** Queries are filtered on the operator's own machine and forwarded
encrypted: no query name leaves the process unless the configured logging mode
explicitly allows it.
**Current focus:** v1 accepted — phases 0 to 9 complete; phase 10 closed on
2026-10-31 with the stockpile discarded.

## Current Position

Phase: 10 of 11 (Optional stockpile) — closed 2026-10-31; phases 0 to 9 accepted
Plan: none — implementation is complete
Status: v1 accepted on 2026-10-22; phase 10 closed without plans (wizard and
two-instance plan discarded, item 3 finished)
Last activity: 2026-10-31 - Closed phase 10: configuration wizard and two-instance plan discarded, item 3 finished

Progress: [██████████] 100% — phases 0 to 9 accepted, phase 10 closed as
discarded.

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

Decisions are logged in PROJECT.md as D-01 to D-27. D-01 to D-25 come from the
ADR intel in `.planning/intel/decisions.md`; D-26 and D-27 come from ADR-0009
and ADR-0023, which the ingest manifest excluded to keep the cross-reference
graph acyclic. Full text and consequences live in `docs/adr/`.

Recent decisions affecting current work:
- D-26 (ADR-0009) and D-27 (ADR-0023): the trace carries names rather than IDs,
  and local zones are a static table with no dynamic loader. Both were verified
  in the code on 2026-10-20 and constrain later phases even though their ADRs
  were outside the ingest manifest.
- D-19 (ADR-0019): all five detectors default to `flag` and block nothing.
- D-21 and D-22 (ADR-0025, ADR-0026): both are proposed, not decided; the code
  default stays `flag` until the maintainer decides after the false-positive
  review.
- D-18 (ADR-0028): the false `Bogus` for a CNAME crossing into NODATA is a
  known, accepted false-alarm source until hickory PR #3595 or equivalent.
- D-05 (ADR-0006): the TSIG panic is a known crash in the delivery
  configuration; move only with a stable `hickory-proto 0.27`.

### Pending Todos

- Phase 8: accepted 2026-10-22; NRD remains unevaluated without data from
  AlpenShield, and the decision on ADR-0025/ADR-0026 stays proposed.
- Phase 10: closed 2026-10-31. Item 3 finished; the configuration wizard
  (item 9) and the two-instance plan (item 10) are discarded. The two remaining
  TODOS gaps — per-list `refresh` (item 3, step 5) and the block-page search
  field (item 8) — are discarded as well. The remaining bullets in
  `docs/ROADMAP.md` phase 10 are optional and not planned for v1.

### Blockers/Concerns

- No blocker for the roadmap. Phases 0 to 9 are accepted; the open points are
  the documented deviations and the concerns below.
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

### Quick Tasks Completed

| # | Description | Date | Commit | Status | Directory |
|---|-------------|------|--------|--------|-----------|
| 261022-s4q | RFC 6303 defaults for the private reverse zones | 2026-10-22 | dc57b43 | Verified | [261022-s4q-rfc-6303-defaults-for-the-private-revers](./quick/261022-s4q-rfc-6303-defaults-for-the-private-revers/) |
| 261022-t0a | A blocked-by-AlpenDNS page for sinkholed names | 2026-10-22 | 6428402 | Verified | [261022-t0a-add-a-blocked-by-alpendns-local-website-](./quick/261022-t0a-add-a-blocked-by-alpendns-local-website-/) |
| 261030-oj8 | WR-05: explain stops counting, keeps the detectors | 2026-10-30 | 67835dd | Unverified | [261030-oj8-fix-wr-05-policy-explain-must-not-mutate](./quick/261030-oj8-fix-wr-05-policy-explain-must-not-mutate/) |
| 261030-p0g | TODOS #4: clock dependency closed as already documented | 2026-10-30 | 58e9c3c | Verified | [261030-p0g-system-clock-order-after-time-sync-a-tro](./quick/261030-p0g-system-clock-order-after-time-sync-a-tro/) |
| 261030-p4w | TODOS #3: age display closed, refresh scheduler split out | 2026-10-30 | 6eb79bc | Verified | [261030-p4w-todos-3-close-the-blocklist-age-item-spl](./quick/261030-p4w-todos-3-close-the-blocklist-age-item-spl/) |
| 261030-pn3 | TODOS audit: every item reconciled with the tree | 2026-10-30 | 281aa96 | Verified | [261030-pn3-todos-audit-reconcile-every-item-status-](./quick/261030-pn3-todos-audit-reconcile-every-item-status-/) |

## Deferred Items

Items acknowledged and deferred, most recent first:

| Category | Item | Status | Deferred At | Milestone |
|----------|------|--------|-------------|-----------|
| Scope | Encrypted client listeners (DoT/DoH/DoQ) | Not implemented; client traffic is cleartext UDP/TCP | Pre-GSD | v1 |
| Scope | Forward-zone failover | Done 2026-10-22 (ADR-0030, TODOS item 13) | Pre-GSD | v1 |
| Scope | RFC 6303 built-in local zones | Done 2026-10-22 (ADR-0032, quick task 261022-s4q) | Pre-GSD | v1 |
| Scope | Recursion, DDR/DNR, blocklist diff review, local-zone build-out, own parser as learning project | Optional, not planned for v1 | 2026-10-20 | Phase 10 (closed) |
| Config | Per-list `refresh` is honoured only as the minimum across all lists — accepted 2026-10-31, a per-list schedule is not planned; EDNS `padding` and `doh_token` are documented but unimplemented | Open (CONCERNS.md) | Pre-GSD | unassigned |

## Session Continuity

Last session: 2026-10-31
Stopped at: Phase 10 closed. The configuration wizard and the two-instance
plan were discarded; item 3 was finished with the update-failure counter.
Resume file: None — the project is complete for v1.
