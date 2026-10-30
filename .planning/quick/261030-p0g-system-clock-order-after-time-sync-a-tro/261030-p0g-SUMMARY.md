---
phase: 261030-p0g
plan: 01
status: complete
subsystem: docs
tags: [docs, todos, clock, dnssec, systemd]

requires:
  - phase: 09
    provides: the systemd unit and `alpendns check` as ExecStartPre

provides:
  - "TODOS.md item 4 closed as done, with the four landing sites named"
  - "the marker-vs-adjtimex deviation recorded with its reason"

affects:
  - docs/TODOS.md
---

# Quick task 261030-p0g — close the clock TODO

## What happened

The task started as "document the dependency on the system clock" and turned out to be
already done. Before planning, all four of the item's steps were found in the tree:

| Step | Site |
|---|---|
| `After=time-sync.target` | `packaging/systemd/alpendns.service` |
| runbook section | `docs/OPERATIONS.md` §4, "Everything is SERVFAIL" |
| hint from `check` | `crates/alpendns/src/clock.rs:147`, wired at `main.rs:1242` |
| threat-model sentence | `docs/THREAT-MODEL.md` |

The item's heading was the only thing still saying "open". It is now marked done like
items 1, 5, 6, 11, 12 and 13, with a Result paragraph naming those four sites and the one
deviation from the item's own text: the hint reads the systemd sync marker rather than
`adjtimex`, because that syscall needs an unsafe binding (B.1 rule 2). On a chrony/ntpd
box the note can therefore be wrong; the note itself says so and points at `timedatectl`.

## Verification

Docs-only. Each of the four claims was checked against the tree before the edit; the
definition-of-done commands were not re-run because no code changed. The two automated
greps in the plan pass.

## Notes

- Item 8 (block page) is in the same position: implemented by quick task 261022-t0a, not
  marked done in TODOS.md. Left alone — different item, not this task's business.
- No production host or `/etc/alpendns` was touched.
