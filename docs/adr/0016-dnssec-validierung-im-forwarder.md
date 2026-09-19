# ADR-0016: Validate DNSSEC yourself, even as a forwarder

**Status:** accepted · **Date:** 2026-08-30 · **Concerns:** [ADR-0002](0002-hickory-proto-statt-eigenem-parser.md), [ADR-0003](0003-forwarder-first.md), [THREAT-MODEL.md](../THREAT-MODEL.md)

## Context

AlpenDNS is a forwarder. The answer comes from Quad9, Mullvad or whoever else,
and with it an AD bit — a single bit, set by exactly the machine toward which the
rest of the project exercises restraint. Encrypted transports, stripping ECS,
`split_by_zone`: all of it assumes that an upstream might be curious. For the AD
bit the assumption so far was: it will be right.

THREAT-MODEL.md has carried that as an open point since phase 1: *"without DNSSEC
validation AlpenDNS trusts the upstream. A compromised upstream can lie."* In
forgoing recursion, ADR-0003 explicitly noted that validation of our own is
possible and sensible in forwarder mode too.

## Decision

**AlpenDNS recomputes the signature chain itself, starting from the compiled-in
root keys.** On by default (`privacy.dnssec = true`). An answer whose zone
declares itself signed and whose chain does not close is discarded; the client
gets SERVFAIL. That is the standard behavior per RFC 4035 §5.5 and what `unbound`
and `knot-resolver` do.

The cryptography comes from `hickory-net`/`hickory-proto` (`dnssec-ring`), for
the same reason as in ADR-0002 for the wire format: to write a signature
verification chain yourself is the kind of code where an error goes unnoticed,
because the wrong result looks exactly like the right one. Our share sits in
`crate::dnssec` and is the *evaluation*: from many record stamps a verdict per
answer, and the conclusion drawn from it.

Three follow-on decisions that look arbitrary from the outside:

**1. The summary is pessimistic.** A single `Bogus` record makes the whole answer
rotten. Otherwise an attacker could hang a forged record next to real ones and
get through. Both the answer **and** the authority section are looked at: a
negative answer carries its proof in the NSEC records of the authority, and if
only the answer section were checked, every forged NXDOMAIN would pass as "no
statement".

**2. `Bogus` is terminal, no second upstream is asked.** The opposite would be
obvious — maybe only one of them is lying. Two things speak against it. A zone
with a broken signature is broken at *every* provider, so the second attempt
would almost always bring the same result; and it would show the name to one more
provider, which is exactly what `split_by_zone` is meant to prevent. For the same
reason `Bogus` does **not** count as a failed attempt for outage detection:
otherwise a single broken zone could mark the whole pool as dead after three
queries and trigger a self-made outage.

**3. The signatures go only to a client with the DO bit,** and the stripping
happens at the **outer edge, behind the cache** (`dnssec::for_client` in
`server::handle_request`). Both only became right in operation, see below. The
AD bit in our answer stands for *our* verdict and goes only to a client that has
set DO or AD (RFC 6840 §5.8).

## Price

**`time` is now a production dependency, and that changes the justification of
an advisory exception.** `hickory-proto/dnssec-ring` pulls the `time` crate in
through its internal `__dnssec` feature. Until phase 7 `time` came in only via
`rcgen` and thus as a dev-dependency; the exception for RUSTSEC-2026-0009 in
`deny.toml` therefore read "is not in the binary at all". That is no longer true.

The exception stays anyway, with a narrower justification: the advisory concerns
the *parsing* of RFC 2822 date strings from foreign input. From `time`, hickory
uses exclusively `OffsetDateTime`, in the constructors of RRSIG and SIG; the
timestamps in them arrive as 32-bit numbers from the wire, not as a string. The
vulnerable path is not entered. The full text including the reversal condition
stands in `deny.toml`.

The alternative would have been to raise the MSRV from 1.85 to 1.88 — then the
resolver picks `time 0.3.55` and the exception falls away without replacement.
What stood against it was the Debian packaging from phase 9: Debian 13 ships
`rustc 1.85`, and with 1.88 the `.deb` could no longer be built with the
distribution's compiler. As soon as the MSRV rises for another reason, the
exception disappears.

**Additional queries.** For every new zone the validating handle fetches DNSKEY
and DS sets, over the same connection to the same upstream. A cache on top of
that sits in `DnssecDnsHandle`. There are still more queries than before, and the
first resolution in a zone takes longer.

**The cache holds more bytes than before.** It stores the answer together with
the signatures, so that a client with DO can still get them; trimming happens
only on delivery. The price is a few hundred bytes per signed entry.

## What is checked

`crates/alpendns/tests/dnssec.rs` runs three vectors with **real** signatures
through the same check that runs in operation: valid signature → `Secure` and the
answer goes through; flipped bit in the signature → `Bogus` and the answer is
discarded; missing signature in a demonstrably signed zone → likewise `Bogus`.

The setup pins the test zone's key as trust anchor instead of building a chain up
to the real root — `DnssecDnsHandle` stops looking for DS records at a key from
the anchor store. Nothing in the computation is shortcut in the process.

That the validating handle is really placed ahead of the transport is recorded by
`encrypted.rs::dnssec_sets_the_do_bit_and_asks_for_the_chain`: with
`dnssec = true` the DO bit is on the wire and chain queries go out; without it
exactly one question goes out and the DO bit is missing.

## What the first run against real upstreams showed

Three things no unit test would have found, because all three hang on reality.
They stand here because they explain why the code looks the way it does at these
places.

**`dnssec-failed.org` gave SERVFAIL, but the counter stayed at zero.** The result
was right, the bookkeeping was not: when the upstream validates itself, no answer
with records comes back at all, but an empty SERVFAIL. The NSEC proof then does
not add up, and `hickory` delivers that as an **error**
(`DnsError::Nsec { proof, response }`) instead of a stamped message. That way it
ran past the evaluation: the counter stood still, the upstream was charged a
failed attempt, the connection was discarded and the next provider was presented
with the same question — exactly the three things ruled out above.
`dnssec::from_error` now catches the case and pulls the verdict together with the
answer out of the error.

**`dig` got the whole signature chain without having asked for it.** The
distinction was wrong: the AD bit in a *query* means, per RFC 6840 §5.7, "tell me
your verdict", not "send me the chain" — and `dig` sets it by default. Only the
DO bit requests the records. Since then `client_wants_records` (DO) is separated
from `client_wants_verdict` (DO or AD).

**A `dig +dnssec` got zero signatures, because a `dig` without it had been there
before.** The stripping ran in the transport, that is *below* the cache — and
that holds one answer for all clients (ARCHITECTURE.md §4). Whoever asked first
without DO put the trimmed version into the cache. The cache now holds the
complete, validated answer; `dnssec::for_client` trims at the outer edge, per
client. That is the same separation that puts filtering before the cache: what
holds for everyone belongs in the cache, what holds for one belongs before or
after it.

## Addendum of 2026-08-30: an outage is not a finding

During the run against real upstreams in phase 8, `wikipedia.org` came back once
as SERVFAIL and on the next attempt as NOERROR. The cause was a confusion that
was laid out above.

If the upstream answers SERVFAIL **itself** and sends no records along with it,
`hickory` reports `Bogus` — the NSEC records are missing, after all, with which
something could be proven. On the wire that looks exactly like a zone with a
broken signature. But it is something entirely different: *we have seen nothing
about which a verdict could be reached.*

The difference is expensive, because per point 2 above `Bogus` is **terminal**. A
single hiccup at the upstream thus became a hard SERVFAIL for the client, without
the second, healthy upstream ever having been asked — a self-made outage, and one
that moreover clears itself on the next attempt and is therefore hard to find.

The distinction is now made on what the answer contains: **empty and with an
error RCODE** means outage (`ResolveError::Unproven`), everything else means
verdict. Both together do not exist — a zone with a broken signature delivers
records, otherwise nobody would have anything to check. `Unproven` asks the next
upstream but charges **no one** a failed attempt: otherwise a broken zone could
still drain the pool, which is exactly what point 2 is meant to prevent.

**That makes a number from the roadmap obsolete.** It says of phase 7:
`dnssec-failed.org → SERVFAIL, bogus=1`. The client still gets SERVFAIL, but the
counter now stands at 0 — Quad9 validates itself and delivers an empty error
answer, so we never see a rotten signature. The old 1 was the mislabel, not the
new 0.

## Addendum of 2026-09-19: the client's CD bit no longer decides

A scan over the codebase found the same cause three times (F1, F2, F3): both
transports hung validation on the request's CD bit. The justification stood as a
comment at `dnssec::checking_disabled` — *whoever cancels the check harms only
himself* — and it was wrong, at the same place where point 3 above was already
wrong once.

**The cache is the reason.** It is keyed by (name, type, class) (`crate::caching`);
CD is not in it. The answer that a CD client triggers is the same one the next
client without CD gets. A single client in the LAN would thereby have cancelled
signature validation for the whole network — and not merely theoretically: with
CD set, the upstream delivers exactly the unvalidated answer that DNSSEC is meant
to intercept (THREAT-MODEL.md, point A3).

**What now holds.** `Transport::send_checked` computes only
`let validate = self.privacy.dnssec;`, `OdohBackend::resolve` takes the validating
handle without a condition; `checking_disabled` has no caller left and is gone.
The configuration decides, never the request. Outwardly CD was ineffective
anyway: `DnssecDnsHandle::send` in `hickory-net` sets `checking_disabled = false`
and `authentic_data = true` on the way to the upstream itself. CD had an effect
only inward, toward our own cache — and that is exactly where it did not belong.

It is the same separation as in the third point under "What the first run against
real upstreams showed", only the other way round: back then the trimming moved to
the outer edge, because the cache holds for everyone; here a condition disappears
that never belonged into the cache. What holds for everyone must be decided for
everyone.

**The second half of the recommendation is deliberately not built.** It was also
proposed to honor CD at the outer edge — delete the AD bit and pass the raw data
through to the CD client. That does not exist: whoever sets CD and asks for a
Bogus name gets SERVFAIL like everyone else. To give him data that we have just
computed to be wrong would be a decision of its own about `dnssec::for_client`.
RFC 4035 §3.2.2 describes CD as "do not check"; that we do it anyway is a
deviation — it stands here so that it is one and not an oversight.

**What that costs.** A client that sets CD and validates itself gets SERVFAIL for
a broken zone instead of the data with which it could have reached its own
verdict. Stricter than necessary, but in the direction in which an error shows
instead of staying silent.
`encrypted.rs::a_client_setting_cd_does_not_disable_validation` records that with
CD the DO bit goes out and the chain is followed. The ODoH branch has no test of
its own; it is read, not exercised — the removed condition is a pure tightening,
but one should not rely on that.

**Reversal condition of this addendum:** if in the practical test a client with
CD shows up that runs into SERVFAIL, the answer is not to switch the check off
again but to handle CD in `dnssec::for_client` — per client, at the outer edge,
behind the cache.

## Reversal condition

If in the practical test from phase 9 broken zones lead to outages that nobody
can explain, the answer is **not** to switch validation off but to make the
discarded answers visible in the UI — the "DNSSEC dropped" counter already
stands there in the privacy tile. Only if it turns out that it regularly hits
legitimate zones would a "check, but pass through" mode be worth considering. It
is deliberately not pre-built.
