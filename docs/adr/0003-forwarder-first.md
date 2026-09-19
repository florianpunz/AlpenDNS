# ADR-0003: Forwarder in v1, recursion left open — but not pre-built

**Status:** accepted · **Date:** 2026-08-29

## Context

A DNS server can resolve names in two ways:

* **Forwarding:** pass the request on to another resolver (Quad9, Mullvad, dnsforge, …)
  and serve its answer.
* **Recursion:** start at the root servers yourself, work your way through TLD servers to
  the zone's authoritative servers, validate DNSSEC.

From a privacy point of view recursion is attractive: no provider sees the complete query,
because every server along the way learns only its part of the name (reinforced by
QNAME minimisation, RFC 9156). But it is also considerably more work: delegation handling,
glue records, CNAME chains across zone boundaries, broken authoritative servers, loop
detection, DNSSEC validation with trust anchor rollover, root hints maintenance, negative
caching per RFC 2308. And it has privacy drawbacks of its own: your own IP contacts every
authoritative server directly, which for a home connection IP with a single user is a
very distinctive signature.

The author has said explicitly that recursion may never come.

## Decision

v1 is a forwarder. There is **exactly one** piece of scaffolding for later recursion:
resolution sits behind a `ResolveBackend` trait with one implementation
(`ForwardBackend`). Nothing more.

What that explicitly does **not** mean:

* no root hints management "for later"
* no delegation data structures that nobody uses
* no configuration keys for recursion
* no traits with one implementation elsewhere "for symmetry"

If recursion ever comes, the likely path is not "write it ourselves" but
`hickory-recursor` behind the same trait — then it is a weekend job instead of a quarter.

## Consequences

* v1 is reachable in weeks instead of months. A running server that does something useful
  is worth more than a half-finished perfect one.
* Trust shifts from the provider to the upstream resolver. That is a real drawback, and
  the answer to it is not recursion but splitting across several upstreams (see
  FEATURES.md P2) — that takes the complete picture away from any single upstream, without
  leaving your own IP at every authoritative server in the world.
* QNAME minimisation does nothing in forwarder mode; it only takes effect with your own
  recursion. That belongs honestly in the documentation, instead of being listed as a
  feature.
* Our own DNSSEC validation is possible and sensible in forwarder mode too (believing the
  upstream's AD bit is worthless). It is noted as its own item in phase 7 and does not
  depend on this decision.

## Alternatives

* **Recursion in v1:** higher privacy ideal, considerably higher risk of never getting the
  project finished.
* **Both in parallel from the start:** double test matrix from day one, for a mode that
  maybe nobody ever switches on.
