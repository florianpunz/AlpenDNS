---
phase: 261022-t0a
plan: 01
subsystem: api
tags: [axum, http, sinkhole, blocklist, policy-trace, web-ui]

requires:
  - phase: 08
    provides: policy engine with typed decision trace and blocklists
  - phase: 05
    provides: BlockMode::Sinkhole and the sinkhole addresses
  - phase: 09
    provides: config validation, alpendns check, packaging

provides:
  - "A opt-in HTTP listener that answers a sinkholed name with a page naming the name and the matched list"
  - "policy::BlockCause and policy::block_cause with a read-only evaluation path"
  - "BlockPageConfig ([block_page]) with startup validation of mode, addresses and port collisions"
  - "web/blocked.html built from the shared app.css and the system light/dark preference"

affects:
  - docs/OPERATIONS.md
  - config/alpendns.example.toml
  - packaging/alpendns.toml

actuals:
  tokens: 14400
  tasks: 2
  commits: 2
  plan_head_before: 7c2eb5f698deeb239fcd041bff60f9bee6b6ef82
  plan_head_after: 930b10b

tech-stack:
  added: []
  patterns:
    - "Read-only evaluation switch (Effects) so a display-only lookup cannot move DNS counters or heuristic state"
    - "Typed BlockCause instead of parsing the rendered explanation"
    - "Page rendered by single-pass placeholder substitution into an include_str! template"

key-files:
  created:
    - crates/alpendns/src/api/block_page.rs
    - web/blocked.html
  modified:
    - crates/alpendns/src/policy/mod.rs
    - crates/alpendns/src/api/mod.rs
    - crates/alpendns/src/api/ui.rs
    - crates/alpendns/src/config.rs
    - crates/alpendns/src/main.rs
    - config/alpendns.example.toml
    - packaging/alpendns.toml
    - docs/OPERATIONS.md

key-decisions:
  - "The page evaluates policy with Effects::ReadOnly: no counter moves and no detector runs, so a page view cannot influence DNS statistics."
  - "The neutral sentence claims only that no blocklist entry applies, because detectors are skipped in read-only mode."
  - "The listener is IPv4-only (0.0.0.0:80); a v6 wildcard would make peers appear v4-mapped and break per-client policy matching (ADR-0031)."
  - "block_page.enabled is a startup error unless blocking.mode is sinkhole and both sinkhole addresses name a host."
  - "Host values are parsed with Name::from_str_relaxed, lowercased and escaped; a missing or invalid Host answers 400 without echoing it."

patterns-established:
  - "Display-only policy queries go through Effects::ReadOnly rather than reaching into counters."
  - "Block-page tests assert on class attributes, not bare CSS tokens, so the inline stylesheet cannot fake a verdict."

requirements-completed: [TODO-08]

coverage:
  - id: D1
    description: "A blocklisted name on the block-page listener returns 200 text/html with no-store, the name and the matched list."
    requirement: TODO-08
    verification:
      - kind: integration
        ref: "crates/alpendns/src/api/block_page.rs#block_page_answers_with_the_name_and_the_matched_list"
        status: pass
      - kind: integration
        ref: "crates/alpendns/src/api/block_page.rs#block_page_uses_the_http_peer_for_the_verdict"
        status: pass
    human_judgment: false
  - id: D2
    description: "An unblocked or rule-blocked name gets a neutral page that names no list, rule, peer or policy."
    requirement: TODO-08
    verification:
      - kind: integration
        ref: "crates/alpendns/src/api/block_page.rs#block_page_is_neutral_when_not_blocked"
        status: pass
      - kind: integration
        ref: "crates/alpendns/src/api/block_page.rs#block_page_names_no_list_for_a_rule_block"
        status: pass
      - kind: unit
        ref: "crates/alpendns/src/api/block_page.rs#block_cause_returns_list_notblocked_and_rule"
        status: pass
    human_judgment: false
  - id: D3
    description: "A page view changes no policy counter and runs no detector."
    requirement: TODO-08
    verification:
      - kind: unit
        ref: "crates/alpendns/src/api/block_page.rs#block_cause_leaves_the_policy_counters_unchanged"
        status: pass
      - kind: unit
        ref: "crates/alpendns/src/api/block_page.rs#block_cause_does_not_run_detectors"
        status: pass
    human_judgment: false
  - id: D4
    description: "A hostile Host value cannot become markup; missing or invalid Host answers 400 without echo."
    requirement: TODO-08
    verification:
      - kind: unit
        ref: "crates/alpendns/src/api/block_page.rs#block_page_escapes_a_hostile_value"
        status: pass
      - kind: integration
        ref: "crates/alpendns/src/api/block_page.rs#block_page_rejects_a_missing_or_invalid_host"
        status: pass
    human_judgment: false
  - id: D5
    description: "The page follows the system theme, reuses app.css, references nothing external, one glass panel."
    requirement: TODO-08
    verification:
      - kind: unit
        ref: "crates/alpendns/src/api/block_page.rs#block_page_theme_follows_the_system"
        status: pass
      - kind: unit
        ref: "crates/alpendns/src/api/block_page.rs#block_page_references_nothing_external"
        status: pass
      - kind: unit
        ref: "crates/alpendns/src/api/block_page.rs#block_page_uses_one_panel"
        status: pass
    human_judgment: false
  - id: D6
    description: "The listener is opt-in and the shipped configuration keeps it off with blocking.mode = nxdomain."
    requirement: TODO-08
    verification:
      - kind: unit
        ref: "crates/alpendns/src/config.rs#block_page_defaults_to_off_on_port_80"
        status: pass
      - kind: unit
        ref: "crates/alpendns/src/config.rs#the_packaged_configuration_is_valid"
        status: pass
    human_judgment: false
  - id: D7
    description: "Enabling the page with a non-sinkhole mode, an unspecified sinkhole, or a port collision is a startup error."
    requirement: TODO-08
    verification:
      - kind: unit
        ref: "crates/alpendns/src/config.rs#block_page_enabled_requires_sinkhole_mode"
        status: pass
      - kind: unit
        ref: "crates/alpendns/src/config.rs#block_page_rejects_an_unspecified_sinkhole"
        status: pass
      - kind: unit
        ref: "crates/alpendns/src/config.rs#block_page_listen_collides_with_api"
        status: pass
      - kind: unit
        ref: "crates/alpendns/src/config.rs#block_page_listen_collides_with_metrics"
        status: pass
    human_judgment: false

duration: 8min
completed: 2026-10-22
status: complete
---

# Phase 261022-t0a Plan 01: Blocked-by-AlpenDNS page Summary

**An opt-in HTTP listener on the sinkhole address that names the blocked domain and the blocklist that matched it, evaluated read-only so a page view cannot move DNS counters**

## Performance

- **Duration:** 8 min (commit span)
- **Started:** 2026-10-22T21:39:59+02:00
- **Completed:** 2026-10-22T21:44:00+02:00
- **Tasks:** 2
- **Files modified:** 10

## Accomplishments

- `policy::block_cause` answers "was this name blocked, and by which list" through a new
  `Effects::ReadOnly` path: no counters, no detector block, no mutation of the tunneling window.
  The DNS path keeps `Effects::Count` and is byte-for-byte unchanged.
- `api::block_page` serves the page on its own listener: `Host` parsed and validated, values
  escaped, 400 without echo on a bad Host, `Cache-Control: no-store`, and `app.css` from the
  existing UI assets.
- `web/blocked.html` is a single glass panel in the Web UI's language, with `data-theme` set
  from `prefers-color-scheme` before first paint; it links nothing and loads nothing external.
- `[block_page]` is off by default; enabling it requires `blocking.mode = "sinkhole"`, sinkhole
  addresses that name a host, and a port free of the API and metrics listeners.
- The shipping and example configurations document the three-step recipe and the two limits
  that cannot be configured away: HTTP only, IPv4 only (ADR-0031).

## Task Commits

1. **Task 1: policy read-only path, block page module, template and tests** — `80dc485` (feat)
2. **Task 2: configuration, listener wiring and documentation** — `930b10b` (feat)

## Files Created/Modified

- `crates/alpendns/src/policy/mod.rs` — `BlockCause`, `Effects::{Count, ReadOnly}`, `block_cause`
- `crates/alpendns/src/api/block_page.rs` — `BlockPageSource`, router, Host handling, escaping, rendering, 23 tests
- `crates/alpendns/src/api/mod.rs` — registers the block page module
- `crates/alpendns/src/api/ui.rs` — exposes `app.css` to both page modules
- `web/blocked.html` — the page, with `{{NAME}}` and `{{BODY}}`
- `crates/alpendns/src/config.rs` — `BlockPageConfig`, defaults, startup validation, 5 tests
- `crates/alpendns/src/main.rs` — third listener, `BlockPageSource for Runtime`, `check` line
- `config/alpendns.example.toml` — documented, disabled `[block_page]`
- `packaging/alpendns.toml` — documented, disabled `[block_page]`
- `docs/OPERATIONS.md` — "A page for blocked names"

## Decisions Made

- Read-only evaluation is a switch on the existing evaluator rather than a second code path;
  a duplicate predicate would drift from the one the DNS path uses.
- The neutral page says only "No blocklist entry applies to this name." A detector-blocked
  name also lands there because read-only mode skips detectors; the sentence stays true.
- The listener binds `0.0.0.0:80` rather than `[::]:80`: a v6 wildcard makes IPv4 peers appear
  v4-mapped, and `[[client]]` nets do not match those (ADR-0031).
- Port collisions with the API and metrics listeners are startup errors, matching B.1 rule 5.

## Deviations from Plan

None — plan executed exactly as written.

## Issues Encountered

- `reqwest` in the tests panicked with "No provider set" because it is built with
  `rustls-no-provider`; the test helper now calls `filter::source::install_crypto_provider()`,
  the same pattern used by `tests/observability.rs`.
- Neutral-page assertions were first written against `is-blocked`, which also appears in the
  page's inline stylesheet; they now assert on the class attribute (`class="verdict is-blocked"`).

## User Setup Required

None — the page ships disabled and needs no external service. The three-step recipe for
turning it on stands in `config/alpendns.example.toml` and `docs/OPERATIONS.md`.

## Next Phase Readiness

- The page is reachable only when an operator enables it; without `sinkhole` mode nothing changes.
- The manual smoke test from the plan was covered by the integration tests, which use a real
  loopback listener and a `Host` header (no port 80 bind needed).

---

*Phase: 261022-t0a*
*Completed: 2026-10-22*

## Self-Check: PASSED

- `crates/alpendns/src/api/block_page.rs` — FOUND
- `web/blocked.html` — FOUND
- commit `80dc485` — FOUND
- commit `930b10b` — FOUND
