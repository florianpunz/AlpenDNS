# Decisions

Extracted from the 25 ADR classifications in the `adr-core` manifest. One entry per ADR; status reflects the classification's `locked` field and the ADR's own status line. Nothing is inferred beyond the source documents.

## ADR-0001: Architecture decisions are written down
- source: docs/adr/0001-adr-prozess.md
- status: locked (accepted)
- decision: Every decision that is hard to reverse or that looks wrong from the outside gets a short, consecutively numbered document in `docs/adr/` with context, decision, consequences and alternatives, one page. An ADR is not changed; it is replaced by a new one (`Status: superseded by ADR-00xx`).
- scope: ADR process; docs/adr; decision documentation; architecture decisions

## ADR-0002: DNS wire format via `hickory-proto`, not written ourselves
- source: docs/adr/0002-hickory-proto-statt-eigenem-parser.md
- status: proposed (not locked: status line reads "partly superseded by ADR-0007 (transport crate)")
- decision: `hickory-proto` for DNS message parsing and serialization; everything above it (cache, filtering, policy, upstream selection, heuristics, API, UI) is our own code. The original clause "`hickory-resolver` is used for the upstream transports" is superseded by ADR-0007 (`hickory-net`). `hickory-server` is not adopted as a framework; v1 does no recursion.
- scope: hickory-proto; DNS wire format; message parsing; serialization; hickory-resolver (superseded transport clause); hickory-server

## ADR-0003: Forwarder in v1, recursion left open — but not pre-built
- source: docs/adr/0003-forwarder-first.md
- status: locked (accepted)
- decision: v1 is a forwarder. Resolution sits behind a `ResolveBackend` trait with exactly one implementation (`ForwardBackend`). No root hints management, no unused delegation data structures, no recursion configuration keys, and no other single-implementation traits are built in advance.
- scope: forwarding resolver; recursion; ResolveBackend; ForwardBackend; root hints; QNAME minimisation; DNSSEC validation

## ADR-0004: Aggregated logging as the default, query log only on request
- source: docs/adr/0004-logging-default-aggregiert.md
- status: locked (accepted)
- decision: Four logging modes — `none`, `aggregate` (default), `ring`, `full`. In `aggregate` a domain appears in any output only from `aggregate_k` hits (default 5); the active mode is displayed permanently in the UI. Addendum: `aggregate` counts with exact salted-hash counters instead of a count-min sketch (ADR-0015); the four modes and the default stay.
- scope: logging; privacy; query log modes; none; aggregate; ring; full; k-anonymity; aggregate_k; ring buffer; ring_seconds

## ADR-0006: The TSIG panic is fixed upstream — we wait for 0.27 instead of reporting
- source: docs/adr/0006-tsig-panic-in-hickory-behoben.md
- status: locked (accepted)
- decision: The third condition from ADR-0005 (report the TSIG panic upstream) is dropped; no issue is opened. Conditions 1 and 2 stay: acceptance proven in the delivery configuration (`-O`, overflow checks off; 2,698,186 runs without a crash) and the case kept in `crates/alpendns/fuzz/known-crashes/`. When `hickory-proto 0.27` appears, update and re-check the known crash; if it passes, `known-crashes/`, ADR-0005 and this ADR disappear together. Do not move to an alpha.
- scope: TSIG panic; hickory-proto 0.26.1; hickory-proto 0.27; fuzzing; known-crashes; upstream issue

## ADR-0007: Transports from `hickory-net`; pool and selection stay ours
- source: docs/adr/0007-hickory-net-statt-hickory-resolver.md
- status: locked (accepted)
- decision: Use `hickory-net` for the DoT/DoH/DoQ transports; do not use `hickory-resolver`. Connection lifecycle, upstream selection, failure detection, cache and privacy handling remain ours. Use the `-ring` crypto variants, not `aws-lc-rs`.
- scope: hickory-net; hickory-resolver; upstream transports; DoT; DoH; DoQ; connection lifecycle; upstream pool and selection; cache; crypto backend

## ADR-0008: The `HashMap` stays — Bloom filter and inverted trie are not coming
- source: docs/adr/0008-hashmap-statt-bloom-und-trie.md
- status: locked (accepted)
- decision: The `HashMap` with suffix lookup stays. Bloom filter, label reversal and inverted trie are not built, based on measurements with two million entries (hit p99 620 ns, no-hit p99 880 ns; 135 MB, about 69 bytes per entry). Memory footprint, not latency, is the number that would reverse the decision.
- scope: blocklist matching; HashMap suffix lookup; Bloom filter; inverted trie; label reversal; memory footprint; lookup latency

## ADR-0010: Two listeners, one token, and a UI without a build step
- source: docs/adr/0010-api-ui-und-metriken.md
- status: locked (accepted)
- decision: Two listeners: the token-protected API on `api.listen` and the tokenless metrics on `metrics.listen`; putting both on the same port is refused. The token is generated at startup if missing with permissions 0600, and is permitted as a URL query parameter for Server-Sent Events (`/api/events`). The UI is compiled into the binary via `include_str!` with no build step. The Prometheus text format is produced by hand. A domain name must never become a Prometheus label.
- scope: HTTP API; Prometheus metrics; web UI; authentication token; Server-Sent Events; include_str!; Prometheus text format

## ADR-0011: `split_by_zone` is the only upstream strategy
- source: docs/adr/0011-eine-upstream-strategie.md
- status: locked (accepted)
- decision: `fastest` and `round_robin` are removed; `split_by_zone` remains as the only value of `upstream_pool.strategy`. Both old names remain recognized through a hand-written `Deserialize`. `strategy::round_robin` remains for `by_zone` fallback paths; `by_latency` is gone. The EWMA remains for metrics and status display.
- scope: upstream_pool.strategy; split_by_zone; fastest; round_robin; by_latency; EWMA; alpendns_upstream_rtt_seconds

## ADR-0012: `fanout` is dropped — exactly one upstream is always queried
- source: docs/adr/0012-fanout-entfaellt.md
- status: locked (accepted)
- decision: `fanout` is removed. The pool queries upstreams one after another in `split_by_zone` order and stops at the first success. `ResolveBackend::resolve` takes `&mut Ctx` again; the trace mutex is gone. The `fanout` key remains recognized and raises a validation error with a reason.
- scope: upstream_pool.fanout; upstream pool; split_by_zone; decision trace; Mutex; Ctx; ResolveBackend; config validation

## ADR-0013: The block mode `refused` is dropped
- source: docs/adr/0013-block-modus-refused-entfaellt.md
- status: locked (accepted)
- decision: `refused` is removed. `nxdomain` (default), `zero_ip` and `sinkhole` remain. `BlockMode` gets a hand-written `Deserialize` explaining the removal. No remaining block mode produces REFUSED.
- scope: blocking.mode; refused; nxdomain; zero_ip; sinkhole; BlockMode; REFUSED RCODE; filtering

## ADR-0014: The parsers for Adblock syntax and RPZ are dropped
- source: docs/adr/0014-adblock-und-rpz-parser-entfallen.md
- status: locked (accepted)
- decision: The Adblock and RPZ blocklist parsers are removed. `hosts`, `domains` and `wildcard` remain. Both old format names remain recognized through a hand-written `Deserialize` that names the reason and points to `wildcard` and `hosts`.
- scope: blocklist formats; Adblock parser; RPZ parser; hosts; domains; wildcard; exception rules; Format Deserialize

## ADR-0016: Validate DNSSEC yourself, even as a forwarder
- source: docs/adr/0016-dnssec-validierung-im-forwarder.md
- status: locked (accepted)
- decision: AlpenDNS recomputes the DNSSEC signature chain itself from compiled-in root keys; `privacy.dnssec = true` by default. An answer whose chain does not close is discarded and the client gets SERVFAIL. One `Bogus` record makes the whole answer Bogus; `Bogus` is terminal and does not count as an upstream failure; signatures go only to clients with DO and are trimmed at the outer edge behind the cache. The request's CD bit does not disable validation; the configuration decides.
- scope: DNSSEC validation; forwarder; signature chain; root trust anchors; AD bit; DO bit; CD bit; cache; Bogus; Unproven; dnssec-ring; RUSTSEC-2026-0009

## ADR-0017: Oblivious DoH via `odoh-rs`, key fetch direct
- source: docs/adr/0017-oblivious-doh.md
- status: locked (accepted)
- decision: ODoH through Cloudflare's `odoh-rs`, with `reqwest` for HTTP. The target's key is fetched directly from `/.well-known/odohconfigs` once per process start, not through the proxy. With ODoH on, every pool resolver must speak `doh://` and an `http://` proxy is a startup error. DNSSEC applies over ODoH.
- scope: Oblivious DoH; odoh-rs; HPKE; ODoH proxy; key fetch; DNSSEC; reqwest; DnssecDnsHandle

## ADR-0018: Public Suffix List and seed rotation for `split_by_zone`
- source: docs/adr/0018-public-suffix-list-und-seed-rotation.md
- status: locked (accepted)
- decision: The registrable domain comes from the compiled-in Public Suffix List (`psl` crate). The `split_by_zone` seed is redrawn every 24 hours by default via `[[upstream_pool]] seed_rotation`; `"0s"` restores the previous behavior. Rotation runs as its own task. The answer cache is untouched.
- scope: split_by_zone; Public Suffix List; psl crate; registrable domain; seed_rotation; upstream_pool; answer cache

## ADR-0019: Heuristics report, they do not block
- source: docs/adr/0019-heuristiken-melden-statt-blocken.md
- status: locked (accepted)
- decision: All five phase-8 detectors (DGA, tunneling, rebinding, typosquat, NRD) default to `flag`, not `block`. Four levels `off`/`log`/`flag`/`block`; thresholds live in code and are measured. `NameDetector` and `AnswerDetector` run at different pipeline points; heuristics run last. A detector without its data is `off`. `forward_zone` is exempt from rebinding protection.
- scope: detection heuristics; DGA detection; tunneling detection; rebinding protection; typosquat detection; NRD detection; detection modes; thresholds; NameDetector; AnswerDetector; Engine::evaluate; forward_zone exemption

## ADR-0020: Above the limit, packets are dropped, not refused
- source: docs/adr/0020-rate-limiting-verwirft.md
- status: locked (accepted)
- decision: Above the per-client rate limit, packets are dropped silently — no REFUSED or SERVFAIL. IPv6 clients are aggregated to /64; IPv4-mapped IPv6 shares the IPv4 bucket. Token buckets live in 16 sharded, mutex-protected drawers with an LRU client cap (default 8192). Throttling runs before message parsing.
- scope: rate limiting; packet dropping; amplification reflection; IPv6 /64 aggregation; token bucket; LRU client cap; alpendns_rate_limited_total; query pipeline

## ADR-0021: Rebinding flags only addresses where a device can sit
- source: docs/adr/0021-rebinding-nur-erreichbare-adressen.md
- status: locked (accepted)
- decision: The rebinding detector's private-address list contains only addresses where a local device can sit: loopback, RFC 1918, link-local, broadcast, CGNAT 100.64/10 and benchmark 198.18/15. `0.0.0.0`, `::` and `192.0.0.0/16` are removed.
- scope: rebinding detector; private address ranges; 0.0.0.0; ::; 192.0.0.0/16; RFC 1918; loopback; link-local; CGNAT; benchmark; NAT64; ipv4only.arpa; allow_zones

## ADR-0022: Glass as a surface
- source: docs/adr/0022-glas-als-flaeche.md
- status: locked (accepted)
- decision: The UI background may carry colour without meaning and is the only place where that is allowed: four deeply desaturated colour fields over a base colour, below the text threshold. The rest of B.6 stands (four semantic colours plus `--brand`, greyscale ramp, centred container, 8-based spacings, one kind of container, now a glass edge with 18px radius). The mode is set as `data-theme` on the root element; contrast is computed at 4.5:1; every surface with text is a pane.
- scope: web UI; glass surface; background colour fields; backdrop-filter; data-theme; contrast; prefers-reduced-transparency; web/app.css

## ADR-0024: Config compatibility on renames
- source: docs/adr/0024-config-kompatibilitaet-bei-umbenennungen.md
- status: locked (accepted)
- decision: No automatic config migration and no version field. On a rename, the old key remains as a `serde(alias)` for at least one major version with a deprecation warning; unknown keys get a Levenshtein-based suggestion. `deny_unknown_fields` stays. The Debian conffile contract rules out rewriting `/etc/alpendns/alpendns.toml`.
- scope: config parsing; serde alias; deprecation warning; deny_unknown_fields; Levenshtein suggestions; Debian conffile contract; automatic migration

## ADR-0025: Rebinding detector switches to block
- source: docs/adr/0025-rebinding-auf-block.md
- status: proposed (not decided; code default stays `flag` per locked ADR-0019)
- decision: Proposed, not decided: the rebinding detector would default to `block`, after ADR-0021's corrected address list produced zero findings in the second observation period. The code default stays `flag` (ADR-0019) until the maintainer decides. `allow_zones` is the escape hatch.
- scope: rebinding detector; block default; flag default; observation period; false positives; private address ranges; allow_zones

## ADR-0026: DGA, tunneling, typosquat, NRD stay on flag
- source: docs/adr/0026-andere-detektoren-bleiben-auf-flag.md
- status: proposed (not decided; code default stays `flag` per locked ADR-0019)
- decision: Proposed, not decided: DGA, tunneling, typosquat and NRD remain on `flag` until their respective observation criteria are met (1-2 weeks on real traffic, false-positive review, rate below 0.1% or an equivalent for that detector). The code default stays `flag` (ADR-0019). Rebinding is the exception proposed in ADR-0025.
- scope: DGA detector; tunneling detector; typosquat detector; NRD detector; detector defaults; flag mode; block mode; observation criteria; false-positive rate

## ADR-0027: Resolve failures are cached briefly, with a backoff
- source: docs/adr/0027-aufloesungsfehler-kurz-cachen.md
- status: locked (accepted)
- decision: `CachingBackend` caches failed resolutions per `(name, type, class)` for 5 s, doubling the TTL per repeated failure up to 60 s (`FAILURE_TTL`, `FAILURE_TTL_MAX`) and resetting on success. At most 1024 entries; no configuration key; a `Step::FailureCached` is added to the trace. A stale answer is served before the failure memory.
- scope: CachingBackend; failure cache; SERVFAIL; backoff; RFC 9520; caching.rs; cache.rs; upstream pool

## ADR-0028: False `bogus` from hickory for a CNAME into another zone
- source: docs/adr/0028-falsches-bogus-hickory-cname.md
- status: locked (accepted)
- decision: Accept the false `Bogus` verdict from `hickory-net`'s `verify_nsec` for a NODATA answer following a CNAME into another zone as a known false-alarm source. DNSSEC validation stays on and `Bogus` stays terminal; the class is named in OPERATIONS.md during the observation week. The state ends with a hickory release containing PR #3595 or an equivalent change.
- scope: hickory-net; verify_nsec; DNSSEC validation; CNAME; NODATA; Bogus; serve_stale; upstream pool; privacy.dnssec; observation week

## ADR-0029: The more specific zone wins, whatever its type
- source: docs/adr/0029-nested-forward-zone.md
- status: locked (accepted)
- decision: For local and forward zones, the longest (most specific) match wins regardless of type; a more specific forward zone is delegated from `LocalBackend` to the `ZoneRouter`. Two zones of the same name remain a startup error. This replaces ADR-0023's no-overlap rule and refines the "entry beats zone" rule inside the winning zone.
- scope: local_zone; forward_zone; zone precedence; LocalBackend; ZoneRouter; config validation; alpendns check; RFC 6303 reverse zones; resolver.arpa (DDR)
