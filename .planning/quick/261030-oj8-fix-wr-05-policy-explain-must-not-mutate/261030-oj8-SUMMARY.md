---
phase: 261030-oj8
plan: 01
subsystem: policy
tags: [policy, explain, api, counters, detectors, docs]

requires:
  - phase: 05
    provides: policy engine with blocklists, allowlists, temporary grants and counters
  - phase: 08
    provides: name detectors and the tunneling window
  - phase: 261022-t0a
    provides: block_cause and the first read-only evaluation switch (Effects::ReadOnly)

provides:
  - "Effects as two independent axes (count, detectors) with three named modes: DNS, EXPLAIN, READ_ONLY"
  - "policy::explain no longer moves blocked/allowed/passed on any branch"
  - "policy::explain still consults the detectors, so a heuristic block stays explainable"
  - "tests/explain.rs pinning all six counter branches, the unchanged DNS path and the detector step"
  - "API and policy docs that state the tunneling-window consequence instead of promising no effect"

affects:
  - crates/alpendns/src/policy/mod.rs
  - crates/alpendns/src/api/mod.rs

actuals:
  tokens: 4530
  tasks: 2
  commits: 1
  plan_head_before: ed21aa89bb36c7b9d36da3a24bc3a71e1a2a4276
  plan_head_after: 67835dd0d22abe745de0b5a3e2647f20db5b9c70

tech-stack:
  added: []
  patterns:
    - "Two independent effect axes instead of one mode: a caller states what it may count and what it may consult"
    - "Red-first integration file under tests/ with a local engine, no network and no live listener"

key-files:
  created:
    - crates/alpendns/tests/explain.rs
  modified:
    - crates/alpendns/src/policy/mod.rs
    - crates/alpendns/src/api/mod.rs

key-decisions:
  - "Locked decision (user, 2026-10-30): explain keeps running the detectors; only the counting stops."
  - "Effects has two bool fields, count and detectors; the previous enum conflated them and exactly one site needed the second axis."
  - "explain calls evaluate_with directly with RecordType::A and Effects::EXPLAIN; the typeless evaluate stays for callers without a real query."
  - "The docs now state the real effect: no policy counter moves, the heuristics are consulted, which advances the tunneling window and can move the process-wide detector counter. A fully inert explain was rejected."
  - "This supersedes the 261022-t0a decision that Effects::ReadOnly was the only read-only shape: the block page keeps it, the explanation does not."

patterns-established:
  - "A read-looking caller names its mode at the call site (Effects::EXPLAIN) instead of inheriting the DNS default."
  - "Counter branches are pinned by driving the same engine through every branch and comparing the whole PolicyStats snapshot."

requirements-completed: [WR-05]

coverage:
  - id: D1
    description: "policy::explain leaves blocked, allowed and passed unchanged on all six branches (final pass, root, allowlist, grant, blocklist, detector)."
    requirement: WR-05
    verification:
      - kind: integration
        ref: "crates/alpendns/tests/explain.rs#explain_leaves_the_policy_counters_alone"
        status: pass
    human_judgment: false
  - id: D2
    description: "The DNS path still counts: a block moves blocked, a pass moves passed, an allowlist hit and a grant each move allowed."
    requirement: WR-05
    verification:
      - kind: integration
        ref: "crates/alpendns/tests/explain.rs#the_dns_path_still_counts"
        status: pass
    human_judgment: false
  - id: D3
    description: "explain still reports a heuristic block, and the reasoning carries the detector step."
    requirement: WR-05
    verification:
      - kind: integration
        ref: "crates/alpendns/tests/explain.rs#explain_still_reports_a_heuristic_block"
        status: pass
    human_judgment: false
  - id: D4
    description: "The block page path is unchanged: block_cause neither counts nor runs detectors, and its tests pass unmodified."
    requirement: WR-05
    verification:
      - kind: unit
        ref: "crates/alpendns/src/api/block_page.rs#block_cause_leaves_the_policy_counters_unchanged"
        status: pass
      - kind: unit
        ref: "crates/alpendns/src/api/block_page.rs#block_cause_does_not_run_detectors"
        status: pass
    human_judgment: false
  - id: D5
    description: "No two-valued predicate remains in the switch; the six counter sites read count and the detector gate reads detectors."
    requirement: WR-05
    verification:
      - kind: automated
        ref: "grep: no Effects::Count or Effects::ReadOnly in crates/alpendns/src"
        status: pass
    human_judgment: false
  - id: D6
    description: "No doc or operator text still claims the lookup changes nothing; the affected docs state the tunneling-window consequence."
    requirement: WR-05
    verification:
      - kind: automated
        ref: "grep: no 'ohne etwas zu ändern|ohne etwas zu verändern|nichts gespeichert und nichts gezählt' in crates/alpendns/src, docs, config, packaging"
        status: pass
    human_judgment: false

duration: 4min
completed: 2026-10-30
status: complete
---

# Quick task 261030-oj8 Plan 01: `policy::explain` must not mutate Summary

**A read-looking lookup that stopped moving the operators' counters without stopping the explanation of a heuristic block, by splitting one mode into two independent gates**

## Performance

- **Duration:** ~4 min (commit span)
- **Started:** 2026-10-30T16:46:45Z
- **Completed:** 2026-10-30T16:50:08Z
- **Tasks:** 2
- **Files modified:** 3 (1 created)

## Accomplishments

- `Effects` is now a struct with two independent axes. `count` drives the six counter
  sites (normalize-miss passed, temporary-allow allowed, allowlist allowed, final passed,
  detector-gate block via `record_block`, and the record_block block itself); `detectors`
  drives the single detector gate. Three named modes say who wants what:
  `DNS { true, true }`, `EXPLAIN { false, true }`, `READ_ONLY { false, false }`.
- `policy::explain` routes through `evaluate_with(..., RecordType::A, Effects::EXPLAIN)`.
  The six branches no longer touch `blocked`/`allowed`/`passed`, while the heuristics still
  run, so a detector-blocked name comes back `blocked == true` with the detector step in
  its reasoning. This is the locked decision, and test 3 fails if it is ever undone.
- `block_cause` and the DNS path are behaviourally unchanged (`READ_ONLY` and `DNS`).
  The four existing `block_cause` tests pass without modification.
- `crates/alpendns/tests/explain.rs` is new and pins the whole thing: the six-branch
  counter snapshot, the counting DNS path, and the detector step. It builds a local engine
  from a `Blueprint` with a `TestClock`, a `FixedWallClock` and a local `Dummy` detector,
  and reaches no network.
- The three doc sites now describe what the code does. The old promise ("simulation, counts
  nothing, leaves nothing behind") is gone; the docs say no policy counter moves, the
  heuristics are consulted, and that this advances the tunneling window and can move the
  process-wide detector counter. The name itself is still stored nowhere.

## Task Commits

The plan allows exactly one commit for tests, code and docs:

1. **Task 1 + Task 2: two-axis switch, new tests, corrected docs** — `67835dd` (fix)

## RED gate evidence

`cargo test --all-features --test explain` was run after writing the test file and before
touching `policy/mod.rs`:

```
running 3 tests
test explain_still_reports_a_heuristic_block ... ok
test the_dns_path_still_counts ... ok
test explain_leaves_the_policy_counters_alone ... FAILED

assertion `left == right` failed: explain moved a counter on the final passed branch (frei.example)
  left: PolicyStats { blocked: 0, allowed: 0, passed: 1, entries: 2 }
 right: PolicyStats { blocked: 0, allowed: 0, passed: 0, entries: 2 }
```

The other two tests passed at HEAD, as required. Nothing was committed between RED and
GREEN; the red tree was never committed.

## Files Created/Modified

- `crates/alpendns/tests/explain.rs` — new integration target with the three tests and the
  local harness (lists, policy, engine with detectors, `Dummy` detector, `peer`).
- `crates/alpendns/src/policy/mod.rs` — `Effects` struct with `count`/`detectors`, the three
  associated consts, the rewired call sites, `explain` through `evaluate_with`, and the
  corrected German doc comments (struct, `explain`, `block_cause`, `evaluate_typed`,
  `evaluate_with`, detector-gate comment).
- `crates/alpendns/src/api/mod.rs` — `StatusSource::explain` trait doc and the
  `/api/explain` handler doc.

## Sweep result for other copies of the claim

Searched `crates/alpendns/src`, `docs`, `config`, `packaging` and `web` for
"nichts gespeichert", "nichts gezählt", "ändert nichts", "hinterlässt nichts",
"ohne etwas zu ändern/verändern" and "simulation":

- The only hits were the two corrected docs in `api/mod.rs` and `policy/mod.rs`.
- `docs/OPERATIONS.md` ("without looking into the log", "without the server having to
  run"), `docs/ROADMAP.md` (phases 5 and 7 acceptance notes, "both call `policy::explain`")
  and `web/app.js` stay accurate and were not edited.
- `crates/alpendns/src/main.rs` needs no change: `policy test` builds its own throwaway
  engine, so the counters it could move are discarded with the process; the help text and
  the comment at `main.rs:1369` stay true. The word "Simulation" at `main.rs:1355` is about
  working from the cached lists without a network, which is still true.
- `web/dist/alpendns-demo.html` is a gitignored build artifact and was not touched.

## Deviations from Plan

**1. [Style correction] Doc comments written in German, not English**
- **Found during:** Task 2
- **Issue:** The plan said to keep the file's existing English. `crates/alpendns/src/api/mod.rs`
  is entirely German; its old line 50 read "ohne etwas zu verändern oder zu speichern".
- **Fix:** Wrote the two rewritten docs in German to match the file and `CLAUDE.md` B.2
  ("match existing style"). The content is exactly what the plan demanded.
- **Files modified:** `crates/alpendns/src/api/mod.rs`
- **Commit:** `67835dd`

**2. [Test detail] The "mention DGA" assertion rides on the detector reason**
- **Found during:** Task 1
- **Issue:** `Detector::Dga` renders as "Algorithmically generated name", not "DGA", so a
  rendered step would not contain the literal "DGA" on its own.
- **Fix:** The `Dummy` detector's `Finding::new` reason is "test detector DGA: suspicious",
  which is what the plan asked for ("a reason naming the detector"). The test asserts the
  rendered steps contain "DGA", which now comes from the reason, and that the synthesized
  step is present too.
- **Files modified:** `crates/alpendns/tests/explain.rs`
- **Commit:** `67835dd`

No auto-fix was needed for a bug or a blocking issue; no architectural change was required.

## Verification

All four definition-of-done commands pass on the committed tree:

```
cargo fmt --all --check                                  -> exit 0
cargo clippy --all-targets --all-features -- -D warnings -> exit 0
cargo test --all-features                                -> all suites ok (506 unit, 7 binary, 157 integration incl. the 3 new; only the --ignored measurement runs skipped)
cargo deny check                                         -> advisories ok, bans ok, licenses ok, sources ok
```

Task gates:

```
cargo test --all-features --test explain   -> 3 passed
cargo test --all-features block_cause      -> 4 passed
grep Effects::Count|Effects::ReadOnly      -> no hits
grep old claim in src/docs/config/packaging-> no hits
```

Manual cross-check: the three mode constants were read side by side with the three
rewritten doc blocks; each sentence matches the mode it describes. `DNS` counts and
consults, `EXPLAIN` counts nothing and consults, `READ_ONLY` does neither.

## Known Stubs

None. No hardcoded empty values, no placeholder text, and no component wired to mock data
were introduced.

## Threat Flags

None beyond the plan's threat model. The change narrows the mutation surface
(T-261030-oj8-01) and documents the accepted one (T-261030-oj8-02); no new endpoint, auth
path or file access was added.

## Self-Check: PASSED

- `crates/alpendns/tests/explain.rs` exists.
- `crates/alpendns/src/policy/mod.rs` and `crates/alpendns/src/api/mod.rs` modified as described.
- Commit `67835dd` is an ancestor of HEAD.
- Working tree clean after the commit; the commit deleted no tracked files.
