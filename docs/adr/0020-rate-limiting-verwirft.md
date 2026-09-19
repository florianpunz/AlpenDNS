# ADR-0020: Above the limit, packets are dropped, not refused

**Status:** accepted · **Date:** 2026-08-30 · **Affects:** [ROADMAP.md](../ROADMAP.md) Phase 9 step 5, CLAUDE.md B.5

## Context

An open resolver is an amplification reflector. The attack is old and
cheap: a short query with a forged source address in, a long answer out to
the victim. CLAUDE.md B.5 therefore makes per-client throttling a
duty before the server listens anywhere where it does not only see its own LAN.

That raises three questions that look like details and are not.

## Decision 1: drop instead of refuse

Above the limit the packet is dropped without a word. No REFUSED, no
SERVFAIL, no shortened "ask over TCP instead".

The reason is the attack itself: with UDP the source address is freely choosable,
and the whole point of the throttling is that nothing is sent to an address that
may never have asked. A REFUSED answer is smaller than a real answer, but it is
still a packet to the victim — a throttling that keeps the reflector running,
only more quietly.

The price is real and is deliberately paid here: a *legitimate* client above
the limit sees no refusal but a timeout, and timeouts are harder to
interpret for whoever sits in front of them than an error message. Against that
stands the fact that the limits are high enough that a single device does not
reach them in normal operation (100 queries/s sustained, peak 200), and that the
counter `alpendns_rate_limited_total` answers exactly the question that then gets
asked.

This is also what BIND and Knot do with Response Rate Limiting. Both
additionally offer a "slip" rate: every n-th excess query is answered with the
TC flag set, so that a real client can fall back to TCP. That is
right for an authoritative server on the internet that serves complete
strangers. For a resolver in one's own LAN it would be a second switch with
a third meaning for a case that does not occur here — and every slipped
answer again goes to a possibly forged address.
Reversal condition: as soon as AlpenDNS serves clients that are not on its
own network (the DoH listener from Phase 10), slip belongs on the agenda.

## Decision 2: IPv6 is aggregated to /64

Counting is per IPv4 address and per IPv6-**/64**, not per IPv6 address.

The reason is not the attacker but normal operation: with Privacy
Extensions (RFC 8981) an ordinary laptop changes its IPv6 address every hour
and uses several at once. Counted per address, the same machine would keep
getting a fresh budget — the throttling would be decoration for IPv6. That an
attacker from one /64 can do the same comes on top of that.

`::ffff:10.0.0.1` and `10.0.0.1` are the same host and share one
bucket. Without this line, a client whose operating system opens the socket on
v6 would have two budgets.

A /64 is the allocation to a single network segment; whoever wants to count
more finely loses more (the real host) than they gain.

## Decision 3: token bucket in drawers, no sliding windows

One bucket per client with a refill rate and a ceiling — the simplest structure
that can express "sustained x, short-term y". A sliding time window
would be more precise and would need a list of timestamps per client.

The buckets live in 16 drawers, each behind its own mutex. CLAUDE.md
B.3 rule 5 forbids a global mutex in the query path, and rightly so:
at 85,000 queries/s a single lock would be the serialization of the whole
server. Something lock-free per client would be the kind of
concurrency code that one gets wrong and where the error only shows up under load.

The number of observed clients is capped (LRU, default 8192). Without this
limit the throttling would itself be the memory hog it is meant to prevent, under
a flood of forged source addresses.

## Consequences

* A throttled client sees a timeout. Operations recognizes this by
  `alpendns_rate_limited_total`; the instructions for it are in
  [OPERATIONS.md](../OPERATIONS.md) §4.
* The address of the throttled client appears only at log level `debug` and
  thus by default nowhere. In the metric it does not appear at all: a label with
  the client IP would be a presence list with timestamps, and Prometheus keeps
  every time series forever.
* Measured cost on the query path: **2 %** throughput
  ([BENCHMARKS.md](../BENCHMARKS.md), Phase 9).
* The throttling sits **before** parsing the message. A dropped packet
  therefore costs a hash and a comparison, not the trip through
  hickory-proto.

## Alternatives that were rejected

**Throttle on UDP only.** TCP senders are confirmed by the handshake and
are no good for reflection. Even so the limit applies there too: the second danger
of an open resolver is plain exhaustion — open connections,
upstream queries, cache eviction — and that knows no transport protocol.

**Weight by answer size** (expensive answers cost more budget). That is
the more precise brake against amplification, because it starts at the
amplification factor. But it presupposes that the answer is already there — by
then the work is done, and the protection against exhaustion falls away.
For a resolver, the query is the right moment.

**Point at nftables.** A packet filter can do this too, and in a large
network that is where it belongs. For a program whose promise is "installed
and hardened in five minutes", a firewall rule that someone has to write
by hand is not protection — it is a footnote.
