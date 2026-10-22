---
status: complete
phase: 08-heuristics-without-a-cloud
source: .planning/ROADMAP.md success criteria (pre-GSD phase; no SUMMARY.md exists)
started: 2026-10-22T16:59:07Z
updated: 2026-10-22T17:01:31.818Z
---

## Current Test

[testing complete]

## Tests

### 1. Detectors report in the trace and all default to flag
expected: For any query the decision trace carries each detector's score and a human-readable reason, and every detector's configured action is `flag`: nothing is blocked by a detector unless the operator changed it.
result: pass

### 2. Rebinding protection is narrow and exempts forward zones
expected: Rebinding flags only answers that point into reachable private address ranges, and names under a `[[forward_zone]]` are exempt — an internal answer from the LAN nameserver is not flagged.
result: pass

### 3. False-positive budget holds and NRD tolerates a missing file
expected: On the top-100k corpus tunneling and DGA stay under 0.1% false positives, typosquat never flags the protected originals, and NRD works from a local file while a missing file is not a startup error.
result: pass

### 4. The flagged panel explains itself and acts in one click
expected: The "Flagged" panel shows detector, score and reason per entry, with one click to allow and one to block; the allow/deny entries are the time-limited ones and say that they do not survive a restart.
result: pass

### 5. A week of real-network operation with a reviewed false-positive list
expected: One week with all detectors on `flag` produced a reviewed false-positive list, and no detector was switched to `block` before that review — the acceptance statement in docs/ROADMAP.md says so.
result: pass

## Summary

total: 5
passed: 5
issues: 0
pending: 0
skipped: 0

## Gaps

[none yet]
