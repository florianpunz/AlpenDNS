# ADR-0030: A forward zone may name several servers, asked in order

**Status:** accepted · **Date:** 2026-10-22 · **Concerns:**
`crates/alpendns/src/config.rs` (`ForwardZone`, `UpstreamList`),
`crates/alpendns/src/resolve.rs` (`Failover`),
`crates/alpendns/src/main.rs` (wiring, `check`),
`config/alpendns.example.toml`, `docs/ARCHITECTURE.md`,
`docs/OPERATIONS.md`, `docs/TODOS.md` item 13

## Context

A reverse zone is often served by more than one nameserver — on dns1 two domain
controllers, one per site, each hosting `1.168.192.in-addr.arpa`. Until now a
`[[forward_zone]]` carried exactly one `upstream`, so the operator had to pick
one; a second `[[forward_zone]]` for the same zone was dead text, and a dead
controller took the whole reverse zone with it. `alpendns check` warns about the
duplicate, which names the problem but does not solve it.

## Decision

`upstream` accepts a string as before or a list of strings. The order is the
failover order: the first server is asked, and the next one only when the
previous one returned an error. `UpstreamList` guarantees a non-empty list at
parse time; `resolve::Failover` implements the retry as an ordinary
`ResolveBackend`, so `ZoneRouter` and everything above it stay unchanged. Each
server gets its own `ForwardBackend`, hence its own cookie and 0x20 state.

The alternatives were rejected:

* **Merging sibling `[[forward_zone]]` entries for the same zone** would make
  an existing mistake work by accident and needs a rule for ordering that the
  configuration does not state.
* **Racing the servers in parallel** doubles the queries on every miss and
  would need the `Ctx` to be cloned — the trace assumes one linear path.

## Consequences

* The first server stays first: a dead one costs its query timeout on every
  query. That is the price of not keeping health state; the order in the file
  is the priority. Whoever wants to avoid the wait puts the reachable server
  first.
* A duplicate `[[forward_zone]]` for the same zone stays a warning, not an
  error: making it an error would break every configuration that has such a
  block today, and B.8 asks first.
* `alpendns check` lists all servers of a zone, so the effective order is
  visible before the start.
* No new dependency, no new configuration key, and configurations with a single
  server parse and behave exactly as before.
