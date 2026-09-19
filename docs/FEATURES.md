# Feature Catalog

Ideas with an assessment. Not all of them get built — that is the point of a list with an
assessment. Every entry has: what it is, why it is interesting, what it costs, and where
the honest limits lie.

**Legend**

* **Effort:** S (one evening) · M (2–4 evenings) · L (a week+)
* **Novel:** how unusual this is among existing resolvers —
  ○ standard · ◐ rare · ● practically nowhere
* **Phase:** where it sits in the [roadmap](ROADMAP.md)

---

## P — Privacy

### P1 · Zero logging with k-anonymity `Effort M` `Novel ◐` `Phase 6/7`

Instead of "logging on/off", four modes, default aggregated with a threshold: a domain
appears in statistics only once it has been queried at least `k` times (default 5).
Rationale and details: [ADR-0004](adr/0004-logging-default-aggregiert.md).

**Why this is more than a setting:** the domains queried exactly once are precisely the
telling ones. Conventional query logs store them with a timestamp and client IP. Here
they cease to exist after the answer.

**Limit, resolved:** this was first implemented with a count-min sketch, and that
overestimates rare elements. The k threshold therefore checked against the *lower*
estimate bound — correct, but the bound grows with traffic: at a million requests it stood
at 11 and thus above any usual `k`, so that **no** domain appeared in the statistics any
more. Measured in [BENCHMARKS.md](BENCHMARKS.md); since then counting is exact
([ADR-0015](adr/0015-exakte-zaehlung-statt-sketch.md)). The guarantee stayed the same, only
now it holds under traffic too.

### P2 · Upstream splitting by zone `Effort M` `Novel ●` `Phase 3/7`

**The most interesting feature of the project.** Instead of sending all requests to one
resolver, the upstream is determined by `hash(seed, registrable_domain) % n`.

Consequences:

* The same name always goes to the same resolver → the cache stays fully effective,
  unlike with round-robin.
* Each resolver sees only about `1/n` of your domains. With three upstreams, none sees
  more than a third of your profile.
* The seed is drawn at random at startup: after a restart each provider sees a *different*
  third. Over time none of them learns a stable picture.
* Because the assignment hangs on the registrable domain (not on the full name),
  `mail.example.com` and `cdn.example.com` land at the same resolver — the structure of a
  visited site thus does not reveal itself to several providers.

**Limits that need documenting:** every upstream still sees your IP. Popular domains are
distributed the same way for all users. An attacker with access to several of the
configured upstreams cancels the protection — the selection should therefore cover
different operators and jurisdictions.

**Followed up in phase 7** ([ADR-0018](adr/0018-public-suffix-list-und-seed-rotation.md)):
the registrable domain came from an approximation ("last two labels") and for
`shop.example.co.uk` yielded the ineffective `co.uk` — all `.co.uk` names landed at one
upstream. Now it comes from the Public Suffix List, compiled in. And the seed no longer
lasts until restart but is by default redrawn every 24 hours: the sentence "over time none
of them learns a stable picture" previously only held for someone who also restarts.
Measured: 10,000 domains across four pool sizes and eight seeds, deviation per upstream
below 5 %.

### P3 · Privacy budget `Effort S` `Novel ●` `Phase 7`

The server counts which share of requests went to which upstream, and shows it in the UI:
*"Quad9 has seen 34 % of your domains, Mullvad 33 %, dnsforge 33 %."*

Small to implement, but it turns an abstract promise into a checkable number. It also
exposes misconfigurations — when one upstream gets 90 % because the others keep failing,
you see that immediately instead of never.

### P4 · Oblivious DoH as a client `Effort M` `Novel ◐` `Phase 7`

RFC 9230. The request is encrypted for the target resolver and sent through a proxy: the
proxy knows your IP but not the request; the resolver knows the request but not your IP.
This is the only technique in this list that actually solves the problem "the upstream
knows your IP" instead of distributing it.

**Limit:** needs a proxy and an ODoH-capable target resolver that must *not* belong to the
same operator — otherwise the protection is theatre. The choice is manageable. Additional
latency from the extra hop.

**Implemented in phase 7** ([ADR-0017](adr/0017-oblivious-doh.md)), default off. Two limits
learned and documented: the proxy knows *whom* you are talking to (`targethost` must be in
the URL, otherwise it cannot forward), and the target's public key is fetched directly from
it once per process start — that one connection does not go through the proxy, and during
it the target sees the address, but no question. Going through the proxy would not work:
it accepts ODoH messages only. When enabled, the configuration demands that *all* resolvers
in the pool speak `doh://` — a `dot://` next to them would be a promise not kept for every
request.

### P5 · DDR/DNR — raise clients to encryption automatically `Effort M` `Novel ●` `Phase 10`

RFC 9462 (DDR) and RFC 9463 (DNR). A client asks `_dns.resolver.arpa` at the configured
plaintext resolver and gets back: *"the same service exists encrypted under this address."*
Current operating systems evaluate that and switch from UDP/53 to DoH or DoT on their own.

**Why that is unusual:** practically no home resolver can do it. It makes LAN traffic to
the resolver encrypted without anything having to be configured on a single device — that
is the difference between "I have set up DoH" and "DNS is encrypted throughout the
household".

**Prerequisite:** a certificate the clients trust, with the resolver's IP or name as SAN.
In the LAN that means either a real domain with ACME-DNS-01 or a CA of your own deployed
on the devices.

### P6 · Hygiene basics `Effort S each` `Novel ○` `Phase 3`

Not a unique selling point, but the prerequisite for the rest not being facade: strip ECS
(do not forward RFC 7871), EDNS padding (RFC 7830/8467), DNS cookies (RFC 7873), 0x20
encoding, source port randomisation, TTL cap against long-lived tracking via the DNS
cache.

### P7 · Cover traffic — deliberately discarded

Random fake requests are supposed to hide the real pattern. In practice: high upstream
traffic, and an observer can usually separate real from generated requests (timing,
repetition patterns, distribution of the names). Cost certain, benefit questionable.

The weakened variant, by contrast, makes sense and is already in the cache prefetch
(phase 2): frequently used names are renewed in the background, which produces upstream
traffic that does not correlate with user activity. That is a side effect of a useful
function, not a feature of its own.

---

## D — Detection without a cloud

All detectors deliver a score and a reason, run locally, and are by default on `flag`, not
`block`. A detector that breaks the internet gets switched off — and all the others with
it.

### D1 · DNS rebinding protection `Effort S` `Novel ○` `Phase 8`

Answers with private IPs (RFC 1918, loopback, link-local) for public names are discarded.
Classic protection against attacks in which a website reaches devices in the LAN through
the browser. `dnsmasq` and `unbound` can do it; it is missing from many blocklist
solutions. Needs an exception list for internal zones and for services that do this
legitimately.

**Implemented in phase 8.** The only one of the five that is not a heuristic: a yes/no
rule, score always 1.000, exception list instead of threshold. Glue records in the
additional section are checked too, as is the IPv4 address wrapped as IPv6
(`::ffff:192.168.1.1`) — that would otherwise be the open side entrance. The
`forward_zone` entries automatically enter the exception list; without that, the
protection would be a trap on first start, since your own LAN nameserver naturally answers
with private addresses.

### D2 · DNS tunnelling detection `Effort M` `Novel ◐` `Phase 8`

Data exfiltration over DNS has conspicuous traits: very long labels, high entropy in the
names, many one-off subdomains under one zone, an unusually high share of TXT/NULL
requests, a high request rate against a single zone.

Instead of a single rule: several signals per zone over a time window, combined into a
score. The decisive trick is to assess **per zone** instead of per request — a single long
subdomain is normal, a thousand of them under the same zone are not.

**Limit:** some CDNs and antivirus products look exactly the same. Hence an exception list
and `flag` as default.

**Implemented in phase 8.** Five signals, weighted, with the one-off subdomains per zone
as the heaviest — entropy depends strongly on the encoding (hex gets no further than
4 bits, base64 6), whereas the number of one-off names depends on the thing itself.
Measured (BENCHMARKS.md): ordinary traffic under one zone 0.17, `dnscat2`-like 0.81,
`iodine`-like 1.0; **0.0000 % false positives** on 100,000 real domains. A tunnel with ten
requests per hour deliberately does not stand out — the state for that would be too
expensive.

### D3 · DGA detection `Effort M` `Novel ◐` `Phase 8`

Malware generates domain names algorithmically (`kqxvbnzmrt.com`). A character n-gram model
over a large corpus of normal domains detects that well, is a few hundred kilobytes in
size, needs no GPU and runs in microseconds.

Approach: learn 3-gram probabilities from a popularity list, score = negative
log-likelihood, normalised to the name length. Supplementary features: consonant clusters,
digit share, dictionary hits.

**Limit:** short names are statistically indistinguishable; `bit.ly`, `t.co` and
randomly looking CDN hostnames produce false positives. Wordlist-based DGAs (two real
words joined together) are not detected by the model. Hence: the false positive rate is
the quality measure, not the hit rate.

**Implemented in phase 8**, and the limits above are measured rather than assumed
(BENCHMARKS.md): **0.077 % false positives** at 92.8 % hit rate on alphanumeric, 40.6 % on
necurs-like and 28.6 % on conficker-like names — but **0.5 %** on pronounceable and
**0.0 %** on dictionary-based ones. The last two stand as a test so that the limit remains
a known one and does not become a surprise.

Two classes learned: punycode (`xn--…`) and everything below a *private* suffix
(`cloudfront.net`, `github.io`) are not assessed at all. In the first measurement run, four
of the twenty most conspicuous names were IDNs — a systematic false alarm for entire
language areas. What remains are mostly Pinyin abbreviations like `hnqxdzkj.com`.

### D4 · Typosquat guard `Effort M` `Novel ●` `Phase 8`

You store the domains that matter to you — bank, government portal, employer. Every
resolved domain is checked against this small list: Damerau-Levenshtein distance 1–2,
keyboard adjacency, Unicode confusables (Cyrillic `а` in `sparkasse.at`), IDN homographs,
confusable TLDs.

**Why that is unusual:** existing solutions check against global phishing lists — that is,
against what had already been reported yesterday. Here the comparison runs against *your*
twenty domains, which means a domain registered ten minutes ago and on no list at all
still stands out. The computational cost is trivial because the protection list is small.

**Implemented in phase 8**, with four kinds of hit: homograph (1.000), a foreign name
carrying the protected domain like `sparkasse.at.com` (0.950), typo with distance 1 or 2
(0.950 / 0.850), other TLD (0.900). Punycode is resolved beforehand — a Cyrillic `а`
reaches us as `xn--sprkasse-…` and in that form looks nothing at all like the original.
Five protected domains against 100,000 real names yielded 18 reports and **not a single
time the protected domain itself**.

Useful addition: on a hit, don't block bluntly but serve a sinkhole explanation page —
*"this name resembles sparkasse.at but differs in one character"*. That is the moment the
protection actually takes effect.

### D5 · Newly registered domains `Effort S` `Novel ◐` `Phase 8`

Domains registered less than 30 days ago are disproportionately often malicious. AlpenDNS
reads a local file with domain + registration date and flags hits.

The file comes from your AlpenShield project (CT logs, zone data). The interface is
deliberately a file and not an API call: the resolver must not depend on a second service
running.

**Implemented in phase 8.** The score falls linearly with age — a domain from yesterday is
more suspicious than one from three weeks ago, and the gradation shows that instead of
treating everything in the window the same. A missing file is **not** a startup error: the
detector runs empty and appears in the status as `off`.

### D6 · Explainability is a duty, not a nicety

Every detector delivers not just a score but the features that led to it (*"label entropy
4.7, 340 one-off subdomains in 5 minutes"*). Without that, a false positive is not
debuggable, and you will switch the feature off instead of improving it.

**Implemented in phase 8 as a mandatory field:** `Finding::reason` is not an `Option`.
A detector *cannot* deliver a finding without a reason. This is what that looks like in
operation:

```
Algorithmically generated name reports (score 0.900): 'kqxvbnzmrtwp' does not fit
naturally grown names: 7.3 bits of surprise per character triple, longest
consonant run 12, digit share 0 %
Newly registered reports (score 0.933): 'kqxvbnzmrtwp.com' was registered on 2026-08-28,
2 days ago (threshold: 30 days)
```

Both findings for the same request — which is why all detectors run and not just up to the
first hit. "Freshly registered *and* algorithmically generated" is a different statement
than either part on its own.

The price: a reason carries the query name. It is therefore subject to the same rules as
everything in the trace (CLAUDE.md B.1 rule 3), and since phase 8 the leak test also scans
the list of conspicuous requests.

---

## C — Clients and policies

### C1 · Client identity beyond the IP `Effort M` `Novel ●` `Phase 5`

IP-based assignment breaks as soon as a device leaves the network. AlpenDNS additionally
identifies via DoH path tokens (`/dns-query/<32 random bytes>`) and mTLS client
certificates.

**Consequence:** your phone keeps its policy on the mobile network. The children's tablet
keeps its rules in the neighbours' WiFi too. This is the point at which a home resolver
becomes a personal resolver — and it costs only the token extraction from the path,
because every standard DoH client does that without modification.

### C2 · Schedules `Effort S` `Novel ○` `Phase 5`

Rules that apply at certain times. Needs an injectable clock, otherwise it is not testable.
Time zones and daylight saving are the only real difficulty.

### C3 · Temporary allowances `Effort S` `Novel ◐` `Phase 5`

*"Allow `youtube.com` for 20 minutes."* Via API, UI or CLI. Expires automatically.
The feature that makes the difference when someone besides you lives in the household —
without it, the filter gets switched off completely at the first conflict.

### C4 · Policy simulation `Effort S` `Novel ●` `Phase 5`

```
$ alpendns policy test ads.example.com --client kids-tablet
BLOCKED
  client   kids-tablet          ← matched by ip 10.0.10.42
  policy   kids
  allowlist local-allow         ← no hit
  blocklist oisd-big:118432     ← ||ads.example.com^
  verdict  NXDOMAIN
```

Testing without making the request. That makes a policy change checkable before it goes
live — and it is the basis of the replay harness from [TESTING.md](TESTING.md).

### C5 · Conditional forwarding `Effort S` `Novel ○` `Phase 3`

Internal zones (`home.arpa`, reverse zones of your own network) go to the internal server
and never to the internet. Standard functionality, but indispensable in the homelab —
without it every internal hostname leaks to the upstream.

---

## O — Observability and operation

### O1 · Decision trace and "why was this blocked?" `Effort M` `Novel ●` `Phase 5/6`

Every answer carries a complete chain of reasoning internally (ARCHITECTURE.md §2). The UI
shows it: which list, which line, which policy, which upstream, how long.

**Why this has to be decided early in architectural terms:** you cannot retrofit it.
Either the pipeline collects the steps from the beginning, or later you have only a `bool`
and guess.

### O2 · Breakage detection `Effort M` `Novel ●` `Phase 8+`

If a client requests the same blocked domain several times within a few seconds and then
goes conspicuously quiet, an app has very likely just broken. AlpenDNS recognises this
pattern and suggests in the UI: *"`api.example.com` was blocked 12 times in 4 seconds from
`florian-laptop` — something probably isn't working. Unblock?"*

That reverses the usual order: normally the user notices the disruption and searches the
log. Here the server speaks up before the search begins. Needs no query log — the pattern
is visible in the ring buffer.

### O3 · Blocklist diff review `Effort M` `Novel ●` `Phase 10`

Before applying a list update: *"This update blocks 47 domains newly, 3 of which you
actually visited in the last 30 days: …"*

The comparison runs against the aggregated counters, not against a query log — so it is
possible even under the privacy default. It solves the problem that a list update at some
point quietly breaks something and nobody connects it to the update.

### O4 · Live query stream `Effort S` `Novel ○` `Phase 6`

Server-sent events, in the UI as a running list. Useful when setting up a new device.
Respects the log mode: with `none` only counters flow.

### O5 · Prometheus and clean metrics `Effort S` `Novel ○` `Phase 6`

Queries/s, blocks, cache hit rate, upstream RTT per resolver, error rates, list sizes, RSS.
No metric ever contains a query name — a label with high cardinality is not just a
performance problem here but a data leak.

### O6 · Sinkhole with explanation instead of NXDOMAIN `Effort M` `Novel ◐` `Phase 6+`

Instead of NXDOMAIN, a local IP with a page explaining what was blocked and why. Works only
for HTTP; with HTTPS the connection breaks with a certificate error, which is more
confusing than a clean NXDOMAIN. Hence: configurable, not default, and described in the UI
with that limitation. For D4 (typosquat) it is nevertheless the right choice — there the
explanation is the actual benefit.

---

## Assessment: what first?

If you build only three things that set AlpenDNS apart from everything else:

1. **P2 Upstream splitting** — solves a problem nobody else solves, and is doable with
   moderate effort.
2. **O1 Decision trace** — has to go into the architecture early, makes everything after
   it debuggable, and is the reason the UI can do something other UIs cannot.
3. **D4 Typosquat guard** — small effort, concrete benefit, and the idea "check against
   *my* important domains instead of against global lists" is the core of what a personal
   resolver can do better than a central service.

**C1 (client identity via DoH token)** is the latecomer with the best effort-to-benefit
ratio — technically almost free, in practice the difference between a home resolver and a
personal one.
