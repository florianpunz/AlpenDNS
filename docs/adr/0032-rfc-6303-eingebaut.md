# ADR-0032: The RFC 6303 reverse zones are built in

**Status:** accepted · **Date:** 2026-10-22 ·
**Concerns:** `crates/alpendns/src/local.rs` (`LocalBackend`),
`crates/alpendns/src/config.rs` (`validate_local_zones`),
`docs/OPERATIONS.md`, `docs/TODOS.md` item 11

## Context

A PTR query for `192.168.1.5` used to leave the house: without a configured
reverse zone, `LocalBackend` passed the question to the upstream. That is a
privacy leak — the question names a device in the operator's own network — and
it was also the source of unstable answers, because upstreams treat the private
ranges differently: some answer NXDOMAIN, some SERVFAIL. On dns1 the effect was
measured twice in the observation period (ADR-0029).

[RFC 6303](https://www.rfc-editor.org/rfc/rfc6303#section-4) §4 lists the reverse
zones that must never be asked outward. Closing them was already possible with
`[[local_zone]] fallback = "nxdomain"`, but it took ten hand-written blocks —
twenty-five zones once the `172.16/12` ellipsis was filled in — and even then
eight of the 33 zones stayed uncovered. A default that depends on the operator
copying a list correctly is not a default.

## Decision

**The RFC 6303 §4 list is compiled into the binary as a fallback table.** It
holds the 33 names of §4.1 to §4.6 — 25 IPv4 `in-addr.arpa` zones and 8 IPv6
`ip6.arpa` zones — as a static `const` in `local.rs`. A name inside one is
answered by `LocalBackend` itself.

The details, in the order the request passes through:

* **NXDOMAIN without a synthetic SOA**, exactly as a closed local zone answers
  today: no `AA`, empty answer section, `RA` set, `RD` mirrored. A synthetic SOA
  would claim an authority AlpenDNS does not have, and it would make the answer
  negatively cacheable; without one the cache has no anchor (RFC 2308 §5), so
  the built-in answer is cheap but never cached — the same contract as a closed
  `[[local_zone]]`.
* **A configured zone wins.** The built-in branch runs only when no configured
  `[[local_zone]]` contains the name (an open zone with `fallback = "upstream"`
  keeps sending the rest outward) and no configured `[[forward_zone]]` contains
  it. The forward check uses the full list captured before the constructor's
  `retain`, which drops exactly the forwarding entries without a local zone
  above them; the retained list would let the built-in pre-empt the
  `ZoneRouter`.
* **`Config::validate` sees only configured entries.** The built-ins never enter
  `Config::local_zone`, `Config::forward_zone` or `build_zones`, so the
  ADR-0029 overlap rule is unchanged and a configuration that starts today keeps
  starting — including the shipped example's `168.192.in-addr.arpa` and
  `10.in-addr.arpa` forward zones, which stay legitimate overrides.
* **No dedicated switch, but the configured zones are one.** RFC 6303 §3 says an
  implementation "MUST provide a mechanism to disable this new behaviour, and
  SHOULD allow this decision on a zone-by-zone basis". That mechanism is the
  override rule itself: a `[[local_zone]]` with `fallback = "upstream"` for
  `10.in-addr.arpa` makes the built-in step aside and sends the name outward
  again. There is no global off key; "do not ask strangers about my LAN" is not
  a preference.
* **Two deliberate deviations from §3, both about authority.** §3 asks for
  `AA=1` on these answers and excepts "queries for the zone name itself", where
  SOA, NS or NODATA should come back. AlpenDNS sets no `AA` and answers
  NXDOMAIN at the apex too. The reason is the same one that forbids a synthetic
  SOA: AlpenDNS hosts no zone, so it claims no authority and invents no record —
  and the apex of `10.in-addr.arpa` is not a name anyone resolves. A client that
  needs the distinction is asking the wrong server.
* **No PTR records derived from `local_zone` A records.** That stays as
  ADR-0023 decided; the built-in table answers NXDOMAIN and invents nothing.
* **The trace says so.** The answer records `Step::LocalAnswer` with the
  built-in zone label, like any other local answer; a query name still leaves
  the pipeline only through `logging::QueryLog` (B.1 rule 3).

This resolves the TODO 11 consequence that ADR-0029 recorded: a configured
zone, local or forwarded, overrides the built-in default, and the most specific
zone wins.

## Consequences

* **A private reverse name no longer leaves the process.** The upstream sees
  neither the address nor the fact that it was asked. The counter tests pin the
  zero.
* **Existing configurations are unaffected.** The built-in table is invisible
  to validation and to `alpendns check`; the migration note in OPERATIONS.md
  says so, and that the old copy-paste block may be deleted.
* **The table is the RFC's list, not the live IANA registry.** A later registry
  addition is not picked up until it is added here — deliberate, so the
  behaviour does not change between restarts without a release.
* **`LocalBackend::new` gains no parameter and no config key**, but the struct
  grows two private fields (`configured_forward`, `builtin`). The crate is a
  library without an API stability promise (see ADR-0029).
* **A configured zone that is equal to a built-in one is still allowed** — it
  is an ordinary configured zone. What remains an error is the same zone as
  both `[[local_zone]]` and `[[forward_zone]]` (ADR-0029).
* **No rebinding exemption is needed.** The built-in answer carries no
  addresses, so the detector has nothing to inspect.

## Alternatives

* **Ship more `[[local_zone]]` blocks in the example config.** Rejected: it
  moves the correctness of a privacy guarantee into every operator's file, and
  a forgotten entry fails silently outward.
* **A configuration switch for the default.** Rejected: the leak is not a
  preference; an opt-in default would repeat the problem it fixes.
* **A synthetic SOA so the answer becomes cacheable.** Rejected: it makes
  AlpenDNS look authoritative for zones it does not host, contradicting
  ADR-0023's "a table, not a zone file".
* **A lazy structure or a lookup tree for the 33 names.** Rejected: the list is
  static, compared with `Name::zone_of` on the hot path, and a linear scan over
  33 entries costs nothing measurable.
