# ADR-0021: Rebinding flags only addresses where a device can sit

**Status:** accepted · **Date:** 2026-09-13 · **Affects:**
[ROADMAP.md](../ROADMAP.md) Phase 8 step 2 and the acceptance of the phase,
`crates/alpendns/src/detect/rebinding.rs`

## Context

The rebinding detector reports every answer in which a public name points at
a private address. The list of private ranges was drawn wide:
alongside RFC 1918, loopback and link-local it contained `0.0.0.0`/`::` and the
whole range `192.0.0.0/16`.

The observation week checked this — 132,208 queries from 2026-08-30 to
2026-09-08: **1,763 reports, not a single attack.** They fall into exactly
three groups:

| Address | Reports | What it was |
|---|---|---|
| `0.0.0.0` | 1,467 | Sinkholes of the upstreams for telemetry |
| `::` | 281 | the same for IPv6 |
| `192.0.0.0/16` | 15 | 192.0.0.170/171 (NAT64, RFC 7050), `github.blog` → 192.0.66.2, `secure.gravatar.com` → 192.0.73.2 |

The first two groups are the usual way of shutting a telemetry
domain down: Apple (`a.gslb.aaplimg.com`), Amazon (`unagi-eu.amazon.com`),
Microsoft (`settings-win.data.microsoft.com`), plus Twitch, Sentry, NordVPN.
The third group is routed address space — Quad9 and Cloudflare answer
independently of each other with `192.0.66.2` for `github.blog`.

Three things about this are the reason, not the number alone:

1. The **own** block mode `zero_ip` returns exactly the answer that the
   detector reports as an attack. The system contradicts itself.
2. The comment at the line said `192.0.0/24`, the code checked the whole
   `/16`. The false positive on `github.blog` was a typo in the width.
3. A detector that false-positives 1,763 times a month is no longer
   read. That is the actual damage — not the number in the log.

## Decision

The list contains only addresses **where a device on the local network can
sit**. Removed are `0.0.0.0`, `::` and `192.0.0.0/16`. What remains is
loopback, RFC 1918, link-local, broadcast, CGNAT (100.64/10) and benchmark
(198.18/15) — the ranges that an attack actually needs as a target.

## Consequences

* After the change the detector would have reported **nothing** in eight and a
  half days of real traffic. The "Auffällig" panel becomes readable again; a hit
  means something.
* What is no longer reported: an answer with `0.0.0.0`. On Linux a
  connection to `0.0.0.0` leads to one's own machine — that was a
  known bypass of the Private Network Access rules in 2024, and the browsers
  followed suit. Should that become a topic here again, it belongs in
  a detector of its own, not in a list that hits every sinkhole.
* Also no longer reported: a network that uses `192.0.0.0/16` internally.
  The range is not RFC 1918 space but reserved for IANA protocols.
* No `allow_zones` entry needed: `ipv4only.arpa` (192.0.0.170/171) falls
  out by itself through the change.

## Alternatives

* **Enter `ipv4only.arpa` in `allow_zones`.** Would have taken care of the 11 NAT64
  reports and left the 1,752 sinkhole reports in place.
* **Enter the sinkhole zones individually.** Not possible: Amazon's device names
  (`78cec6f6…minerva.devices.a2z.com`) are randomly generated.
* **Put `0.0.0.0` on `log` instead of `flag`.** Changes the visibility, not
  the number — the panel stays full.
* **A second switch "report sinkholes".** Configuration for a case
  that is not a decision.
