---
status: complete
phase: 09-operation-and-packaging
source: .planning/ROADMAP.md success criteria (pre-GSD phase; no SUMMARY.md exists)
started: 2026-10-22T17:06:00Z
updated: 2026-10-22T17:46:58.575Z
---

## Current Test

[testing complete]

## Tests

### 1. Unprivileged service on port 53 with a hardened unit
expected: The service runs as an unprivileged user and gets port 53 from `AmbientCapabilities=CAP_NET_BIND_SERVICE`, not from root; `systemd-analyze security alpendns` stays under 3.0.
result: pass

### 2. check refuses a broken config without taking the old instance down
expected: `alpendns check` as `ExecStartPre` refuses a broken configuration while the running instance keeps running, and the check needs no network.
result: pass

### 3. The .deb installs on a fresh Debian VM and the service starts
expected: The package installs on a fresh Debian 13 VM and the service starts with no manual follow-up work — user, directories, unit and configuration come from the package.
result: pass

### 4. One client over the limit is dropped, the others are not
expected: A client above the rate limit has its queries dropped while other clients are unaffected, and IPv6 clients are keyed by /64 so one host cannot rotate through addresses.
result: pass

### 5. A fresh installation is reachable, and says what that costs
expected: After installation the listener binds every IPv4 interface, so the LAN reaches it without an edit; the config file, OPERATIONS.md and `alpendns check` warn that this is an open resolver on a host with a public address, and per-client throttling stays on. Criterion replaced on 2026-10-22 at the operator's request — it previously required loopback only (ADR-0031).
result: pass

### 6. A week as the only LAN resolver, and the docs carry a second person
expected: The server ran a week as the only resolver in the LAN without complaints, and docs/BENCHMARKS.md plus docs/OPERATIONS.md are enough for a second person to install and operate it.
result: pass

## Summary

total: 6
passed: 6
issues: 0
pending: 0
skipped: 0

## Gaps

[none yet]
