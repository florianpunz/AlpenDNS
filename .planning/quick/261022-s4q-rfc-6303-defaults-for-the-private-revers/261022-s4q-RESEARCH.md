# Quick Task 261022-s4q: RFC 6303 §4 defaults for the private reverse zones — Research

**Researched:** 2026-10-22
**Domain:** DNS reverse-zone policy in the `LocalBackend` layer
**Confidence:** HIGH (RFC list read from the RFC; all integration points read in the code)

## Summary

The change belongs entirely inside `crates/alpendns/src/local.rs`, in
`LocalBackend`. `Config::validate` needs no edit: it iterates only the parsed
`self.local_zone` / `self.forward_zone` vectors, so a compiled-in table that
never enters those vectors is invisible to the ADR-0029 overlap check. The one
non-obvious part is the override rule: `LocalBackend::new` currently discards
every `forward_zone` that is not nested under a configured local zone
(`local.rs:229-233`), so the built-in gate needs the **full** forward-zone list
that `new` already receives, or a forward zone such as the shipped
`168.192.in-addr.arpa` (example config line 120) would be shadowed by the
built-in NXDOMAIN.

The answer must stay exactly what a closed `[[local_zone]]` produces today:
`ResponseCode::NXDomain`, no SOA, no `AA`, `RA` set, `RD` mirrored
(`local.rs:342-352`, `364-370`). Because the cache only caches negative answers
that carry an SOA (`cache.rs:245-247`), the built-in NXDOMAIN is **not** cached —
identical to today's closed local zones, and the reason a per-query hit on the
built-in is cheap but never served from cache.

**Primary recommendation:** add a `const RFC6303_ZONES: &[&str]` table (33 names)
and a pre-parsed, depth-sorted `builtin: Vec<(Name, Arc<str>)>` field to
`LocalBackend`; consult it only after every configured local or forward zone
has been ruled out, and record `Step::LocalAnswer` with the zone label so the
query log proves no upstream step.

## User Constraints (from CONTEXT.md)

### Locked Decisions
- **List scope:** Full RFC 6303 §4: the IPv4 `in-addr.arpa` entries and the IPv6
  `ip6.arpa` entries. The same leak is closed for v6.
- **Answer type:** NXDOMAIN without a synthetic SOA, exactly as
  `LocalZone::is_closed` answers a closed zone today. No new message synthesis.
- **Override and validation:** A configured `[[local_zone]]` or
  `[[forward_zone]]` wins over the built-in default. `Config::validate` sees
  only configured entries; the built-in table must not trigger overlap errors,
  so a configuration that starts today keeps starting.
- **Claude's Discretion:** Where the table lives and how it is looked up,
  following the existing lookup order in `LocalBackend`/`ZoneRouter`; static
  slice versus lazy structure; keep the hot path allocation-free where the
  existing code allows it.

### Deferred / Out of Scope
- No switch to turn the default off ("do not ask strangers about my LAN" is not
  a preference).
- No PTR records derived from `local_zone` A records (ADR-0023).

## Architectural Responsibility Map

| Capability | Primary Tier | Secondary Tier | Rationale |
|---|---|---|---|
| Built-in reverse-zone lookup | `LocalBackend` (`local.rs`) | — | It already owns "answer locally, never ask upstream" and sits above `ZoneRouter` |
| Configured override precedence | `LocalBackend` + `ZoneRouter` | `Config::validate` | Configured zones win; the router still owns forwarding |
| Overlap rejection | `Config::validate` | — | Must see only configured vectors; no change |
| Trace/log of the answer | `Step::LocalAnswer` | `logging::QueryLog` | Names leave only through the existing logging layer (B.1 rule 3) |

## Standard Stack

No new dependency. The implementation uses only `hickory_proto::rr::Name`
(already imported in `local.rs`), `Arc<str>`, and the existing `Step` enum.
`cargo deny`/`deny.toml` are untouched.

## The RFC 6303 §4 zone list (verbatim from the RFC)

Read directly from `https://www.rfc-editor.org/rfc/rfc6303.txt` §4.1–4.6
(downloaded and read in this session). §4 says: *"The following subsections are
the initial contents of the IANA registry as described in the IANA
Considerations section."* — so this is the static list the task names; it is not
the live IANA registry. 33 entries: 25 IPv4, 8 IPv6.

```text
# §4.1 RFC 1918 (18)
10.in-addr.arpa.
16.172.in-addr.arpa.  17.172.in-addr.arpa.  18.172.in-addr.arpa.
19.172.in-addr.arpa.  20.172.in-addr.arpa.  21.172.in-addr.arpa.
22.172.in-addr.arpa.  23.172.in-addr.arpa.  24.172.in-addr.arpa.
25.172.in-addr.arpa.  26.172.in-addr.arpa.  27.172.in-addr.arpa.
28.172.in-addr.arpa.  29.172.in-addr.arpa.  30.172.in-addr.arpa.
31.172.in-addr.arpa.  168.192.in-addr.arpa.

# §4.2 RFC 5735 / RFC 5737 (7)
0.in-addr.arpa.
127.in-addr.arpa.
254.169.in-addr.arpa.
2.0.192.in-addr.arpa.
100.51.198.in-addr.arpa.
113.0.203.in-addr.arpa.
255.255.255.255.in-addr.arpa.

# §4.3 IPv6 unspecified / loopback (2)
0.0.0.0.0.0.0.0.0.0.0.0.0.0.0.0.0.0.0.0.0.0.0.0.0.0.0.0.0.0.0.0.ip6.arpa.
1.0.0.0.0.0.0.0.0.0.0.0.0.0.0.0.0.0.0.0.0.0.0.0.0.0.0.0.0.0.0.0.ip6.arpa.

# §4.4 IPv6 unique-local (1)
d.f.ip6.arpa.

# §4.5 IPv6 link-local (4)
8.e.f.ip6.arpa.  9.e.f.ip6.arpa.  a.e.f.ip6.arpa.  b.e.f.ip6.arpa.

# §4.6 IPv6 example prefix (1)
8.b.d.0.1.0.0.2.ip6.arpa.
```

`[VERIFIED: RFC 6303 §4.1-4.6, rfc-editor.org/rfc/rfc6303.txt]`

Note on the two 32-nibble names: each has 34 labels (`32 + ip6 + arpa`);
`num_labels()` on the IPv6 literal is therefore 34, on `d.f.ip6.arpa` it is 4.
The exact zero counts above are 32 and 31 respectively (the RFC's `\` line
breaks are formatting only).

## Exact integration points

### 1. `LocalBackend` — `crates/alpendns/src/local.rs:206-251`

Fields and constructor:

```rust
pub struct LocalBackend<B> {
    /// Absteigend nach Tiefe sortiert, damit die spezifischste Zone gewinnt.
    zones: Vec<LocalZone>,
    /// Die `[[forward_zone]]`-Namen aus der Konfiguration.
    forward_zones: Vec<Name>,
    ddr: Option<(Name, Arc<str>)>,
    inner: B,
}
```

`[VERIFIED: crates/alpendns/src/local.rs:206-221]`

The constructor already receives **all** forward-zone names and filters them:

```rust
        forward_zones.retain(|forward| {
            zones.iter().any(|local| {
                forward.num_labels() > local.zone.num_labels() && local.zone.zone_of(forward)
            })
        });
```

`[VERIFIED: crates/alpendns/src/local.rs:229-233]`

`main.rs` passes the full list (`zones.iter().map(|(name, _)| name.clone())`,
`main.rs:302-303`), so the built-in gate can clone it **before** `retain` into a
new field (e.g. `configured_forward: Vec<Name>`). Do this; do not widen
`forward_zones`, which exists only for the nesting/delegation check.

### 2. The response path — `local.rs:253-356`

Order today: (a) class-filter and first question, (b) most-specific configured
local zone, (c) nested-forward delegation, (d) configured entry, (e) DDR
NODATA, (f) closed configured zone → NXDOMAIN, (g) `inner`.

Closed-zone answer to copy verbatim:

```rust
            if let Some(zone) = local_zone
                && zone.is_closed()
            {
                let mut response = response_to(request);
                response.metadata.response_code = ResponseCode::NXDomain;
                ctx.record(Step::LocalAnswer {
                    zone: zone.label(),
                    records: 0,
                });
                return Ok(response);
            }
```

`[VERIFIED: crates/alpendns/src/local.rs:342-352]`; `is_closed` is
`matches!(self.fallback, Fallback::Nxdomain)`
`[VERIFIED: crates/alpendns/src/local.rs:134-136]`; `response_to` sets no `AA`
and sets `recursion_available = true` `[VERIFIED: crates/alpendns/src/local.rs:364-370]`.

**Where the built-in branch goes:** immediately after (f), before the final
`self.inner.resolve(...)`. It must be guarded by **both**:

```rust
if local_zone.is_none()
    && !self.configured_forward.iter().any(|f| f.zone_of(query.name()))
    && let Some((zone, label)) =
        self.builtin.iter().find(|(z, _)| z.zone_of(query.name()))
{
    let mut response = response_to(request);
    response.metadata.response_code = ResponseCode::NXDomain;
    ctx.record(Step::LocalAnswer {
        zone: Arc::clone(label),
        records: 0,
    });
    return Ok(response);
}
```

- `local_zone.is_none()` implements "a configured `[[local_zone]]` claims the
  name, even if it is open (`fallback = "upstream"`)" — such a zone must keep
  sending the rest upstream.
- The `configured_forward` check implements the same for `[[forward_zone]]`
  entries that are not nested under a local zone (they normally reach the
  `ZoneRouter` via `inner`, which the built-in would otherwise pre-empt).

### 3. Table and construction

Follow the DDR pattern (`const DDR_ZONE: &str = "resolver.arpa.";`,
`Name::from_ascii(DDR_ZONE).ok()`, `local.rs:197, 234-236`) but as a slice.
Because the literals are compile-time constants, `.ok()` + `filter_map` is
acceptable under B.1 (this is not network data); a unit test must assert that
all 33 parse so a typo cannot silently shrink the list.

```rust
/// RFC 6303 §4 — reverse zones that are never asked outward.
const RFC6303_ZONES: &[&str] = &[ /* 33 absolute, lowercase literals */ ];

// in new(), before the retain above:
let configured_forward = forward_zones.clone();
let mut builtin: Vec<(Name, Arc<str>)> = RFC6303_ZONES
    .iter()
    .filter_map(|text| {
        Name::from_ascii(text)
            .ok()
            .map(|name| (name, Arc::from(*text)))
    })
    .collect();
builtin.sort_by_key(|(name, _)| std::cmp::Reverse(name.num_labels()));
```

Storing `(Name, Arc<str>)` keeps the hot path allocation-free: `zone_of` is a
label comparison, and the trace label is a clone of one precomputed `Arc`.

### 4. `Config::validate` — no edit, but a regression test

```rust
    fn validate_local_zones(&self) -> Result<(), ConfigError> {
        build_zones(&self.local_zone)?;
        for local in &self.local_zone {
            for forward in &self.forward_zone {
                if local.zone.0 == forward.zone.0 {
```

`[VERIFIED: crates/alpendns/src/config.rs:1350-1367]`

The loop touches only configured vectors. Keeping built-ins out of
`Config::local_zone` (and out of `build_zones`) is sufficient. Add a test that
a config containing `[[forward_zone]] zone = "168.192.in-addr.arpa"` still
validates (the shipped example config does exactly this at
`config/alpendns.example.toml:120`), and keep the existing equal-zone error test
(`config.rs:2071-2078`).

### 5. What must NOT change

- `Config::rebinding_allow_zones` (`config.rs:1437-1451`) — built-ins answer
  NXDOMAIN and produce no addresses, so there is nothing for the detector to
  inspect. No entry needed.
- `alpendns check` (`main.rs:1033-1094`) — it iterates configured vectors.
  Optional nicety: print one line stating the RFC 6303 defaults are active.
- `build_zones` duplicate detection — built-ins are not passed to it.
- No new config key, so `deny_unknown_fields` and ADR-0024 are unaffected.

## Common Pitfalls

1. **Suffix matching without label boundaries.** Never compare
   `name.to_ascii().ends_with("in-addr.arpa.")`; `not10.in-addr.arpa.` and
   `10.in-addr.arpa.example.com.` would match. `Name::zone_of` is the correct
   primitive: it compares trailing labels and is case-insensitive
   `[VERIFIED: hickory-proto 0.26 source, name.rs:394-412: `pub fn zone_of(&self, name: &Self) -> bool { self.zone_of_with(name, <[u8]>::eq_ignore_ascii_case) }`]`.
2. **Off-by-one in 172.16/12.** The list is `16.172` … `31.172`, not `12.172`
   and not `32.172`. Test `15.172.in-addr.arpa.`, `32.172.in-addr.arpa.` and the
   two endpoints explicitly — `32.172` is public address space.
3. **Covering a configured forward zone.** The bug the current `retain` invites:
   `forward_zones` is filtered down to nested entries, so using it for the gate
   would let the built-in answer over a configured forward zone. Use the full
   list, and test with the shipped example's `168.192.in-addr.arpa` forward.
4. **Open configured local zone.** `fallback = "upstream"` on the same (or a
   broader) name must suppress the built-in and pass through; guard on
   `local_zone.is_none()`.
5. **Apex vs subdomain.** `zone_of` matches the zone itself, so
   `10.in-addr.arpa.` gets NXDOMAIN too — same as a closed configured zone.
   All qtypes are answered, not just PTR.
6. **Case and trailing dot.** Built-in literals must be absolute (trailing dot,
   `is_fqdn = true`) and are compared case-insensitively by `zone_of`; `Name`
   equality/hash would care about `is_fqdn`, but the lookup here uses
   `zone_of`, not a `HashMap`. Still store absolute lowercase names: their
   `Arc<str>` label appears in the trace and should look like a zone label.
7. **Cache/serve-stale expectations.** NXDOMAIN without SOA is deliberately not
   cached
   `[VERIFIED: crates/alpendns/src/cache.rs:245-247: „Negative Antworten (NXDOMAIN und NODATA) richten sich nach RFC 2308 §5 nach dem SOA der Authority-Section; ohne SOA gibt es keinen belastbaren Anhaltspunkt und wir cachen nicht."]`.
   Do not add a synthetic SOA "to make it cacheable" — that contradicts the
   locked decision and would also make AlpenDNS look authoritative.
8. **Trace/logging.** Use `Step::LocalAnswer`; do **not** add a
   `BlockReason`/`Synthesized` mapping — `trace.rs:111-120` explains that
   `server::build_event` reads that pattern as "blocked". The query name still
   leaves only through `logging::QueryLog`.
9. **Non-IN class.** The class filter is already before the branch
   (`local.rs:263-265`); keep the built-in after it, so CH etc. passes through.
10. **Zero/relative question names.** The branch only runs when
    `request.queries.first()` matched the IN filter; the missing-question path
    must keep passing through unchanged (existing test
    `a_request_without_a_question_is_passed_through`).

## Validation Architecture

Nyquist validation is enabled (no `workflow.nyquist_validation` opt-out found).

| Requirement | Behavior | Test type | Command / file |
|---|---|---|---|
| 33 zones parse, none dropped | table integrity | unit | `cargo test -p alpendns rfc6303` in `local.rs` |
| Private PTR answered NXDOMAIN, upstream hit 0 | leak closed | unit + integration | `local.rs` counter test; `tests/local_zones.rs` fake-upstream test |
| Label boundary / 172.16-31 edges | no over-match | unit | `local.rs` |
| Configured local/forward overrides built-in | precedence | unit + integration | `local.rs`; `config.rs` validation test |
| Existing configs still validate | no regression | config unit | `cargo test -p alpendns config` |
| Trace names the built-in zone | observability | unit | `Step::LocalAnswer` assertion in `local.rs` |
| No `AA`, no SOA, no answers | answer shape | unit | `local.rs` |

Quick run: `cargo test -p alpendns local`; full gate: the four
Definition-of-Done commands from `docs/TESTING.md`.

Wave 0 gaps: none — `local.rs` has a unit-test module with a `Counter`
backend, and `tests/local_zones.rs` has the fake-upstream harness needed for the
end-to-end proof.

## Security Domain

| ASVS category | Applies | Control |
|---|---|---|
| V5 Input validation | yes | Class/first-question filter stays in front; `zone_of` on parsed `Name`, never on raw strings; no slicing/indexing |
| V6 Cryptography | no | — |
| V2/V3/V4 | no | No auth/session/access-control surface changes |

Threat pattern addressed: **information disclosure** (private LAN reverse names
sent to third-party upstreams). B.1 rule 1 constrains the implementation: no
`unwrap`/`expect`/`panic`/indexing on the query path; `.ok()` on static
literals at construction only, with a test asserting the table length.

## Assumptions Log

| # | Claim | Section | Risk if Wrong |
|---|---|---|---|
| A1 | The task means the RFC's §4 list as of RFC 6303, not later IANA registry additions | RFC list | Missing newer entries would leave a leak; low risk, wording is explicit |
| A2 | "A configured zone claims the name" means zone containment, even for an open (`fallback = "upstream"`) zone | Integration points | If instead entries-only were meant, an open configured zone would fall through to the built-in; the CONTEXT wording ("claims the name") supports containment |
| A3 | Listing the built-ins in `alpendns check` is optional, not part of the acceptance | What must NOT change | Cosmetic; `check` already reflects configured state only |

## Open Questions

1. **Does this need an ADR?** It is a behavior change (names that used to leave
   now get NXDOMAIN) and removes a documented operator choice. ADR-0029's
   consequences already anticipate the rule ("TODO 11 still needs the same
   rule"). Recommendation: a short new ADR recording "built-in RFC 6303 list,
   no switch, configured zones win", since ADRs are never edited.
2. **Mark `docs/TODOS.md` item 11 done?** The task names only the OPERATIONS
   migration note; marking the item done is a one-line courtesy and keeps
   Phase 10 material honest.
3. **Keep or remove the explicit block in `config/alpendns.example.toml`?**
   Lines 115-125 forward `168.192`/`10.in-addr.arpa` to a LAN server; that is a
   legitimate configured override and should stay. The OPERATIONS copy-paste
   block (lines 267-315) is incomplete (14 of 33 zones) and becomes redundant —
   replace it with the migration note.

## Sources

### Primary (HIGH confidence)
- RFC 6303 §4.1–4.6, read from `https://www.rfc-editor.org/rfc/rfc6303.txt` in
  this session — the 33-zone list and the "initial contents of the IANA
  registry" framing.
- `crates/alpendns/src/local.rs` — `LocalBackend` fields/constructor, resolve
  order, closed-zone answer, DDR pattern.
- `crates/alpendns/src/config.rs` — `Config`, `ZoneName`, `validate_local_zones`,
  `rebinding_allow_zones`.
- `crates/alpendns/src/router.rs`, `crates/alpendns/src/main.rs`,
  `crates/alpendns/src/cache.rs` — precedence, wiring, negative-cache rule.
- `docs/TODOS.md` item 11, `docs/adr/0023-lokale-zonen.md`,
  `docs/adr/0029-nested-forward-zone.md`, `docs/OPERATIONS.md` §"Private
  reverse zones".

### Secondary (MEDIUM confidence)
- hickory-proto 0.26 `Name::zone_of` implementation in the local cargo registry
  (`name.rs:394-412`) — confirms label-boundary and case-insensitive matching.

**Research date:** 2026-10-22
