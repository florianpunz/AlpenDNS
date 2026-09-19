# ADR-0017: Oblivious DoH via `odoh-rs`, key fetch direct

**Status:** accepted · **Date:** 2026-08-30 · **Concerns:** [FEATURES.md](../FEATURES.md) P4, [ADR-0002](0002-hickory-proto-statt-eigenem-parser.md)

## Context

Every other privacy measure of the project *distributes* the problem "the
upstream knows your IP". `split_by_zone` ensures that no provider sees more than
a fraction of the domains — but each one sees its fraction together with the
sender. Over weeks that still adds up to a profile.

ODoH (RFC 9230) solves it instead of distributing it: the query is encrypted for
the target resolver and sent through a proxy. The proxy sees the address and an
opaque block; the target sees the question and, as sender, the proxy.

## Decision

### The crypto comes from `odoh-rs`

`odoh-rs` is Cloudflare's reference implementation, BSD-2-Clause, two files of
source, and delivers the ODoH wire format, the parsing of the published
configuration and the HPKE underneath (RFC 9180, `hpke`).

The alternative would have been to take only `hpke` and write the framing
ourselves. The same argument as in ADR-0002 speaks against that: the info
strings, the key derivation and the AAD construction are the place where an
error silently lifts the whole protection — the message would still be
encrypted, only wrongly, and nobody would notice, because the answer arrives
anyway.

`odoh-rs` builds no connections of its own; we do the HTTP on top of it with
`reqwest`, which is in the tree for the blocklists anyway (B.1 rule 4 thereby
stays untouched: only configured targets are contacted).

As a fourth dependency `hpke` itself joins in — solely to be able to name
`hpke::rand_core::RngCore`, which `odoh-rs` carries in its signature. Ten lines of
adapter over our `rand` are cheaper than a second randomness system in the tree.

### The key fetch goes directly to the target, not through the proxy

That is the unpleasant spot, and it belongs named rather than left out.

Once per process start AlpenDNS fetches the target's public key from its
`/.well-known/odohconfigs`. That one connection does **not** go through the proxy
— so the target does see the address in the process. It sees not a single
question in the process, and it learns only that someone is here who will use
ODoH later; not when and not for what. All following queries run through the
proxy.

Going through the proxy would be the clean way. It is not open: an ODoH proxy
accepts exclusively `application/oblivious-dns-message` and is not a general HTTP
proxy. RFC 9230 §6.1 leaves open where the configuration comes from and names
paths outside the protocol; that is why `OdohTransport::new` takes the address of
the key as a parameter instead of building it. Whoever has it from elsewhere does
not have to smuggle it in through a detour. The normal case is delivered by
`well_known_config_url`.

### The startup error instead of the half effect

With ODoH switched on, **every** resolver in the pool must speak `doh://` — ODoH
exists only over HTTP. A `dot://` next to it is a startup error with a reason,
not a silent pass-through in the cleartext path. A setting that does nothing for
half the queries is worse than one that does not exist (B.1 rule 5). Likewise an
`http://` proxy is a startup error: over cleartext an eavesdropper would see the
target address and the time of every query.

### DNSSEC applies over ODoH too

The ODoH transport is wrapped as a `DnsHandle`, so that `DnssecDnsHandle`
(ADR-0016) can be placed in front of it. Without that detour `privacy.dnssec = true`
would silently do nothing with ODoH switched on — again a setting without effect.
The price: the chain queries run through the proxy as well, each one its own HTTP
round. The validation cache limits that to once per zone.

## Limits that no implementation can fix

**Proxy and target must not belong to the same operator.** Otherwise one party
knows both and ODoH is an elaborate DoH detour. No code can check that; it stands
in the example configuration above the key.

**The proxy knows whom you are talking to.** `targethost` stands as a parameter
in the URL — there is no other way, it has to pass it on, after all. So it knows:
this address asks this provider. Not: what.

**Additional latency.** One more round, plus the cryptography. In a household
network that is the cache-miss case and therefore rare.

**The selection is manageable.** There are few public ODoH proxies and targets.
That is why the setting is off by default: switched on without having
deliberately chosen the two addresses, it would be a reassurance without cover.

## What is checked

`crates/alpendns/tests/odoh.rs` builds the full chain on loopback: a proxy that
passes on without being able to decrypt, and a target with a real key pair. The
most important test searches the bytes that went through the proxy for the labels
of the query name — they are not in there. In addition: the key is fetched
exactly once and held afterwards; an answer changed by the proxy is discarded
instead of delivered; a dead proxy yields an error instead of a hang.
