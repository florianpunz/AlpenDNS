# ADR-0023: A local zone is a table, not a zone

**Status:** accepted · **Date:** 2026-09-23 · **Affects:**
`crates/alpendns/src/local.rs`, `crates/alpendns/src/config.rs` (`[[local_zone]]`),
the pipeline in [ARCHITECTURE.md](../ARCHITECTURE.md) §1

## Context

Two needs had no answer in the configuration:

1. **Manual overrides for FQDNs.** `nas.miloo.at` should point at
   `192.168.1.5` without a second DNS service running in the LAN.
2. **Names that must never see an upstream.** `miloo.at` should not be
   queried.

Both are often wanted together — a home server under one's own domain — and
neither was possible. `[[forward_zone]]` forwards a zone to *another*
nameserver over cleartext UDP into the own network; that is the opposite of
answering it here. Anyone without a second instance (dnsmasq, Knot) had no
way. The word `hosts` in the repository means a *blocklist format*, not local
records.

The seam for the solution already exists: `ResolveBackend` is the one trait a
layer hangs off, and hickory's `RData::try_from_str` takes over parsing the
record values.

## Decision

AlpenDNS answers a configured list of names itself. `[[local_zone]]` is a
**static table** in the configuration file:

```toml
[[local_zone]]
zone = "miloo.at"
ttl = 300
fallback = "upstream"
records = [
  { name = "@",       type = "A",     value = "192.168.1.5" },
  { name = "nas",     type = "A",     value = "192.168.1.5" },
  { name = "drucker", type = "CNAME", value = "nas" },
]
```

Five properties belong to the decision, and each of them is a place where it
could have gone the other way:

* **Not authoritative, and not on the way there.** No SOA, no NS, no
  AXFR/IXFR, no NOTIFY, no signing, and `AA` stays unset in the answer. The
  README's "Not authoritative — to host a zone, use Knot or NSD" stays true;
  this is a lookup table in front of the resolver, not a zone server. The
  boundary is drawn in the module header of `local.rs` so that it does not
  erode record by record.
* **`fallback` decides the names without an entry**, default `upstream`
  (split horizon): a local table pins down the names in it, everything else is
  a zone like any other. `fallback = "nxdomain"` closes the zone and is the
  one-liner for "this domain is nobody else's business" — including with no
  `records` at all.
* **A missing type is NODATA, not a question to the upstream** — also under
  `fallback = "upstream"`. A name must not have two horizons: otherwise A
  comes from the table and AAAA from the outside, and an unrelated type is the
  back door out of the zone.
* **No CNAME chasing.** `drucker → nas` is answered as it stands; the client
  asks for the target itself and hits the same table. One round trip more, in
  exchange for no resolution loop that would need a bound.
* **A local zone and a forwarded zone may not overlap.** With the layer above
  the `ZoneRouter` (see below), a name in both would leave it unpredictable who
  answers. A startup error rather than a rule about precedence.

**Position in the chain:** `Policy → Cache → Local → ZoneRouter → Pool`.
Below the cache, so that local answers are cached with their TTL like any
other, and so that the cache keeps holding the *unfiltered* answer (B.3 rule
1). Above the `ZoneRouter`, because an entry is more specific than a zone:
whoever forwards `miloo.at` to the LAN server *and* pins down `nas.miloo.at`
means the entry — the same rule as "the most specific zone wins", one level
finer. `router.rs` does not change; `LocalBackend` is one more link in the
chain `main` wires up.

**Rebinding protection exempts the entered names, not the zone.** A local
answer is typically `192.168.x.y` and therefore exactly the hit the detector
waits for (ADR-0019, ADR-0021). But under `fallback = "upstream"`,
`www.miloo.at` still comes from outside, and a private address there is the
attack itself — exempting the whole zone would switch the protection off
precisely there. A closed zone is exempt as a whole: nothing leaves the house
from it, so there is nothing to check.

## Consequences

* **Changes to the table need a restart.** Like `forward_zone`. Positive
  answers *are* cached, so after a restart the cache is empty and every change
  takes effect immediately.
* **No synthetic SOA**, therefore an NXDOMAIN from a closed zone is not cached
  (the cache needs a SOA in the authority section for that). Here that is the
  cheaper mistake: the lookup is a `HashMap` access, while an invented SOA
  would claim authority we do not have (MNAME, RNAME, serial) and would keep
  the *old* answer to a changed table alive over `max_negative_ttl` — 15
  minutes by default.
* **A CNAME may point outside the zone.** The client then resolves that name
  normally, i.e. possibly over the upstream. Not a leak of the zone, but the
  one way an entry indirectly points outward.
* **A signed public zone that is overridden locally** can produce answers a
  validating client rejects. That is the known split-horizon problem, not
  ours; a sentence in OPERATIONS.md.
* **An older binary with the new configuration refuses to start** — `[[local_zone]]`
  is an unknown key and `deny_unknown_fields` makes that an error (B.1 rule 5).
  Intended: silently running without the table would be worse.
* The rebinding exemption matches like the zone list it is, so it covers
  subdomains: `nas.miloo.at` takes `x.nas.miloo.at` along. For an entry under
  one's own zone that is a theoretical difference; for an entry under a
  *foreign* zone (the device vendor case) it reaches one level too far.
  Noted at `LocalZone::rebinding_exempt` with the reason it stays.
* **PTR means a reverse zone of its own.** `dig -x 192.168.1.5` needs
  `[[local_zone]] zone = "1.168.192.in-addr.arpa"` with a `PTR` record.
  Generating it from the A records would be convenient and is exactly the magic
  nobody understands later.

## Alternatives

* **A `ZoneTarget` enum in the `ZoneRouter`** (forward / local / block as the
  zone's target), instead of a layer of its own. Rejected: the router would
  have to hold the fallback backend, so the wiring in `main` would have to move
  into `block_on`, and the router would know the pipeline below it. The price of
  the chosen way is the overlap rule above.
* **A zone file parser** (`$ORIGIN`, `$TTL`, `IN A`, includes). Rejected: it
  brings a second configuration language, `$INCLUDE` brings paths, and the
  boundary to "then we are authoritative after all" blurs. TOML is already
  there, is checked by `alpendns check`, and a table that fits in one screen is
  what the audience has.
* **A synthetic SOA for negative answers.** Rejected, see above — it claims
  authority and keeps stale answers alive.
* **PTR generated from the A records.** Rejected for v1, see above.
* **Storing the table in a separate file** (like a hosts file). Rejected: a
  second file to back up, to get the permissions right for and to reload, for a
  table that changes once a year.
