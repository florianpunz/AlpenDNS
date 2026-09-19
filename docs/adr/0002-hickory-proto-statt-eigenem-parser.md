# ADR-0002: DNS wire format via `hickory-proto`, not written ourselves

**Status:** accepted · **Date:** 2026-08-29

## Context

A DNS server has to be able to read and write DNS messages. Two ways:

1. Implement it yourself — RFC 1035 plus about thirty more RFCs for record types,
   EDNS(0), name compression, DNSSEC record types.
2. Use an existing library. In Rust that is practically
   [`hickory-proto`](https://docs.rs/hickory-proto) (version 0.26, as of May 2026), the
   protocol layer of Hickory DNS. The alternative would be `domain` (NLnet Labs).

The appeal of variant 1 is real: you properly learn DNS doing it. So is the price. The
most dangerous part of a DNS parser is **name compression** (RFC 1035 §4.1.4) —
pointers in the message body that point at earlier names. A pointer that points at itself
or backwards into a loop is the classic DoS hole; it has been found in more than one
production implementation. Add to that truncated packets, length fields that point past
the end of the packet, and record types with variable structure.

## Decision

`hickory-proto` for parsing and serialization. Everything above it — cache, filtering,
policy, upstream selection, heuristics, API, UI — is our own code.

`hickory-resolver` is used for the upstream transports (DoT/DoH/DoQ) where it fits.
`hickory-server` is **not** adopted as a framework: our request pipeline with its decision
trace is the actual substance of the project and should not be pressed into foreign
handler traits.

Hickory's `recursor` crate is marked experimental. That is no problem, because v1 does no
recursion — but it is good news for the case that recursion does come: then there is a
starting point.

## Consequences

* The riskiest class of memory and DoS bugs sits in a library that has seen far more eyes
  and far more fuzzing hours than this project will ever get.
* We are bound to its data types and to its release cycle. A major update can cost work.
* The learning effect of "what does a DNS packet look like on the wire" is missing.
  Remedy, if desired: our own parser as a separate, non-production exercise project,
  checked against `hickory-proto` as a reference (differential testing). That is a nice
  way to learn the format without hanging the server's security on it.
* Fuzzing remains mandatory all the same — for *our* use of the library.

## Alternatives

* **`domain` (NLnet Labs):** clean design, but a smaller community and fewer ready-made
  transports for DoH/DoQ.
* **Our own parser:** see above. As an exercise yes, as a production base no.
