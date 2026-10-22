---
phase: 261022-s4q-rfc-6303-defaults-for-the-private-revers
reviewed: 2026-10-22T21:10:00Z
depth: quick
files_reviewed: 6
files_reviewed_list:
  - crates/alpendns/src/config.rs
  - crates/alpendns/src/local.rs
  - crates/alpendns/tests/local_zones.rs
  - docs/OPERATIONS.md
  - docs/TODOS.md
  - docs/adr/0032-rfc-6303-eingebaut.md
findings:
  critical: 0
  warning: 2
  info: 4
  total: 6
status: issues_found
---

# Quick Task 261022-s4q: Code Review Report

**Reviewed:** 2026-10-22T21:10:00Z
**Depth:** quick (focus-area verification performed on top of the quick scan)
**Files Reviewed:** 6
**Status:** issues_found

## Summary

The change adds the RFC 6303 §4 reverse zones as an always-on fallback in
`LocalBackend`, with configured local and forward zones taking precedence. The
core logic is sound, and the focus areas all check out:

* **Table transcription:** I compared `RFC6303_ZONES` (`local.rs:210-250`) with
  the RFC 6303 §4.1–§4.6 tables: 18 RFC 1918, 7 IPv4 special-use, 2 IPv6
  unspecified/loopback, 1 unique-local, 4 link-local, 1 example prefix = 33
  names; every literal matches, including the 32-nibble names and the reversed
  octet order (`254.169`, `2.0.192`, `100.51.198`, `113.0.203`).
* **Gate order:** `configured_forward` is cloned **before** the `retain`
  (`local.rs:294-302`), and the built-in branch reads it (`local.rs:443-447`).
  A non-nested forward zone therefore reaches `ZoneRouter` instead of being
  pre-empted by the built-in table. This covers the example config's
  `[[forward_zone]] 168.192.in-addr.arpa` (active in
  `config/alpendns.example.toml`; commented out in `packaging/alpendns.toml`).
  Configured local zones win via `local_zone.is_none()` (open, closed, and
  equal-name zones all take precedence).
* **Suffix matching:** hickory-proto's `Name::zone_of` compares label arrays
  from the end, case-insensitively, and returns true for the apex. So
  `example.168.192.in-addr.arpa.` is inside, while `not10.in-addr.arpa.` and
  `10.in-addr.arpa.example.com.` are not.
* **Validation:** no production change in `config.rs`; the built-ins are never
  inserted into `Config::local_zone`/`forward_zone`/`build_zones`, and
  `validate_local_zones` still rejects configured local/forward equality
  (ADR-0029). The new test pins that forward zones in RFC 6303 space validate.
* **B.1:** no new `unwrap`/`expect`/`panic`/indexing on network paths; `unsafe`
  untouched; the built-in answer records only `Step::LocalAnswer` with a
  constant zone label, so query names still leave solely via `QueryLog`.
* **Tests:** `cargo test -p alpendns --lib local::tests` (47 passed),
  `cargo test -p alpendns --test local_zones` (18 passed), the new config test,
  `cargo clippy --all-targets --all-features -- -D warnings`, and
  `cargo fmt --all --check` are all green. The "zero upstream hits" claim is
  proven by counters, and counter-tests provide the positive control.

Two substantive issues remain: the test that is supposed to protect the table
against transcription errors does not actually pin its contents, and the zone
apex is answered NXDOMAIN although RFC 6303 §3 explicitly excepts it. The rest
are documentation/ test-strength notes.

## Warnings

### WR-01: The 33-entry table is not pinned; a parseable typo would go undetected

**File:** `crates/alpendns/src/local.rs:1418-1441`
**Issue:** `rfc6303_the_table_has_33_parseable_zones` asserts only the length
(33), that each literal is absolute/lowercase/parseable, and that the built
table has 33 entries. It never compares the names with the RFC list. The other
tests exercise only `10.in-addr.arpa`, `168.192.in-addr.arpa`, the 172.16/31
edges, and `d.f.ip6.arpa` — 26 of the 33 entries are never looked up. A
parseable typo such as `245.169.in-addr.arpa.` (instead of `254.169`)
or `8.b.d.0.1.0.0.3.ip6.arpa.` would keep every test green while the
corresponding PTR queries silently leak outward again — exactly the failure this
change exists to prevent. The plan's own success criterion is "contains exactly
the 33 RFC 6303 §4 names"; the test verifies only "contains 33 parseable
names". The comment above the test claims it guards against a typo, but it only
guards against a literal that stops parsing.
**Fix:** Pin the content, e.g.

```rust
#[test]
fn rfc6303_the_table_is_the_rfc_list() {
    let mut actual: Vec<&str> = RFC6303_ZONES.to_vec();
    actual.sort_unstable();
    let mut expected = vec![
        // §4.1
        "10.in-addr.arpa.", "16.172.in-addr.arpa.", /* … 17…31 … */
        "168.192.in-addr.arpa.",
        // §4.2
        "0.in-addr.arpa.", "127.in-addr.arpa.", "254.169.in-addr.arpa.",
        "2.0.192.in-addr.arpa.", "100.51.198.in-addr.arpa.",
        "113.0.203.in-addr.arpa.", "255.255.255.255.in-addr.arpa.",
        // §4.3–§4.6
    ];
    expected.sort_unstable();
    assert_eq!(actual, expected);
}
```

Add at least one lookup per §4 group (or iterate the full expected list through
`query_private` and assert zero hits).

### WR-02: The zone apex is answered NXDOMAIN, contrary to RFC 6303 §3's explicit exception

**File:** `crates/alpendns/src/local.rs:443-463` (and the test at
`crates/alpendns/src/local.rs:1445-1449`)
**Issue:** The built-in branch returns NXDOMAIN for every name matched by
`zone.zone_of(query.name())`, including the zone name itself. RFC 6303 §3 says
the local answer applies "with the obvious exception of queries for the zone
name itself where SOA, NS, and 'no data' responses will be returned as
appropriate to the query type". The unit test pins the opposite:
`("10.in-addr.arpa.", "10.in-addr.arpa")` must be NXDOMAIN. NXDOMAIN asserts
that `10.in-addr.arpa.` (and `d.f.ip6.arpa.`, etc.) does not exist, although it
is a defined zone; a client asking for its SOA/NS now gets a false
non-existence answer where it previously got the upstream's zone apex answer.
The ADR documents NXDOMAIN "like a closed local zone" but never records this
deviation from the §3 apex exception, and the docs say "a name inside one"
(which the apex is not).
**Fix:** For the apex, return NOERROR with an empty answer section (NODATA),
which needs no synthetic SOA and is therefore still uncacheable:

```rust
if let Some((zone, label)) = self.builtin.iter().find(|(z, _)| z.zone_of(query.name())) {
    let mut response = response_to(request);
    if zone == query.name() {
        // RFC 6303 §3: the zone name itself is the exception.
        response.metadata.response_code = ResponseCode::NoError;
    } else {
        response.metadata.response_code = ResponseCode::NXDomain;
    }
    // …
}
```

and update the apex assertion in
`rfc6303_a_private_reverse_name_is_answered_nxdomain_without_asking_anyone`. If
the project consciously keeps NXDOMAIN at the apex, record that as a deviation
from RFC 6303 §3 in ADR-0032 instead of leaving the RFC reference unqualified.

## Info

### IN-01: ADR-0032's count of the old copy-paste block is wrong and self-contradictory

**File:** `docs/adr/0032-rfc-6303-eingebaut.md:19-20`
**Issue:** "it took 25 hand-written blocks, and the copy-paste block in
OPERATIONS.md carried only 14 of the 33 zones." The removed OPERATIONS section
contained 10 explicit `[[local_zone]]` blocks; if the `17.172 … 31.172` ellipsis
is expanded it covered 25 of the 33 zones. "14" matches neither count and
contradicts "25 hand-written blocks" in the same sentence.
**Fix:** State the actual count (e.g. "10 explicit blocks, 25 zones if the
17.172–31.172 ellipsis is followed"), or drop the number.

### IN-02: ADR-0032 claims no switch without acknowledging RFC 6303 §3's requirement

**File:** `docs/adr/0032-rfc-6303-eingebaut.md:50-51`
**Issue:** RFC 6303 §3 says "An implementation of this recommendation MUST
provide a mechanism to disable this new behaviour, and SHOULD allow this
decision on a zone-by-zone basis"; §1 says "unless intentionally configured
otherwise". The per-zone override satisfies the SHOULD, but the ADR rejects a
global disable without mentioning that this deviates from the MUST. (No
configuration change is proposed here — the decision is defensible for a
forwarder, but the omission is a documentation gap in the decision record.)
**Fix:** One sentence in ADR-0032 recording the deviation from §3's disable
requirement, with the reason already given ("not a preference").

### IN-03: The "not cached" test does not prove non-caching

**File:** `crates/alpendns/src/local.rs:1623-1643`
**Issue:** `override_the_builtin_answer_is_not_cached` asserts only that the
Authority section is empty. That is the precondition AlpenDNS's cache currently
uses (`crates/alpendns/src/cache.rs:265-274` returns `None` without a SOA), but
the test would still pass if that cache rule changed. The test's comment admits
that `CachingBackend` is unreachable from the module, yet
`crates/alpendns/tests/local_zones.rs` builds the real cache and could assert
the cache counters instead.
**Fix:** Add a pipeline test there (query twice, assert cache inserts/hits show
no stored entry, or assert the second query is still answered below the cache);
keep the unit test as the contract check.

### IN-04: "Shipped example config" is ambiguous about which file is meant

**File:** `docs/OPERATIONS.md:268-272`
**Issue:** The paragraph says "The shipped example config does exactly this:
its explicit `168.192.in-addr.arpa` and `10.in-addr.arpa` forward zones remain
legitimate overrides". Those forward zones are active in
`config/alpendns.example.toml` but commented out in the shipping
`packaging/alpendns.toml`; "shipped example config" can be read as the `/etc`
shipping configuration and lead an operator to believe the built-in default is
disabled on a fresh install.
**Fix:** Name `config/alpendns.example.toml` explicitly.

---

_Reviewed: 2026-10-22T21:10:00Z_
_Reviewer: Claude (gsd-code-reviewer)_
_Depth: quick_
