# Threat model

A privacy product without an honest threat model is marketing. This document
states what AlpenDNS protects against, what it does not, and where the line
between the two is blurry.

## Who are the adversaries?

### A1 — The internet provider / the local network

**Can:** read all unencrypted traffic, forge DNS answers, redirect DNS queries
to its own resolver (transparent redirect on port 53).

**AlpenDNS against it:** every upstream query goes over DoT/DoH/DoQ. The
provider sees a TLS connection to a known resolver, not the names. Port 53
redirects come to nothing, because AlpenDNS does not use port 53 outbound.

**Remains visible:** the destination IPs of every connection *afterwards*, and
— for TLS without ECH — the server name in the ClientHello. **This is the most
important limitation of the whole project:** DNS encryption hides which name
you *looked up*, not which address you *connect to*. Anyone who needs
protection against that needs a VPN or Tor, not this DNS server.

### A2 — The upstream resolver

**Can:** see every query it answers, linked to your IP; build a profile from
that; tamper with answers.

**AlpenDNS against it:** `split_by_zone` distributes names deterministically
across several providers, so each one sees only a share — the unit is the
registrable domain according to the Public Suffix List, and the seed is redrawn
every 24 hours by default, so that no provider keeps a stable picture over time
([ADR-0018](adr/0018-public-suffix-list-und-seed-rotation.md)). ECS is stripped
so the query does not additionally carry your subnet. Padding prevents
inferences from message lengths. Optionally ODoH: the proxy knows your IP but
not the query, the resolver the other way round
([ADR-0017](adr/0017-oblivious-doh.md)).

**Remains:** without ODoH, every upstream sees your IP and its share of the
names. That share is not randomly distributed — popular domains land in the
same bucket for everyone, identical across all users. An attacker with access
to *several* of the configured upstreams removes the protection entirely. So
when choosing upstreams: different operators, different jurisdictions.

**Remains even with ODoH:** the proxy knows which provider you are talking to
(`targethost` is in the URL, otherwise it could not forward), and once per
process start the target sees your address when it fetches its public key — but
no query along with it. If proxy and target belong to the same operator, the
protection is void; no code can check that.

### A3 — Off-path attackers (cache poisoning, spoofing)

**Can:** send forged answers if it guesses the query ID and source port.

**AlpenDNS against it:** encrypted upstream transports make this largely moot.
On top of that: 0x20 encoding, DNS cookies (RFC 7873), strict validation of
every answer against the question that was asked before it is cached, source
port randomization.

**Closed since phase 7:** AlpenDNS recomputes the signature chain itself,
starting from the compiled-in root keys, and discards an answer whose zone
declares itself signed and whose chain does not close (SERVFAIL). The AD bit in
our answer then stands for our verdict, not for the upstream's claim
([ADR-0016](adr/0016-dnssec-validierung-im-forwarder.md)).

**Remains:** DNSSEC covers only signed zones, and that is the smaller part of
the internet. For an unsigned zone a compromised upstream can still lie, and in
forwarder mode there is no remedy against that — a recursor would not have one
either. Turning validation off (`privacy.dnssec = false`) returns you to the
state before.

### A4 — The attacker inside your own LAN

**Can:** flood the resolver with queries, use DNS tunneling for exfiltration,
run DNS rebinding against other devices in the LAN.

**AlpenDNS against it:** per-client rate limiting, a tunneling heuristic,
rebinding protection (private IPs in answers for public names are discarded).

**Remains:** a device in the LAN can bypass AlpenDNS by asking 1.1.1.1
directly, or by opening DoH to a provider of its own. The only thing that helps
is the firewall — block port 53 outbound, block known DoH endpoints. That is a
network job, not a resolver feature.

### A5 — Whoever has physical access to the box

**Can:** anything.

**AlpenDNS against it:** little, beyond storing as little as possible worth
taking. Default logging is aggregated with a k-anonymity threshold; there is no
query log for anyone to seize. That is the real value of the zero-log default —
not encryption, but the non-existence of the data.

### A6 — The operator against their own users

A DNS server in a family network is a surveillance tool. Whoever configures a
children's policy can also read queries.

AlpenDNS does not make that impossible, but it makes the default state "do not
record" and the switch to `full` a visible, documented act in the
configuration. The UI shows the active log mode prominently.

## What is explicitly not covered

* **Traffic analysis.** The timing, sizes and frequency of queries remain a
  signal even under encryption. Padding helps against lengths, not against
  timing.
* **A compromised client.** Malware on the laptop bypasses any resolver.
* **Malware protection.** The heuristics detect patterns, not threats. A DGA
  score of 0.9 means "looks generated", not "is malicious".
* **Anonymity.** AlpenDNS is a privacy tool, not an anonymity tool. The
  difference is not cosmetic.

## The project's own attack surface

The greatest realistic damage does not come from an attacker but from the code:

1. **A panic in the request path** is a remote DoS for the whole network. Hence
   the hard Clippy rules in CLAUDE.md B.1.
2. **An open resolver** is an amplification reflector. Hence rate limiting, and
   default listeners on private addresses only.
3. **A dependency with network access** breaks the telemetry promise without
   the author doing anything. Hence `cargo deny`, and the requirement to
   justify every new dependency.
4. **False positives in the heuristics** break the internet and lead the user
   to switch everything off. Hence `flag` is the default, not `block`.
