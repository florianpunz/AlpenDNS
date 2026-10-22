---
phase: 261022-t0a
verified: 2026-10-22T19:59:12Z
status: human_needed
score: 8/8 must-haves verified
covered_files: [".planning/quick/261022-t0a-add-a-blocked-by-alpendns-local-website-/261022-t0a-PLAN.md", ".planning/quick/261022-t0a-add-a-blocked-by-alpendns-local-website-/261022-t0a-SUMMARY.md", "config/alpendns.example.toml", "crates/alpendns/src/api/block_page.rs", "crates/alpendns/src/api/mod.rs", "crates/alpendns/src/api/ui.rs", "crates/alpendns/src/config.rs", "crates/alpendns/src/main.rs", "crates/alpendns/src/policy/mod.rs", "docs/OPERATIONS.md", "packaging/alpendns.toml", "web/blocked.html"]
covered_digest: "v3:sha256:8b94c9a681668f44e5924824e687688f9c85eb35deb1321eea5209a5167aacdd"
behavior_unverified: 0
overrides_applied: 0
human_verification:
  - test: "Open the page in a browser with the system in light mode and then in dark mode (a listener on a high port such as 127.0.0.1:8080, a blocklisted Host)."
    expected: "One glass panel over the sky, readable in both modes, the name in monospace, the blocked verdict in the danger colour and nothing else coloured; no text on the bare sky, no second container."
    why_human: "The panel material, blur, contrast feel and the light/dark switch are visual; the static tests prove the script and the class/colour rules, not what the user sees."
  - test: "Request a name longer than 200 bytes (for example a 253-byte label) on the same listener, then a name that is not blocked."
    expected: "The long name wraps inside the panel instead of pushing the page into horizontal scroll; the neutral page shows 'No blocklist entry applies to this name.' and the tab title does not claim the name was blocked."
    why_human: "Wrapping and the absence of a scrollbar are layout outcomes of the browser; the test only pins the CSS declaration."
---

# Phase 261022-t0a: A "Blocked by AlpenDNS" page for sinkholed names — Verification Report

**Phase Goal:** A "Blocked by AlpenDNS" page for sinkholed names: an opt-in HTTP listener on port 80 serving a page in the Web UI's visual language, showing the blocked name, the fact that it was blocked on this network, and the matched blocklist name; neutral for unblocked names; light/dark following the system preference.
**Verified:** 2026-10-22T19:59:12Z
**Status:** human_needed
**Re-verification:** No — initial verification

## Goal Achievement

### Observable Truths

| # | Truth | Status | Evidence |
|---|-------|--------|----------|
| 1 | A GET whose Host names a blocklisted domain returns 200 `text/html` with `Cache-Control: no-store`, showing that name and the matched list. | ✓ VERIFIED | `block_page_answers_with_the_name_and_the_matched_list` starts a real listener on `127.0.0.1:0`, asserts status, `text/html`, `no-store`, `nosniff`, the normalized name, `ads-list`, and `class="verdict is-blocked"`. Ran green. |
| 2 | An allowlisted or unknown name gets a neutral page that names no list and says only that no blocklist entry applies. | ✓ VERIFIED | `render` (`api/block_page.rs:129-162`) emits exactly `<p class="verdict">No blocklist entry applies to this name.</p>`; `block_page_is_neutral_when_not_blocked` and the second half of `block_page_uses_the_http_peer_for_the_verdict` assert the sentence and the absence of `is-blocked`. |
| 3 | A regex/schedule/temporary-deny block gets a "blocked by a rule" page naming no list, rule or policy. | ✓ VERIFIED | `BlockCause::Rule` renders "This name is blocked by a rule on this network." `block_page_names_no_list_for_a_rule_block` uses a regex and asserts the pattern text is absent; `block_cause_returns_list_notblocked_and_rule` covers a temporary deny. `deciding_step` maps every non-blocklist step to `Rule`. |
| 4 | A page view changes no policy counter and consults no heuristic. | ✓ VERIFIED | `Effects::ReadOnly` guards every counter increment (`policy/mod.rs:316,324,349,399,449`) and the detector block (`policy/mod.rs:395`). `block_cause_leaves_the_policy_counters_unchanged` asserts `engine.stats()` unchanged; `block_cause_does_not_run_detectors` proves a blocking detector is skipped; `block_page_does_not_disclose_peer_policy_or_rule` asserts the counter stays at 13 after an HTTP request. |
| 5 | The lookup uses the HTTP peer, works without the API token, and renders no IP, policy name, rule text, counts or UI link. | ✓ VERIFIED | Handler reads `ConnectInfo<SocketAddr>` (`block_page.rs:164-190`); `block_page_uses_the_http_peer_for_the_verdict` sends from `127.0.0.2` and gets the strict list, from `127.0.0.1` the neutral page. The router has no token middleware and the tests send no header. `block_page_does_not_disclose_peer_policy_or_rule` asserts none of `127.0.0.2`, `viewer`, `strict-policy`, `strict-list`, the regex, or `13` appear. |
| 6 | A hostile Host cannot become markup; an invalid or missing Host answers 400 without echoing it. | ✓ VERIFIED | `escape` covers `& < > " '`; `fill` is single-pass. `block_page_rejects_a_missing_or_invalid_host` proves 400 for a missing Host, a `<script>` Host (not echoed) and `[::1]:80`; `block_page_escapes_a_hostile_value` and `block_page_fill_does_not_rescan_inserted_values` pin the primitives. |
| 7 | The page follows the system theme via `data-theme` set before first paint, reuses `app.css`, references nothing external, and has exactly one glass container. | ✓ VERIFIED | `web/blocked.html:8-15` sets `dataset.theme` from `matchMedia("(prefers-color-scheme: dark)")` in the `<head>`; `app.css:139` defines the dark set under `:root[data-theme="dark"]`, and `.panel` (`app.css:347-354`) is the shared glass edge. `block_page_theme_follows_the_system`, `block_page_references_nothing_external`, `block_page_uses_one_panel`, `block_page_links_nothing_to_the_api_or_ui`, `block_page_colour_is_limited_to_the_blocked_verdict` and `block_page_wraps_a_long_name` pass. Visual confirmation is a human item below. |
| 8 | Shipped config keeps `block_page.enabled = false` and `blocking.mode = nxdomain`; a non-sinkhole mode, unspecified sinkhole, or API/metrics port collision is a startup error. | ✓ VERIFIED | `packaging/alpendns.toml:144-145,220-222`, `config/alpendns.example.toml:268-274,427-429`; `Config::validate` (`config.rs:1368-1447`) rejects all four cases. `the_packaged_configuration_is_valid`, `the_packaged_configuration_listens_on_every_interface` (still exactly 2 public DNS listeners) and the five `block_page_*` config tests pass. |

**Score:** 8/8 truths verified (0 present, behavior-unverified)

### Required Artifacts

| Artifact | Expected | Status | Details |
|----------|----------|--------|---------|
| `crates/alpendns/src/policy/mod.rs` | private `Effects`, public `BlockCause`, public `block_cause` | ✓ VERIFIED | `Effects::{Count,ReadOnly}` (line 113), `BlockCause` (92), `block_cause` (160) using `evaluate_with(..., Effects::ReadOnly)` and the typed `deciding_step`. Wired from `main.rs:900`. |
| `crates/alpendns/src/api/block_page.rs` | trait, router, Host handling, escaping, rendering, tests | ✓ VERIFIED | 849 lines, 23 tests, no `tracing`, no non-test `unwrap`/`expect`/`panic!`/slice indexing. |
| `web/blocked.html` | page with `{{NAME}}` and `{{BODY}}` (now also `{{TITLE}}`) | ✓ VERIFIED | Included via `include_str!` (`block_page.rs:26`); one panel, one stylesheet link, inline theme script. |
| `crates/alpendns/src/config.rs` | `BlockPageConfig`, defaults, validation, tests | ✓ VERIFIED | Struct at 351-375, field at 87-88, validation 1368-1447, 7 tests. |
| `crates/alpendns/src/main.rs` | third listener, `BlockPageSource` impl, `check` line | ✓ VERIFIED | Listener spawn 588-608 with `into_make_service_with_connect_info`; impl 894-904; separate check line 1271-1277 and honest summary 1044-1052. |
| `config/alpendns.example.toml`, `packaging/alpendns.toml` | disabled `[block_page]` | ✓ VERIFIED | Both carry the recipe comment, `enabled = false`, `listen = "0.0.0.0:80"`; no `[PHASE n]` marker. |
| `docs/OPERATIONS.md` | opt-in recipe, HTTP-only and IPv4-only limitations | ✓ VERIFIED | "A page for blocked names" (line 182): TOML recipe, `check` + restart, HTTP-only/HSTS and IPv4-only paragraphs, firewall note, statistics note. |

### Key Link Verification

| From | To | Via | Status | Details |
|------|----|-----|--------|---------|
| `page` handler | `ConnectInfo<SocketAddr>` → `BlockPageSource::cause` → `policy::block_cause` → `deciding_step(Step::BlocklistHit)` | function calls | ✓ WIRED | `block_page.rs:164-190` → `main.rs:894-904` → `policy/mod.rs:160-184`. |
| one `Arc<Runtime>` | `Arc<dyn StatusSource>` and `Arc<dyn BlockPageSource>` | coercion | ✓ WIRED | `main.rs:523-540,595`. Block-page spawn uses `into_make_service_with_connect_info` (`main.rs:602-604`), so `ConnectInfo` does not 500. |
| `api::block_page` | `ui::STYLE` (`web/app.css`) | `super::ui::STYLE` | ✓ WIRED | `block_page.rs:192-201`; `ui.rs:16` is `pub(super)`. `block_page_serves_the_stylesheet` returns 200 `text/css` containing `--danger:`. |
| `render` | escaped values, single-pass fill | `escape` + `fill` | ✓ WIRED | Both pinned by tests; list names and names cannot re-enter the placeholder scan. |
| `public_listeners()` / packaged test | block page outside the DNS list | untouched | ✓ WIRED | `config.rs:1539-1549` still lists only DNS/API/metrics; `the_packaged_configuration_listens_on_every_interface` still expects 2. |

### Data-Flow Trace (Level 4)

| Artifact | Data Variable | Source | Produces Real Data | Status |
|----------|---------------|--------|--------------------|--------|
| `api::block_page` page | `cause: BlockCause` | `Runtime::cause` → `policy::block_cause` → live `Engine` (`matcher.lookup`, `temporary.check`, regex, schedule) | Yes — real policy engine; integration tests show the actual matched list and the neutral sentence from the same engine | ✓ FLOWING |

### Behavioral Spot-Checks

| Behavior | Command | Result | Status |
|----------|---------|--------|--------|
| Block page and read-only cause | `cargo test -p alpendns --lib block_` | 36 passed, 0 failed | ✓ PASS |
| Packaged configuration defaults | `cargo test -p alpendns --lib the_packaged_configuration` | 4 passed, 0 failed | ✓ PASS |
| `check` reachability wording | `cargo test -p alpendns --bin alpendns check_` | 3 passed, 0 failed | ✓ PASS |
| Definition of done — formatting | `cargo fmt --all --check` | exit 0 | ✓ PASS |
| Definition of done — lints | `cargo clippy --all-targets --all-features -- -D warnings` | exit 0 | ✓ PASS |
| Definition of done — tests | `cargo test --all-features` | all suites green (506 lib + 7 bin + integrations; 0 failed) | ✓ PASS |
| Definition of done — supply chain | `cargo deny check` | advisories/bans/licenses/sources ok | ✓ PASS |

The `block_*` integration tests start a real axum server on `127.0.0.1:0` and issue HTTP requests, so the listener, connect info, headers and rendered body are exercised, not merely grepped.

### Probe Execution

SKIPPED — the plan declares no probes and the task is not a migration/tooling phase.

### Requirements Coverage

| Requirement | Source Plan | Description | Status | Evidence |
|-------------|-------------|-------------|--------|----------|
| TODO-08 | `261022-t0a-PLAN.md` | Block page for sinkholed names (docs/TODOS.md item 8) | ✓ SATISFIED | Listener, page, read-only cause, config validation, shipped defaults, docs and tests as above. NOTE: TODO-08 lives in `docs/TODOS.md`, not in `.planning/REQUIREMENTS.md`, so there are no orphaned requirement IDs to cross-check for this quick task. |

### Anti-Patterns Found

| File | Line | Pattern | Severity | Impact |
|------|------|---------|----------|--------|
| — | — | No `TBD`/`FIXME`/`XXX`, no stub returns, no placeholder text, no hardcoded empty data in any changed file | — | None |
| `crates/alpendns/src/policy/mod.rs` | 123-147 | `explain` still evaluates with `Effects::Count` although its docs say nothing is counted (REVIEW.md WR-05) | ℹ️ Info | Pre-existing behaviour, explicitly not introduced by this change and outside this plan's must-haves; recorded here so it is not lost. Does not affect this task's goal. |

### Test Quality Audit

| Test File | Linked Req | Active | Skipped | Circular | Assertion Level | Verdict |
|-----------|-----------|--------|---------|----------|-----------------|---------|
| `crates/alpendns/src/api/block_page.rs` | TODO-08 | 23 | 0 | 0 | Behavioral (real listener: status, headers, body, peer) | sufficient |
| `crates/alpendns/src/config.rs` (block_page / packaged) | TODO-08 | 9 | 0 | 0 | Value (validation errors, defaults) | sufficient |
| `crates/alpendns/src/main.rs` (check verdict) | TODO-08 | 3 | 0 | 0 | Value (exact summary text) | sufficient |

**Disabled tests on requirements:** 0 → none.
**Circular patterns detected:** 0 → none.
**Insufficient assertions:** 0 → none.

### Decision Coverage

No trackable decisions in CONTEXT.md. (Gate output verbatim: `{"skipped": true, "blocking": false, "reason": "no trackable decisions", "total": 0, "honored": 0, "not_honored": [], "message": "No trackable decisions in CONTEXT.md."}`)

Manual cross-check of the five locked decisions in `261022-t0a-CONTEXT.md`: own unauthenticated listener on port 80 (yes, `main.rs:588-608`); HTTP only (no TLS code); content limited to name + local-block + matched list (yes); shipped default `nxdomain` and `enabled = false` (yes); `app.css` reused with `data-theme` from `matchMedia` in the head (yes).

### Human Verification Required

1. **Visual language in light and dark**
   **Test:** Run the listener on a high port with a blocklisted `Host`, open it with the system in light mode, then in dark mode.
   **Expected:** One glass panel over the sky, readable in both modes; the name in monospace; the blocked verdict in the danger colour and no other colour; no text on the bare sky; no second container.
   **Why human:** The material, blur, contrast feel and the actual light/dark switch are visual. The tests pin the script, the token usage and the colour confinement, not the rendered result.

2. **Long name and the neutral page**
   **Test:** Request a very long (up to 253-byte) name on the same listener, then an unblocked name.
   **Expected:** The long name wraps inside the panel and the page does not scroll horizontally; the neutral page shows "No blocklist entry applies to this name." and its tab title does not say "Blocked".
   **Why human:** Wrapping and the absence of a scrollbar are browser layout outcomes; the test only asserts the CSS declaration and the title string.

### Gaps Summary

No gaps. All eight must-have truths are backed by passing behavioral tests and by the current tree after the six review repairs (`b9fa9e4..6428402`). The four definition-of-done commands are green. The only open items are the two visual checks above, which are not automatable.

---

_Verified: 2026-10-22T19:59:12Z_
_Verifier: Claude (gsd-verifier)_
