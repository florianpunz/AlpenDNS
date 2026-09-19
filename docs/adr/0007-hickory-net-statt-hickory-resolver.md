# ADR-0007: Transports from `hickory-net`; pool and selection stay ours

**Status:** accepted · **Date:** 2026-08-29 · **Supplements:** [ADR-0002](0002-hickory-proto-statt-eigenem-parser.md)

## Context

[ADR-0002](0002-hickory-proto-statt-eigenem-parser.md) records:
"`hickory-resolver` is used for the upstream transports (DoT/DoH/DoQ) where it
fits." Implementing Phase 3 turned up two things.

**First, the crates have been rearranged.** In `hickory 0.26`, `hickory-proto` has
been boiled down to the bare wire format; the transports live in a new crate,
`hickory-net`. `hickory-resolver` builds on top of that and adds: its own cache,
its own retry logic, its own name-server pool with its own selection.

**Second, that is exactly the part we have ourselves.** The cache is Phase 2 and,
per ARCHITECTURE.md §4, deliberately ours — it stores the *unfiltered* answer,
because filtering sits in front of it. Pool, selection strategies and failure
detection are Phase 3, steps 3 to 5, and `split_by_zone` exists nowhere else
(FEATURES.md P2). Using `hickory-resolver` would mean having those parts twice and
passing our own through to someone else's.

## Decision

We use **`hickory-net` for the transports** — DoT, DoH and DoQ, that is the TLS
handshake, HTTP/2 framing, QUIC streams and the matching of answers to requests on
a multiplexed connection. That is the same class of risky code ADR-0002 was about,
and the same reasoning applies.

`hickory-resolver` is **not** used. Ours remain:

* when a connection is opened, reused and discarded (`upstream::transport`),
* which upstream gets a request (`upstream::strategy`),
* when an upstream counts as failed (`upstream::pool`),
* the cache (`cache`, Phase 2),
* what happens to the message before it is sent (`privacy`).

For the crypto backend, the `-ring` variants are chosen throughout, not
`aws-lc-rs`: the latter brings the OpenSSL licence with it, which is not on the
allowlist in `deny.toml`.

## Consequences

* The connection lifecycle is our code, around 190 lines. It has to be right: a
  connection that is not discarded after an error answers no more requests.
* An API break in `hickory-net` hits us directly. In exchange, we are not bound to
  the release policy of `hickory-resolver`, which has considerably more surface.
* The statement from ADR-0002 remains valid in substance; the crate name in it is
  outdated. This ADR does not replace ADR-0002 — the decision "no parser of our
  own" stands unchanged.
* `hickory-net` rewrites the query ID itself on a multiplexed connection.
  Resetting it is our job, and forgetting to is a mistake none of our fake tests
  saw — see the note on Phase 3 in ROADMAP.md.

## Alternatives

* **`hickory-resolver` with its pool.** Saves us our pool, but brings a second
  cache with it — exactly what ARCHITECTURE.md §4 rules out — and turns
  `split_by_zone` into a foreign body in someone else's selection logic.
* **Write the transports ourselves.** TLS handshake, HTTP/2 and QUIC by hand: the
  same answer as in ADR-0002, only with more risk.
