# ADR-0028: False `bogus` from hickory for a CNAME into another zone

**Status:** accepted · **Date:** 2026-10-17 · **Concerns:**
[ADR-0016](0016-dnssec-validierung-im-forwarder.md), `crates/alpendns/src/dnssec.rs`,
`crates/alpendns/src/upstream/transport.rs`, the dependency `hickory-net`

## Context

The diagnosis of dns1 (2026-10-17) found a class of answers that AlpenDNS drops
as `Bogus` even though they are correctly signed: a NODATA answer that follows a
`CNAME` into another zone. Measured over 8 days: 2,144 client-visible SERVFAILs
from this path — about 1 % of all queries — and roughly 12,400 log lines, because
every background refresh of an expired entry runs into the same verdict.

The mechanism is not in AlpenDNS. `hickory-net`'s `verify_nsec` compares the SOA
of the NODATA proof against the **original** query name:

```rust
if !soa_name.zone_of(&query.name) {
    return nsec1_yield(Proof::Bogus, "SOA record is for the wrong zone");
}
```

With a CNAME into another zone the SOA belongs to the target's zone, which is not
an ancestor of the query name. Present in `hickory-net` 0.26.1 (line 1615), 0.26.2
(line 1742) and on `main` at `fb29151` (line 1937).

**The way to the fix is PR
[#3595](https://github.com/hickory-dns/hickory-dns/pull/3595) "net: verify NSEC
records against CNAME target" at hickory-dns** (open since 2026-04-17, branch
`cname-nsec`, `crates/net/src/dnssec/mod.rs` +43/−4). It is very likely this
case; a short field report with the reproduction is prepared for the PR
(`/tmp/hickory/pr-3595-comment.md`, the human posts it). Until it lands, the
behaviour described here stays.

Our own share is only how we consume the verdict: `dnssec::from_error` accepts
hickory's proof as a *verdict* whenever the answer carries records (which it does
here: CNAME, SOA, NSEC, RRSIGs), and [ADR-0016](0016-dnssec-validierung-im-forwarder.md)
makes `Bogus` terminal. Both are intended; the input is wrong.

Reproduction without network (a test in a hickory checkout, `fb29151`):
`verify_nsec` on the recorded answer returns `Bogus`, expected `Secure`
(issue text: `/tmp/hickory/issue-cname-nsec.md`).

## Decision

**Option A is taken: accept it and document it as a known false-alarm source.**
Validation stays on and `Bogus` stays terminal; the class is named where it is
seen (observation week) so that the false positives are not counted as findings
against the detectors. Option B stays below as the rejected alternative — it is
the only lever we have, and it costs exactly the protection ADR-0016 was written
for.

### Option A — accept it and document it as a known false-alarm source

Keep validation on, keep `Bogus` terminal, and name the class in OPERATIONS.md
(observation week) so that the false positives are not counted as findings
against the detectors.

* **For:** no change on the security-critical path. The failure is visible and
  bounded (~1 %), only affects negative answers of CNAME chains into zones whose
  names AlpenDNS would not otherwise need. The end is a hickory release.
* **Against:** until then, affected names never enter the cache — they are
  served from `serve_stale` or as SERVFAIL, and each retry costs an upstream
  round trip and three failed attempts for the pool. A device that polls such a
  name keeps a share of the pool marked down.

### Option B — rejected: switch off our own validation for now

`privacy.dnssec = false`: rely on the upstream's AD bit. The transport is
authenticated with DoT/DoH/DoQ and the three configured upstreams validate
themselves.

* **For:** the class disappears immediately, without touching the code.
* **Against:** what stays unprotected is exactly what
  [ADR-0016](0016-dnssec-validierung-im-forwarder.md) was written for — the
  compromised or lying upstream. The AD bit is a claim by the machine we decided
  not to trust; TLS authenticates the *channel*, not the *data*. A forged answer
  on the encrypted path would be passed on to the client, and the client's own
  validator would only notice if the zone is signed **and** the client validates.
  In addition the AD bit would have to be taken from the upstream into our
  answer again, which contradicts the addendum of 2026-09-19 in ADR-0016.

## End condition

The state ends with a hickory release that contains the fix (PR #3595 or an
equivalent change). The proof is a test that pins it: the recorded answer from
this diagnosis as a fixture, validated through hickory, expected `Secure` — the
same shape as the reproduction above, but in `crates/alpendns/tests/dnssec.rs`
against the pinned version. Until that test is green, this ADR describes the
accepted state: the class is a known false-alarm source and is named as such in
the observation report.
