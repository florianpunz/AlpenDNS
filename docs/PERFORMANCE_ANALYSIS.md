# Performance and Lightweightness Analysis

Numbers, not claims — the same principle as in [BENCHMARKS.md](BENCHMARKS.md). This
document collects what stood out while walking the **hot path**: query pipeline,
cache, matcher, upstream pool, the detectors from phase 8.

## Method and measurement status

This analysis is a **reading result**, not a measurement run. Two things therefore
belong up front, otherwise the "measurement result" column reads wrong:

1. **What is measured** is in [BENCHMARKS.md](BENCHMARKS.md) and is only quoted
   here. The measurement machine is Linux (`load.rs` with SO_REUSEPORT and a fake
   upstream in-process, `--test-threads=1`).
2. **What is newly proposed here is not measured yet.** The load generator runs
   only under Linux, the corpus is not in the repo, and the machine on which this
   analysis was written has no Rust toolchain. A candidate therefore does not stand
   there as "faster", only as "worth measuring" — with the command that confirms or
   rejects it.

That is exactly why the table below distinguishes two kinds of entries:

* **measured** — the number comes from BENCHMARKS.md and is proven.
* **pending** — the approach is to be implemented in isolation and measured against
  the baseline before it may go into the `main` branch. Before that it would be
  optimisation without measurement, and that is exactly what the project explicitly
  forbids.

The path from candidate to decision is the same for all pending ones:

```bash
# 1. Baseline (as shipped, expensive case: nothing but new names)
cargo test --release --test load -- --ignored --nocapture --test-threads=1

# 2. Implement in isolation on a perf/<topic> branch, then run the counter-run.
#    What is judged is the comparison of the two rows, not the absolute value.
```

## Result at a glance

| # | Location | Problem | Solution | Measurement result | Effort |
|---|---|---|---|---|---|
| 1 | `server/mod.rs` `build_event` | builds `query_type`, `why` (Vec<String>), `findings` on **every** request, although `aggregate`/`none` discard them | materialise fields only when the log mode needs them | pending | M |
| 2 | `upstream/pool.rs` `is_down` | a `Mutex` lock per upstream per cache miss, although the value is written only on failure | `Mutex<Option<Instant>>` → `AtomicU64` (monotonic microseconds) | pending | S |
| 3 | `policy/mod.rs` `resolve` | `asked` string (to_ascii + lowercase) on every request, needed only for the rebinding check | build it only when an answer detector is attached | pending | S |
| 4 | `detect/dga/mod.rs` | `symbols`/`mean_surprise` allocate a Vec per label; labels are ≤ 63 characters | fixed-size stack buffer instead of a heap Vec | pending | S |
| 5 | `upstream/strategy.rs` `registrable_domain` | PSL lookup + up to three string allocations per cache miss | hash the registrable domain without the last `to_owned()` | pending | S |
| 6 | `detect/typosquat.rs` `embeds` | `format!(".{protected}")` three times per entry per request | compute it once when the detector is built | pending | S |
| — | detectors, total | run on every allow request | **none** — an accepted price, see below | **measured: ~10 %** | — |
| — | tunneling mutex | a global mutex per request | **none for now** — reversal condition documented | **measured: part of the 10 %** | — |
| — | cache hit `with_ttl` | deep clone of the answer per hit | **none** — the price of the hit | **measured: p50 18.8 µs** | — |

The last three rows are **not candidates** — they are in the table because they are
the loudest things in the hot path, and the decision to leave them as they are is
part of this analysis (see "Rejected approaches").

---

## The candidates in detail

### 1. `build_event` materialises name and reason in every log mode

**Current state.** [server/mod.rs:206–239](crates/alpendns/src/server/mod.rs#L206-L239)
builds a `QueryEvent` for every answered request, and in it stand — always — `name`,
`query_type`, `why` (a `Vec<String>`, one `format!` call per `Step`, see
[trace.rs:108–170](crates/alpendns/src/trace.rs#L108-L170)) and `findings`. In the
quiet modes the logging layer throws most of that away:

| Field | needed in `none` | in `aggregate` (default) | in `ring`/`full` |
|---|---|---|---|
| `name` | no | yes (salted counter) | yes |
| `query_type` | no | **no** | yes |
| `why` | no | **no** | yes |
| `findings` | no | **no** | yes |

So in the default `aggregate` mode `query_type`, `why` and `findings` are formatted
on *every* request and then discarded. `why` is the expensive part: a `write!` per
step with name, duration or score — two to four steps for a normal allow hit, all as
their own string allocation.

**Approach.** Build the fields only when the mode needs them. Concretely: `build_event`
(or `QueryLog::record`) checks `mode.keeps_names()` and otherwise leaves the expensive
fields empty or does not build them at all. The trace itself (`ctx.steps()`) stays
untouched — the rule "the trace is always there, the mode only decides what happens
to it" (ARCHITECTURE.md §2, B.1 rule 3) still applies to *steps*, not to the
*formatting* derived from them.

**Trade-off check.** No functionality is lost: in `ring`/`full` everything is still
built, in `aggregate`/`none` nothing is built that would never be read there anyway.
The only risk point is that `QueryEvent` is today a single struct that `record`
consumes completely — the rework makes individual fields optional or moves their
creation. Hence M, not S. The privacy assurance
(`no_query_name_leaves_the_process_in_the_quiet_modes`) has to stay green after the
rework; it does not change, because nothing changes about the *content*.

### 2. `pool::order` takes a mutex per upstream per request

**Current state.** [pool.rs:361–367](crates/alpendns/src/upstream/pool.rs#L361-L367)
reads `is_down` for **every** upstream on **every** cache miss from
`down_until: Mutex<Option<Instant>>` ([pool.rs:47](crates/alpendns/src/upstream/pool.rs#L47)).
The value is written only when an upstream fails for the third time in a row or
recovers ([pool.rs:216–279](crates/alpendns/src/upstream/pool.rs#L216-L279)) — so
practically never, while it is read constantly. That is exactly the pattern B.3
rule 5 (`ArcSwap`/atomic instead of a mutex in the request path) was made for.

**Approach.** `down_until` as an `AtomicU64`, encoded as "monotonic microseconds
since a process start time", `0` meaning "not down". Writes and reads become
`Ordering::Relaxed` accesses. The comparison `now < until` stays the same, just
without a lock.

**Trade-off check.** No behaviour changes: the same threshold, the same cooldown
duration. The only price is a small encoding function for the time. An `Instant` has
no portable epoch, hence the detour via `Instant::now().elapsed()` — the start time
is recorded once in the `Health`. Low risk, clear gain: a relaxed load replaces a
mutex lock (which under contention can reach all the way to a syscall).

### 3. `asked` is built on every request, but only the rebinding check needs it

**Current state.** [policy/mod.rs:620–621](crates/alpendns/src/policy/mod.rs#L620-L621)
builds `asked` — `query.name().to_ascii().trim_end_matches('.').to_lowercase()` — on
every request. It is needed only further down in the rebinding post-check
([policy/mod.rs:636](crates/alpendns/src/policy/mod.rs#L636)). On the typical
cache-hit path this string is created and never used.

**Approach.** Build `asked` only when `self.engine` holds an answer detector at all
(that is, the rebinding protection is not `off`). That is known at construction and
can be queried as a `bool`/method call without touching the request path.

**Trade-off check.** Trivial and without a behaviour change; the one extra string
allocation per request goes away on the hit path. Effort S.

### 4. DGA detection allocates on the heap per label

**Current state.** [dga/mod.rs:144–175](crates/alpendns/src/detect/dga/mod.rs#L144-L175):
`symbols()` creates a `Vec<usize>`, `mean_surprise()` calls that and iterates. Per RFC
a DNS label is at most 63 characters, so with the three boundary markers at most 66
symbols — a ceiling that is built into the process rather than arising at runtime.

**Approach.** Fixed stack buffers (`[usize; 66]` or similar) instead of `Vec`,
analogous to the `SmallVec` idea from the trace. Two heap allocations per label go
away; DGA detection runs on every allow request, that is, on the hot path.

**Trade-off check.** No behavioural difference, just an allocation replacement. The
`MIN_LENGTH` bound and the thresholds stay untouched — all that changes is where the
symbol list lives. Effort S. The only care needed: document the buffer size as a
constant next to `ALPHABET`, so that nobody "optimises" the 66 without counting the
boundary markers.

### 5. `registrable_domain` builds strings that are only hashed

**Current state.** [strategy.rs:23–30](crates/alpendns/src/upstream/strategy.rs#L23-L30):
`to_ascii()` (allocation), `trim_end_matches('.').to_ascii_lowercase()` (second
allocation), `psl::domain_str(...)`, and at the end
`.map_or(text.clone(), ToOwned::to_owned)` — a third allocation that in `zone_index`
exists only to own the `&str` so that it can be hashed. Hashing the `&str` directly
is enough, as long as `text` lives.

**Approach.** Drop the last `to_owned()` and hash the `&str` borrowed from
`psl::domain_str`. `text` stays alive until the end of the function; the hash lookup
needs no owner of its own.

**Trade-off check.** Pure allocation avoidance on the miss path, no behavioural
difference — the hash of the same `&str` is identical. Effort S. A note in the
interest of honesty: the gain only hits cache misses; in household operation with a
high hit rate it is small.

### 6. Typosquat rebuilds the embed string per entry per request

**Current state.** [typosquat.rs:179–184](crates/alpendns/src/detect/typosquat.rs#L179-L184)
assembles `format!(".{protected}")` three times in `embeds` — per protected domain,
per request. The protected domains are known when the detector is built and never
change.

**Approach.** Attach the leading dot string to the `Protected` once at construction
([typosquat.rs:67–83](crates/alpendns/src/detect/typosquat.rs#L67-L83)) and only
compare in `embeds`.

**Trade-off check.** Only relevant when `detection.typosquat.protect` is not empty;
then three small allocations per protected domain per request go away. Effort S, no
behaviour changed.

---

## Rejected approaches

These were noted while walking the code and **deliberately not** taken up as
candidates. Each with its reason, briefly, so that the decision stays traceable and
does not come to mean "we overlooked it".

### Detectors cost ~10 % throughput — left as they are

**Measured** (BENCHMARKS.md "Phase 8"): four detectors cost around 10 %, worst run
15 %. That is a price, but an affordable one: 87,000 requests/s is three orders of
magnitude above what a household resolver needs. The decision to bear the price is
already in [ADR-0019](adr/0019-heuristiken-melden-statt-blocken.md) and in the
BENCHMARKS rationale — here it is only made visible once more, not renegotiated.

### Tunneling mutex: no rework before the reversal condition

The global `Mutex<HashMap>` in
[tunneling.rs](crates/alpendns/src/detect/tunneling.rs) is the loudest single
detector item. The reversal condition is already documented (BENCHMARKS.md
"Phase 8"): if throughput ever falls below two thirds — more detectors, weaker
hardware — the zone table moves behind an `ArcSwap` or a split by hash. **Before
that it would be optimisation without measurement.** Candidate 2 (the pool mutex) is
therefore the more honest first step: there the mutex is a *pure read* mutex without
content, here it protects real state.

### A cache hit clones the answer completely — that is the price of the hit

`with_ttl` ([cache.rs:291–305](crates/alpendns/src/cache.rs#L291-L305)) clones the
entire message in order to set the remaining TTL. **Measured:** p50 18.8 µs, p99
28.1 µs — with a two-million-entry matcher loaded. The answer has to come out per hit
with the matching remaining TTL, and the cached copy must not be mutated; a clone is
the direct way there. The alternative (hold answers in a more compact form and
rebuild them cheaply) would be an architectural rework for a number that vanishes
against the DoT round-trip time to the internet anyway. **Rejected:** no measurable
benefit expected, high effort.

### `cache::shard` hashes with SipHash — below the noise floor

[shard()](crates/alpendns/src/cache.rs#L150-L155) computes a SipHash over the whole
key per lookup, only to derive 4 bits. SipHash is more expensive than a cheap hash —
but measured against a cache hit of 18.8 µs that is nanoseconds. A faster hash would
also need a new dependency (`ahash`/`fxhash`/…), which B.2 demands a rationale for.
**Rejected:** below the noise floor, and a dependency for under 1 % of a hit is the
wrong trade. Reconsider only once a profiler makes the shard selection visible.

### UDP buffer pooling — complexity without benefit

The `to_vec()` per packet in [udp.rs](crates/alpendns/src/server/udp.rs) allocates a
fresh buffer per incoming packet. That is how tokio UDP works: every `recv` needs a
buffer, and the answer needs its bytes. A pool on top of it saves one allocation and
buys it with lifetime bookkeeping and the risk that a pooled buffer lives on
somewhere. **Rejected:** the effort is out of all proportion to the gain, as long as
no profile shows the UDP allocation as hot.

### Duplicate `Message` clones in the cache and transport layer

`request.clone()` in [caching.rs:61](crates/alpendns/src/caching.rs#L61) and the
clones in [transport.rs](crates/alpendns/src/upstream/transport.rs) are cheap: a
request carries only its question, no answers. **Rejected:** not the place where time
is lost; a rework here would risk lifetime errors for nothing.

---

## What has to happen next

The pending candidates are to be implemented in isolation — one `perf/<topic>` branch
each, never on `main` — and measured against the baseline. Only an approach with a
proven benefit comes back. The order of measurement should follow the "Effort"
column, not the assumed gain: the three S candidates (2, 3, 4) are an afternoon each
and quickly deliver a decision you can stand on.

This analysis changes nothing on the `main` branch and nothing in the running
field-test week — it is the basis for deciding *after* acceptance, with measurements,
which of these is worth it.
