---
phase: 261022-s4q
plan: 01
subsystem: resolver
tags: [dns, rfc6303, reverse-zones, privacy, local-zones, nxdomain]

requires:
  - phase: 9
    provides: "LocalBackend chain (Policy → Cache → Local → ZoneRouter → Pool) and the closed-zone NXDOMAIN shape"
  - phase: "ADR-0029"
    provides: "nested-forward precedence and the configured-only overlap rule in Config::validate"

provides:
  - "Built-in RFC 6303 §4 reverse-zone table (33 names) answered NXDOMAIN locally"
  - "Configured local/forward zone precedence over the built-in default"
  - "Config::validate unchanged; existing configurations still start"
  - "Operator migration note and ADR-0032"

affects: [phase 10 optional stockpile, docs/TODOS.md item 11]

actuals:
  tokens: 8300
  tasks: 3
  commits: 2

tech-stack:
  added: []
  patterns:
    - "Static, compile-time table consulted on the hot path with Name::zone_of; no new dependency"
    - "Override gate reads the unfiltered forward list captured before the constructor's retain"

key-files:
  created:
    - docs/adr/0032-rfc-6303-eingebaut.md
  modified:
    - crates/alpendns/src/local.rs
    - crates/alpendns/src/config.rs
    - crates/alpendns/tests/local_zones.rs
    - docs/OPERATIONS.md
    - docs/TODOS.md

key-decisions:
  - "NXDOMAIN without a synthetic SOA — no authority claim and, per RFC 2308 §5, not negatively cached"
  - "A configured local or forward zone always wins, read from configured_forward (before retain), not the retained list"
  - "Config::validate stays configured-only so existing configurations keep starting"
  - "No switch to disable the default"

patterns-established:
  - "Built-in defaults never enter the configured vectors; they are consulted as a fallback in LocalBackend::resolve"

requirements-completed: [TODO-11]

coverage:
  - id: D1
    description: "A PTR query inside an RFC 6303 §4 zone is answered NXDOMAIN locally and never reaches the upstream (IPv4 and IPv6)."
    requirement: TODO-11
    verification:
      - kind: unit
        ref: "crates/alpendns/src/local.rs#rfc6303_the_table_has_33_parseable_zones"
        status: pass
      - kind: unit
        ref: "crates/alpendns/src/local.rs#rfc6303_a_private_reverse_name_is_answered_nxdomain_without_asking_anyone"
        status: pass
      - kind: unit
        ref: "crates/alpendns/src/local.rs#rfc6303_an_ipv6_private_reverse_name_is_answered_locally"
        status: pass
      - kind: integration
        ref: "crates/alpendns/tests/local_zones.rs#private_reverse_names_answer_nxdomain_and_never_reach_the_upstream"
        status: pass
      - kind: integration
        ref: "crates/alpendns/tests/local_zones.rs#private_reverse_names_outside_the_list_still_reach_the_upstream"
        status: pass
    human_judgment: false
  - id: D2
    description: "A configured [[local_zone]] (open or closed) and a configured [[forward_zone]] (nested or not) win over the built-in; label boundaries and the 172.16–31 edges hold."
    requirement: TODO-11
    verification:
      - kind: unit
        ref: "crates/alpendns/src/local.rs#override_a_non_nested_forward_zone_beats_the_builtin"
        status: pass
      - kind: unit
        ref: "crates/alpendns/src/local.rs#override_an_open_local_zone_keeps_its_upstream_fallback"
        status: pass
      - kind: unit
        ref: "crates/alpendns/src/local.rs#override_a_closed_local_zone_answers_with_its_own_label"
        status: pass
      - kind: unit
        ref: "crates/alpendns/src/local.rs#override_the_172_edges_are_16_through_31"
        status: pass
      - kind: unit
        ref: "crates/alpendns/src/local.rs#override_suffix_matching_respects_label_boundaries"
        status: pass
      - kind: unit
        ref: "crates/alpendns/src/local.rs#override_a_class_other_than_in_is_passed_through"
        status: pass
      - kind: unit
        ref: "crates/alpendns/src/local.rs#override_the_builtin_answer_is_not_cached"
        status: pass
      - kind: integration
        ref: "crates/alpendns/tests/local_zones.rs#private_reverse_configured_forward_zone_wins_over_the_builtin"
        status: pass
    human_judgment: false
  - id: D3
    description: "Config::validate keeps seeing only configured entries; a config with forward zones inside RFC 6303 space still validates."
    requirement: TODO-11
    verification:
      - kind: unit
        ref: "crates/alpendns/src/config.rs#a_forward_zone_inside_rfc6303_space_still_validates"
        status: pass
      - kind: unit
        ref: "crates/alpendns/src/config.rs#the_same_zone_as_local_and_forward_is_a_startup_error"
        status: pass
    human_judgment: false
  - id: D4
    description: "OPERATIONS.md documents the built-in default and the no-change migration; ADR-0032 records the decision; TODOS item 11 is marked done."
    requirement: TODO-11
    verification:
      - kind: other
        ref: "test -f docs/adr/0032-rfc-6303-eingebaut.md && grep -q ADR-0032 docs/OPERATIONS.md && grep -q '^## 11\\. RFC 6303 defaults for the private reverse zones — done\\.' docs/TODOS.md"
        status: pass
    human_judgment: true
    rationale: "The automated check proves the files and anchors exist, not that the prose describes the implemented behaviour correctly; a human read is the acceptance step."

duration: 25min
completed: 2026-10-22
status: complete
plan_head_before: 61260ef6e6d9ca0693f3239151b242c936cfe16b
plan_head_after: 54e8aa21462669b5ee4645789b5f7ec922213723
commits: 2
---

# Quick Task 261022-s4q: RFC 6303 defaults for the private reverse zones — Summary

**The 33 reverse zones of RFC 6303 §4 are compiled into `LocalBackend`: a private PTR answer is NXDOMAIN, never asked outward, and configured zones still win.**

## Performance

- **Duration:** ~25 min
- **Completed:** 2026-10-22
- **Tasks:** 3
- **Files modified:** 6 (5 modified, 1 created)

## Accomplishments

- `RFC6303_ZONES` — the verbatim 33-name list from RFC 6303 §4.1–4.6 (25 IPv4, 8 IPv6) — is now a static table in `local.rs`, pre-parsed once in `LocalBackend::new`.
- A name inside a built-in zone answers NXDOMAIN with the same shape as a closed local zone: no SOA, no `AA`, `RA` set, `RD` mirrored, empty answer section, and therefore not negatively cached.
- The override gate consults the full forward-zone list captured before the constructor's `retain`, so a `[[forward_zone]]` without a local zone above it is not pre-empted by the built-in; an open configured `[[local_zone]]` also keeps its upstream fallback.
- `Config::validate` is untouched: built-ins never enter `Config::local_zone`/`forward_zone`, so a configuration that started before this change still starts, and the ADR-0029 same-zone error stays.
- OPERATIONS.md now documents the built-in default instead of asking the operator to copy 14 of 33 blocks; ADR-0032 records the decision; TODOS item 11 is marked done.

## Task Commits

Each task's code and documentation was committed atomically:

1. **Tasks 1–2: built-in table, gated NXDOMAIN branch, override and validation tests** — `ab07850` (feat)
2. **Task 3: migration note, ADR-0032, TODOS bookkeeping** — `54e8aa2` (docs)

_Note: tasks 1 and 2 share one commit. The override gate is part of the same branch code the tracer task established, and CLAUDE.md B.7 ("one commit = one logical change; can one be reverted without taking the other along?") puts the implementation and the tests that pin it together._

## Files Created/Modified

- `crates/alpendns/src/local.rs` — `RFC6303_ZONES`, `configured_forward`/`builtin` fields, the gated NXDOMAIN branch, 3 `rfc6303_*` and 7 `override_*` unit tests.
- `crates/alpendns/src/config.rs` — `a_forward_zone_inside_rfc6303_space_still_validates` regression test; `validate_local_zones` unchanged.
- `crates/alpendns/tests/local_zones.rs` — three end-to-end tests through the real Policy → Cache → Local → (ZoneRouter →) Forward chain.
- `docs/OPERATIONS.md` — the section now describes the built-in default, the override rule and the migration; the copy-paste block is gone, the nested-forward `/24` example stays.
- `docs/adr/0032-rfc-6303-eingebaut.md` — the decision record.
- `docs/TODOS.md` — item 11 marked done, finding framed as historical, `**Built.**` paragraph added.

## Decisions Made

- **NXDOMAIN without a synthetic SOA.** Locked by CONTEXT.md: it matches `LocalZone::is_closed`, avoids an authority claim, and keeps the answer out of the negative cache (RFC 2308 §5).
- **Override reads `configured_forward`, not `forward_zones`.** The constructor's `retain` drops every forward entry without a local zone above it; using the retained list would let the built-in answer over a legitimate forward zone.
- **`local_zone.is_none()` guards the built-in** so an open (fallback = upstream) configured zone is not silently closed.
- **No config key and no dependency.** `deny_unknown_fields` and `deny.toml` untouched.

## Deviations from Plan

None — plan executed exactly as written. The only judgement call was the commit split (see Task Commits): tasks 1 and 2 are one logical change under CLAUDE.md B.7.

## Issues Encountered

None. `cargo fmt --all` reformatted two assertions after the first test run; no logic changed.

## Threat Model Follow-up

- T-s4q-01 (information disclosure): closed — `rfc6303_*` unit tests and the `private_reverse_*` integration tests pin zero upstream hits.
- T-s4q-02 (gate shadowing a configured zone): closed — `override_a_non_nested_forward_zone_beats_the_builtin` and its integration counterpart.
- T-s4q-03 (validation regression): closed — `a_forward_zone_inside_rfc6303_space_still_validates`; same-zone error test still green.
- T-s4q-04 (matching/injection): `Name::zone_of` only; no `unwrap`/`expect`/`panic`/indexing outside `#[cfg(test)]`.
- T-s4q-05 (trace/logging): only `Step::LocalAnswer` with a constant label; query names still leave solely through `logging::QueryLog`.
- T-s4q-SC (package installs): no new dependency, no install command.

## Definition of Done

- `cargo fmt --all --check` — pass
- `cargo clippy --all-targets --all-features -- -D warnings` — pass
- `cargo test --all-features` — pass (476 lib tests plus all integration suites; the ignored measurement runs stay ignored)
- `cargo deny check` — advisories, bans, licenses, sources all ok

## Known Stubs

None.

## Next Phase Readiness

The private reverse space is answered locally for IPv4 and IPv6, with the RFC list pinned by a count test so a typo cannot shrink it. Phase 10's TODO 11 is complete; the ADR-0029 consequence that anticipated this rule is now resolved.

## Self-Check: PASSED

All listed files exist and both task commits are ancestors of HEAD.
