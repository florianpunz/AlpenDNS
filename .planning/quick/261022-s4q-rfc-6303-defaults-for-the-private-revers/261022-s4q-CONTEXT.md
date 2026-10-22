# Quick Task 261022-s4q: RFC 6303 defaults for the private reverse zones - Context

**Gathered:** 2026-10-22
**Status:** Ready for planning

<domain>
## Task Boundary

Compile the RFC 6303 §4 zone list into the binary as a fallback used only when
no configured `[[local_zone]]` or `[[forward_zone]]` claims the name. A
configured entry overrides the built-in default. `Config::validate` keeps
rejecting configured local/forward overlap per ADR-0029 but must not treat the
built-in defaults as configured entries, so existing configurations keep
starting. A name inside a built-in zone answers NXDOMAIN like a closed local
zone. Migration note in `OPERATIONS.md`.

</domain>

<decisions>
## Implementation Decisions

### List scope
- Full RFC 6303 §4: the IPv4 `in-addr.arpa` entries and the IPv6 `ip6.arpa`
  entries. The same leak is closed for v6.

### Answer type
- NXDOMAIN without a synthetic SOA, exactly as `LocalZone::is_closed` answers a
  closed zone today (`crates/alpenDNS/src/local.rs`). No new message synthesis.

### Override and validation
- A configured `[[local_zone]]` or `[[forward_zone]]` wins over the built-in
  default.
- `Config::validate` sees only configured entries; the built-in table must not
  trigger overlap errors, so a configuration that starts today keeps starting.

### Claude's Discretion
- Where the table lives and how it is looked up, following the existing lookup
  order in `LocalBackend`/`ZoneRouter`.
- Static slice versus lazy structure; keep the hot path allocation-free where
  the existing code allows it.

</decisions>

<specifics>
## Specific Ideas

- `docs/TODOS.md` item 11 names the overlap rule and the answer explicitly.
- No switch to turn the default off: "do not ask strangers about my LAN" is not
  a preference.
- No PTR records derived from `local_zone` A records (ADR-0023).

</specifics>

<canonical_refs>
## Canonical References

- `docs/TODOS.md` item 11 — finding, design, steps, effort
- `docs/adr/0023-lokale-zonen.md` — local zones are a static table
- `docs/adr/0029` — forward zone deeper than local zone wins
- RFC 6303 §4 — the zone list
- `crates/alpendns/src/local.rs`, `crates/alpendns/src/config.rs`

</canonical_refs>
