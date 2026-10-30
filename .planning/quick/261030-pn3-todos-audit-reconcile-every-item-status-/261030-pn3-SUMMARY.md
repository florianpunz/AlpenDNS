---
phase: 261030-pn3
plan: 01
status: complete
subsystem: docs
tags: [docs, todos, audit, systemd, logging, blocklists, config, block-page]

requires: []
provides:
  - "TODOS.md statuses reconciled with the tree: 2, 7 and 8 closed, 3 corrected, B2/B3 corrected"
  - "the remaining open work named precisely: item 9, item 10 stages, item 3 step 6, item 8 search field"

affects:
  - docs/TODOS.md
  - .planning/STATE.md
---

# Quick task 261030-pn3 — TODOS audit

## Why

Four of the last five TODOS items turned out to be implemented but unmarked. The list was
describing a state that had not existed for weeks, which is worse than no list: it sends
the next reader to build what is already there, or to argue against a decision that has
already been taken.

## What the audit found

| # | Before | After | Why |
|---|---|---|---|
| 2 | open | **done** | `packaging/logrotate/alpendns` with `rotate 7`, `BufWriter` + `run_flusher`, `rotation_hint`, OPERATIONS sentence |
| 3 | (a) done, (b) open | **done except step 6's counter** | `policy::run_updater` has existed since phase 5 — the scheduler is built; a failed update has no counter, and the interval is the minimum across lists |
| 7 | open | **done** | `typo_hint` built; the `serde(alias)` pattern and the rejection of a version field are ADR-0024 |
| 8 | open | **done, different half** | the full block page was built in 261022-t0a; the recommended search field was not |
| 9 | open | open | no validate endpoint, no wizard |
| 10 | open | open | no stage 0–3 work: no benchmark, no config hash, no peer endpoint |

The cross-cutting findings B2 and B3 were also wrong and are corrected. B2 claimed there
is no update scheduler (there has been one since phase 5); B3 claimed the log is written
unbuffered under a lock (it sits behind a `BufWriter` flushed once a second).

## The mistake this audit caught in my own work

Two hours earlier, quick task 261030-p4w wrote into TODOS.md that the refresh scheduler
"does not exist" and that `blocking.<list>.refresh` is "parsed and ignored". Both were
false: I had grepped `main.rs` and `filter/source.rs` and missed `policy::run_updater`.
This task corrects that sentence and the identical claim in STATE.md's deferred table.
The lesson is in the method, not the outcome: a status claim needs a search for the
behaviour, not for the words the TODO used.

## Verification

Docs-only. Every claim in the table above was checked in the tree (the auditor's report
was spot-checked by reading the code it cites), and the one claim that contradicted the
previous commit was verified twice.

## Genuinely open after this

1. Item 10, stages 0–1 (measurement, config hash) and stage 3 (peer endpoint).
2. Item 9, the configuration wizard — "last or not at all".
3. Item 3, step 6: the update-failure counter.
4. Item 8: the search field from the recommended half, and the combined
   sinkhole-plus-rebinding test.
