# Architecture

This document describes the target picture. What of it already exists is in
[ROADMAP.md](ROADMAP.md).

## 1. Basic shape

AlpenDNS is a single process, a single binary, one configuration file. No
database server, no Redis, no sidecar. That is deliberate: the audience is
someone with a box in the basement who wants it to run.

Inside the process there are five layers that a request passes through in
order:

```
                 ┌──────────────────────────────────────────────┐
  UDP/53 ──┐     │ 1  Listener                                  │
  TCP/53 ──┼────▶│    Terminate transport, parse message,        │
  DoT/853  │     │    rate limit, determine client address       │
  DoH/443 ─┤     └───────────────────┬──────────────────────────┘
  DoQ/853 ─┘                         │  Request { msg, client_id }
                                     ▼
                 ┌──────────────────────────────────────────────┐
                 │ 2  Policy                                     │
                 │    Resolve client → policy                    │
                 │    Allowlist → blocklists → regex → schedule  │
                 │    Heuristics (DGA/tunnel/typosquat)          │
                 └───────────┬──────────────────────┬───────────┘
                             │ Verdict::Block       │ Verdict::Allow
                             ▼                      ▼
                 ┌────────────────────┐  ┌──────────────────────┐
                 │ synthetic answer   │  │ 3  Cache             │
                 │ (NXDOMAIN/0.0.0.0/ │  │    Hit → return      │
                 │  REFUSED/sinkhole) │  │    Miss → continue   │
                 │                    │  │    Stale → parallel  │
                 └─────────┬──────────┘  └──────────┬───────────┘
                           │                        │ Miss
                           │                        ▼
                           │             ┌──────────────────────┐
                           │             │ 4  Resolve backend   │
                           │             │    (trait)           │
                           │             │  ├ Forwarder (v1)    │
                           │             │  └ Recursor (open)   │
                           │             └──────────┬───────────┘
                           │                        │ answer
                           │                        ▼
                           │             ┌──────────────────────┐
                           │             │ 5  Post-processing   │
                           │             │    Rebinding check,  │
                           │             │    validate answer,  │
                           │             │    clamp TTL, cache  │
                           │             └──────────┬───────────┘
                           └────────────┬───────────┘
                                        ▼
                             Decision-Trace → logging/metric/API
```

### The slot where recursion would later hook in

Layer 4 is a trait:

```rust
trait ResolveBackend: Send + Sync {
    async fn resolve(&self, q: &Query, ctx: &Ctx) -> Result<Response, ResolveError>;
}
```

v1 implements exactly one variant: `ForwardBackend`. If a `RecursiveBackend`
joins it one day, nothing changes in layers 1, 2, 3 and 5. That is the entire
preparation for recursion — deliberately, nothing more is built in advance
([ADR-0003](adr/0003-forwarder-first.md)).

## 2. The decision trace

This is the architecturally most important detail, and the reason "why was this
blocked?" is answerable in AlpenDNS.

Every request produces a `Trace` — a small, allocation-light list of steps:

```rust
struct Trace {
    id: u64,                 // monotonic, process-local
    steps: SmallVec<[Step; 8]>,
    verdict: Verdict,
    elapsed: Duration,
}

enum Step {
    ClientMatched { client: ClientId, by: MatchKind },
    PolicyApplied { policy: PolicyId },
    AllowlistHit  { list: ListId, rule: RuleRef },
    BlocklistHit  { list: ListId, rule: RuleRef },
    ScheduleHit   { schedule: ScheduleId },
    Heuristic     { name: &'static str, score: f32, action: Action },
    CacheHit      { ttl_left: u32, stale: bool },
    UpstreamUsed  { pool: PoolId, resolver: ResolverId, rtt: Duration },
    Synthesized   { mode: BlockMode },
}
```

The trace is **always** there, regardless of the log mode. What happens to it
is decided by the logging layer:

| `privacy.logging.mode` | What happens to the trace |
|---|---|
| `none` | increment counters, discard the trace |
| `aggregate` | counters + frequency under a salted hash, counted exactly; the name appears in statistics only from `aggregate_k` hits onwards ([ADR-0015](adr/0015-exakte-zaehlung-statt-sketch.md)) |
| `ring` | additionally in a RAM ring buffer for `ring_seconds`, never on disk |
| `full` | additionally as a structured line on disk |

The UI shows "why" from the ring buffer. That is why the explanation works even
under log mode `ring`, without a query log existing anywhere.

What the interface looks like is not an architecture question: the design
language is in CLAUDE.md B.6, the material in
[ADR-0022](adr/0022-glas-als-flaeche.md). Architecturally the UI is a handful of
static files that live inside the binary and are served over three routes — it
does not know the pipeline, and queries it through the same API every other
client uses ([ADR-0010](adr/0010-api-ui-und-metriken.md)).

**One deviation in the implementation** (phase 5): steps carry names as
`Arc<str>` rather than IDs, so that a trace is readable without the
configuration next to it. Rationale:
[ADR-0009](adr/0009-decision-trace-mit-mutex.md).

The second deviation — the trace behind a `Mutex` instead of passed through
exclusively — is gone again. It had exactly one reason, `fanout > 1`, and that
went away with `fanout` ([ADR-0012](adr/0012-fanout-entfaellt.md)). `resolve`
receives the context as `&mut Ctx`.

## 3. Blocklists: data structure

The naive approach would be a `HashSet<String>` of the domains. At 1–2 million
entries that is 100–200 MB depending on length, and several lookups per request
(one for each suffix level).

The target model:

1. **Reverse and intern the labels.** `ads.example.com` → `com.example.ads`.
   That turns wildcard matching from a suffix problem into a prefix problem.
2. **A Bloom filter in front.** The overwhelming majority of queries are not on
   a list. A Bloom filter with a 1 % false positive rate answers those in ~50 ns
   without a cache miss.
3. **Behind it the exact structure**, consulted only on a Bloom hit.
4. **A match returns a `RuleRef`** (list ID + line number), not just `true` —
   otherwise there is no trace.

**Measured, and for now not built:** phase 4 implemented and measured the simple
variant — two million entries, p99 lookup under one microsecond, 135 MB. No
number justifies the Bloom filter at that point. The target model above stands
as a plan; the decision and the numbers are in
[ADR-0008](adr/0008-hashmap-statt-bloom-und-trie.md).

**Updates without downtime:** lists are loaded into a new structure and swapped
atomically via `arc_swap::ArcSwap`. Requests in flight see the old one, new ones
the new one. No lock in the hot path.

## 4. Cache

* Key: `(name (lowercase), QType, QClass)`. The name is normalized for the key,
  but kept in its original spelling for 0x20 towards the upstream.
* Value: the complete answer plus an expiry instant, not the remaining TTL —
  otherwise every hit needs arithmetic instead of a comparison.
* TTL is clamped to `[min_ttl, max_ttl]`, negative answers separately.
* **Serve-stale (RFC 8767):** on expiry the old answer is served *and* a refresh
  is triggered in parallel. The client does not wait.
* **Prefetch:** entries hit more than N times over their lifetime are renewed in
  the background at 85 % of their TTL. A practical side effect: more even
  upstream traffic, less correlation with "the user was just active".
* **Cache isolation between policies:** the cache stores the *unfiltered*
  answer. Filtering happens before the cache. That way all clients share one
  cache without one client's policy affecting another's answer.

## 5. Upstream selection

A pool is a list of resolvers plus a strategy — and there is only one left:

* `split_by_zone` — the upstream is determined by
  `hash(registrable_domain) % n`, with a seed drawn randomly at startup.
  Consequences: the same name always goes to the same resolver (the cache stays
  effective), but each resolver sees only ~1/n of your domains, and which third
  it sees differs on every restart. Details and limits:
  [FEATURES.md](FEATURES.md), P2.

**Removed:** `fastest` (EWMA of the RTT) and `round_robin`. Both are classic,
and both amount to every upstream having seen everything in the end — exactly
what this project exists to oppose. They stood two paragraphs above their own
refutation. Rationale: [ADR-0011](adr/0011-eine-upstream-strategie.md).

Health checking: passively, via error rates and timeouts, not via active probes
— active probes are themselves a signal.

## 6. Client identity

Evaluated in this order, first match wins:

1. **mTLS client certificate** (DoT/DoQ) — the strongest binding, survives
   network changes.
2. **DoH path token** — `https://dns.miloo.at/dns-query/<32 random bytes>`.
   Works with any standard DoH client, without distributing certificates.
3. **Source IP / subnet** — the normal LAN case.
4. **Default policy** — everything that could not be assigned.

A token belongs in the configuration, not in a log, and is shown in the API
only by its last four characters.

## 7. Configuration and reload

One TOML file, `serde` with `deny_unknown_fields`. Two ways to reload it:

* `SIGHUP` → reparse the configuration. **If parsing fails, the old
  configuration stays active** and the error goes to the log. A reload must
  never lead to a state in which filtering should happen but does not.
* `alpendns check -c /etc/alpendns/alpendns.toml` → validates without a restart,
  usable as `ExecStartPre` in the systemd unit.

The **policy layer** is what is hot-reloadable: clients, policies, regexes,
schedules and the list sources (URLs/formats) are rebuilt and swapped in
atomically on reload. That is exactly the state `run_updater` builds
periodically from `Blueprint` + `Lists` anyway — the reload only wakes it
instead of waiting for the next refresh tick.

Everything else requires a restart: listener addresses, upstreams/TLS,
`forward_zone`, cache configuration, rate limiting, `blocking.mode`/sinkholes,
detectors and `privacy.logging.mode`. The reload names that boundary explicitly
in the log, rather than silently ignoring a change.

## 8. Concurrency

* Tokio multithreaded, one task per request.
* The UDP socket is opened several times with `SO_REUSEPORT` (one socket per
  worker) so the kernel does the distribution — that is the difference between
  30k and 200k requests/s on the same hardware.
* Shared state: `ArcSwap` for everything that rarely changes (lists,
  configuration, policies), a sharded map for the cache. No global `Mutex` in
  the request path.
* **Query deduplication:** 500 clients asking the same uncached name at the
  same time produce exactly one upstream query. Without it, a cache expiry is a
  home-made load spike generator.

## 9. Persistence

Default: **none**. The process keeps everything in RAM.

Optional on disk, each separately switchable:

* Blocklist cache (`/var/cache/alpendns/lists/`) — so a restart filters even
  without internet.
* Aggregated statistics (`/var/lib/alpendns/stats.redb`) — counters per day, no
  query names below the k threshold.
* Cache snapshot on shutdown — optional, saves the cold start.

No query log, unless the operator explicitly turns on `full`.

## 10. Error handling

| Situation | Behaviour |
|---|---|
| Upstream timeout | next resolver in the pool; all dead → `serve_stale`, otherwise SERVFAIL |
| Blocklist not loadable | last cached version, log the error, raise the metric, do not prevent startup |
| Blocklist not loadable on first start | abort startup — better no DNS than unfiltered DNS |
| Broken request from a client | FORMERR, raise the counter, no log spam per packet |
| Answer does not match the question | discard, do not cache, count as a possible spoofing attempt |
| Config reload fails | old config stays, error to the log |
| Panic in a task | must not happen (see CLAUDE.md B.1); if it does, `panic = "abort"` — a half-broken resolver is worse than a restarting one |
