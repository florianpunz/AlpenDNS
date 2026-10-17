# ADR-0027: Resolve failures are cached briefly, with a backoff

**Status:** accepted · **Date:** 2026-10-17 · **Concerns:**
`crates/alpendns/src/caching.rs`, `crates/alpendns/src/cache.rs`,
[RFC 9520 §3.2](https://www.rfc-editor.org/rfc/rfc9520#section-3.2)

## Context

`cache.rs` deliberately does not cache `SERVFAIL`, and the reason stands in the
code: *"SERVFAIL, REFUSED und alles andere sind Zustandsmeldungen, keine Daten.
Sie zu cachen würde einen kurzen Ausfall verlängern."* The test
`servfail_is_not_cached` pins it.

The measurement on dns1 (2026-10-17) shows the price of that decision. One name
under a dead delegation (`time.ien.at`; both name servers of the zone time out)
was asked 645 times in 6.3 hours. Every one of those questions went to an
upstream and ran into a 3–8 s timeout; three of them in a row mark the upstream
as down for 30 s — for **all** clients:

* 63.1 % of the 130 down events in that window had an `ien.at` question within
  the 10 s before it (baseline: 6.0 % at randomly chosen timestamps — a factor of
  ten), median distance to the down event 3.8 s
* 425 of 645 `ien.at` questions (65.9 %) were followed by a down event within 30 s
* the failure before the down event was "Upstream did not answer within the time
  budget" in 74 of 130 cases, "resource too busy" in 9
* in the same window 130 down events and 130 recoveries flapped: the pool was
  never really dead, only permanently re-declared dead

**The source is found and fixed.** The questions came from the OpenWrt access
point `192.168.1.22` (a Cudy WR3000S in dumb-AP mode): `time.ien.at` stood in
its NTP candidate list, and busybox-ntpd asks every candidate in rotation,
including the dead one. The DCs only forwarded (`w32time` on them is clean,
`at.pool.ntp.org`). The entry was removed on 2026-10-17.

That changes what this decision is for: **not** a cure for this one device, but
the safety net for the next device of that kind — a poller nobody looks at, on a
name nobody can fix, that drags the upstream pool down for everyone.

RFC 9520 §3.2 is unambiguous: *"Resolvers MUST implement a cache for resolution
failures. […] Resolvers MUST cache resolution failures for at least 1 second.
[…] resolution failures MUST NOT be cached for longer than 5 minutes."* It also
suggests the shape: *"the initial time for negatively caching a resolution
failure might be set to 5 seconds and increased after each retry that results in
another resolution failure, up to a configurable maximum, not to exceed the
5-minute upper limit."*

## Decision

`CachingBackend` remembers a failed resolution for **5 s** and answers the same
question from that memory — i.e. with `SERVFAIL` — without asking an upstream.

* **Backoff per key:** the first failure locks for **5 s**, every further
  failure for the same key doubles it — 10, 20, 40, **60 s** — and stops there
  (`FAILURE_TTL`, `FAILURE_TTL_MAX`). A **success resets** it to 5 s. With that,
  a poller on a 35 s cycle is caught after three rounds; a fixed 5 s window
  would never catch it.
* **60 s instead of the 300 s the RFC allows:** after a network outage a
  forgotten lockout holds at most one minute per name, and the net is still
  tight enough for a device that asks once a minute.
* **Key:** the same as the answer cache, `(name, type, class)` (`cache::Key`).
  Not the upstream: a name that fails is a property of the name, and the client
  asks the same question again, not a different one.
* **What is remembered:** any `ResolveError` from the inner backend. Not only
  timeouts: a bogus verdict, an unreachable upstream and an unproven answer are
  all "this question just failed", and all of them cost the same retry.
* **What is not remembered:** a stale answer. The lookup order stays
  cache → failure memory → upstream, so a `serve_stale` entry is served first
  and wins; the failure only applies when there is nothing to serve.
* **Memory bound:** at most 1024 entries; when the limit is reached, expired
  entries are dropped first (RFC 9520's "mitigate resource exhaustion").
* **No configuration key.** B.1 rule 5: no key without asking, and the RFC fixes
  the range anyway.
* **A `Step::FailureCached` in the trace**, so a `SERVFAIL` in the query log has
  a reason instead of an empty `why` list.

## Alternatives

* **A fixed 5 s window (the first draft).** Rejected by the measurement above:
  the AP asks every ~35 s, so it was never inside the window. The backoff is
  what makes the cache meet a slow poller at all.
* **Only stop counting timeouts as an upstream failure.** Directly attacks the
  symptom (the pool flapping) and is one condition in `pool::try_one`. Rejected:
  a genuinely dead upstream would then never be skipped, and the client still
  waits 3–8 s per question — the cost the household sees stays exactly the same.
  It also weakens outage detection for every other case.
* **Cache the `SERVFAIL` message in the answer cache.** Rejected: that cache
  holds data, and its negative branch is keyed on a SOA (RFC 2308). A `SERVFAIL`
  has no SOA; a synthetic one would claim authority, the same reason ADR-0023
  refuses a synthetic SOA.
* **300 s, the RFC's upper bound.** Rejected: a name that recovers would stay
  broken for five minutes, and after a network outage every touched name would
  carry that. 60 s is the compromise.
* **Time-based decay of the backoff.** Not built: only a success (or a restart)
  resets it. A name that fails five times over a week therefore stays at 60 s —
  accepted, because a name that fails that often is broken, not unlucky.

## Consequences

* **A retry after a timeout no longer reaches the upstream**, and a device that
  polls a dead name every minute no longer declares the pool dead: after three
  rounds its questions are answered from memory.
* **A recovered name stays failed for up to 60 s**, and the first question after
  the lockout asks the upstream again.
* **The failure memory is per process**, like the answer cache: a restart
  forgets it, including the backoff state.
* **`Bogus` becomes cheaper.** A bogus verdict is remembered the same way, so
  the same name does not cost a fresh terminal verdict on every retry.
* **A7 (the effect after the AP was fixed) is measured separately**; the numbers
  belong in the observation report, not in this ADR.
* **Testing:** six unit tests in `caching.rs` — no upstream call inside the
  window, a call after it, the sequence 5→10→20→40→60→60, a success resetting to
  5 s, the 1024-entry bound, and a stale entry winning over a remembered failure.
  All of them run on the `TestClock`, no `sleep`.
