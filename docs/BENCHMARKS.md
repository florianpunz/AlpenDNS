# Benchmarks

Numbers, not claims. Every phase collects them anew (docs/TESTING.md §5); old values stay
in place so that changes remain visible.

All measurements run against a **fake upstream in the same process**. Tests and
measurements never contact real resolvers. That pushes the numbers in one direction that
you have to keep in mind while reading: the fake answers without network latency, so the
cache's advantage is **under**estimated here. Against a real upstream over DoT, the gap
between a cache hit and an upstream request is not a factor of 3 but the round-trip time
into the internet.

## Test machine

| | |
|---|---|
| CPU | AMD Ryzen 5 5600X, 6 cores / 12 threads |
| RAM | 31 GiB |
| Kernel | Linux 7.0.0-30-generic |
| Rust | 1.98.0, `release` profile (`lto = "thin"`, `codegen-units = 1`) |

The numbers depend on the machine. What is meaningful is the comparison of the rows with
each other, not the absolute value.

## Reproducing

```bash
cargo test --release --test load -- --ignored --nocapture --test-threads=1
```

`--test-threads=1` is mandatory — otherwise the measurements run at the same time and
compete for the same cores, which pushes throughput down by around a third.

`dnsperf` (docs/TESTING.md §5) is not installed on the development machine; for now the
load generator in `crates/alpendns/tests/load.rs` replaces it. Compared with `dnsperf` it
has one advantage and one disadvantage: it runs without installing a system package and
measures the same setup reproducibly — but it is not an established tool whose numbers
would be comparable with other projects.

---

## Phase 2 — Cache · measured on 2026-08-29

16 clients × 2,000 requests = 32,000 requests per run.

| Corpus | Requests/s | Upstream requests |
|---|---:|---:|
| every request a new name | 103,317 | 32,000 |
| always the same name | 341,761 | **1** |

Factor 3.3 in throughput. The more important column is the right one: with a repeated
corpus exactly **one** request reaches the upstream. That is not only a performance
statement but a privacy statement — the upstream sees 32,000 fewer requests.

### Memory under eviction

`max_entries = 10,000`, five rounds of 32,000 **new** names each (160,000 in total, that
is sixteen times the cache size):

| | RSS |
|---|---:|
| after 1 round | 13,420 KiB |
| after 5 rounds | 14,184 KiB |
| growth | 764 KiB |

Without working LRU eviction, memory would have to grow linearly here. It does not.

---

## Phase 3 — after 0x20, cookies and ECS stripping · measured on 2026-08-29

Same setup. What was interesting was what the privacy mechanisms cost on the plaintext
path: 0x20 re-rolls the spelling of the name for every request, cookies attach an EDNS
option.

| Corpus | Requests/s | before | Upstream requests |
|---|---:|---:|---:|
| every request a new name | 110,106 | 103,317 | 32,000 |
| always the same name | 347,726 | 341,761 | **1** |

Factor 3.2. The differences are within the noise of the measurement — the privacy layer
costs nothing measurable.

| | RSS |
|---|---:|
| after 1 round | 13,860 KiB |
| after 5 rounds | 14,680 KiB |
| growth | 820 KiB |

**Not measured:** the encrypted path. A DoT or DoQ handshake against a fake in the same
process mainly measures the crypto library, not AlpenDNS. The number that counts is the
latency to the real upstream anyway — in the smoke test Quad9 (DoT) was at 34 ms and
Mullvad (DoH) at 113 ms.

---

## Phase 4 — Blocklists · measured on 2026-08-29

Two million entries, as the acceptance criterion demands.

### Matcher

| | |
|---|---:|
| Parsing the list (hosts format) | 396 ms |
| Building the matcher | 795 ms |
| Lookup, hit | p50 230 ns · p99 620 ns |
| Lookup, no hit | p50 390 ns · p99 880 ns |

The more expensive case is the miss: it walks all suffix levels, while a hit usually stops
at the first one. That is exactly why it is measured separately — in operation it is the
normal case.

### Memory

| | RSS |
|---|---:|
| before | 3,588 KiB |
| with matcher | 326,460 KiB |
| after freeing the matcher | 191,292 KiB |
| **matcher itself** | **135,168 KiB — around 69 bytes per entry** |

The obvious number (322 MB of growth) would be wrong: it contains the list in raw text and
the parsed entries that existed additionally during construction, plus what the allocator
does not return to the system after freeing. The third row separates the two.

### Request from the cache with two million entries loaded

| | |
|---|---:|
| p50 | 18.8 µs |
| **p99** | **28.1 µs** |
| p999 | 37.6 µs |

The acceptance criterion demands under 1 ms. Consequence for the data structure:
[ADR-0008](adr/0008-hashmap-statt-bloom-und-trie.md).

### Throughput unchanged

| Corpus | Requests/s | Phase 3 | Upstream requests |
|---|---:|---:|---:|
| every request a new name | 104,838 | 110,106 | 32,000 |
| always the same name | 323,740 | 347,726 | **1** |

The filter sits before the cache and is therefore consulted on *every* request. That
throughput nevertheless stays within the noise of the previous measurement fits the 880 ns
per lookup.

---

## Phase 7 — Counting structure behind the k threshold · measured on 2026-08-30

The count-min sketch against an exact table. Both measured in the same run so that the
memory figures are comparable. `k = 5`, every name is asked five times — a name that
therefore hits the threshold **exactly**.

### 50,000 distinct names · 250,000 requests

| | Sketch | exact |
|---|---:|---:|
| Memory | 4,168 KiB | 2,180 KiB |
| per entry | 109 ns | 42 ns |
| per query | 99 ns | 34 ns |
| error bound | 2 | 0 |
| **names above the threshold** | **45** | **50,000** |

### 200,000 distinct names · 1,000,000 requests

| | Sketch | exact |
|---|---:|---:|
| Memory | 4,096 KiB | 4,356 KiB |
| per entry | 78 ns | 54 ns |
| per query | 74 ns | 40 ns |
| error bound | 11 | 0 |
| **names above the threshold** | **0** | **200,000** |

The last row is the number this is about. The sketch overestimates, so the threshold
checks against the lower estimate bound — and that is `estimate − error bound`. At 250,000
requests the bound stands at 2, so a name asked five times arrives with 3 and stays below
`k = 5`: of 50,000 names, 45 make it. At one million requests the bound stands at 11 and
the statistics are **empty**.

That is not a bug in the implementation — the lower bound is exactly right, and
FEATURES.md P1 had predicted this limit. It is the structure that is unsuitable for this
order of magnitude: it is built for data streams whose cardinality does not fit in memory.
For a household resolver it does fit.

Memory and time are the side result: with exact counting it is half the memory at 50,000
names and roughly twice as fast; at 200,000 names — the table's upper bound — it costs
about the same.

Measured on its own, without the sketch before it in the same process, the exact table
sits at **1,384 KiB** (50,000 names) and **6,532 KiB** (200,000 names). The difference from
the table above is allocator behaviour, not structure: 4 MiB of sketch had just been freed
there. The 6.5 MB at full size are the honest upper bound, and it is capped — the table
does not take in more than `MAX_TRACKED` names.

Consequence: [ADR-0015](adr/0015-exakte-zaehlung-statt-sketch.md).

---

## Phase 7 — Live stream under load · measured on 2026-08-30

**Occasion:** the web UI hung during a load test and showed an update only every few
seconds. The cause was not in the browser but in the interface: the server sent **one SSE
message per request**.

Measured against a running server on port 15353 with a local blocklist (all requests are
answered locally, no upstream involved). The load generator is a UDP flooder without
response evaluation, `dnsperf` is still not installed on this machine either. The binary
ran in the `dev` profile — the absolute throughput figures are therefore *not* a statement
about the resolver's performance, only the frame for the comparison below.

| | before (calculated) | after (measured) |
|---|---|---|
| Requests in 5 s | 484,294 | 484,294 |
| SSE messages | 484,294 | **151** |
| JSON frames per second | ~97,000 | 25 |
| DOM rows per second in the browser | ~97,000 | 25 |

Of the 151 messages, 5 carried a `skipped` field, together 428,471 skipped requests — one
at the start of each second. The pulse thus remains complete even though 99.97 % of the
messages are omitted.

**Does an open GUI cost the resolver anything?** Twice a 4 s flood, once without and once
with an open SSE connection:

| | Requests answered in 4 s |
|---|---|
| without an open GUI | 354,497 |
| with an open GUI | 387,734 |

The difference lies within the spread between two runs; a GUI reading along can no longer
be found in the noise. Before, it cost the server a formatted timestamp, several
allocations and a JSON serialisation per request.

**At normal pace nothing is omitted:** 12 requests at 250 ms intervals yielded 12 messages,
none of them with `skipped`. The limit only takes effect above 25 requests per second —
nobody can read along faster than that anyway.

The browser side is not measured separately (no browser is available here). Three things
were changed there, whose effect follows from the number above: events are collected and
drawn once per frame instead of individually, the table has one listener instead of two per
row, and an invisible page no longer draws and polls.

---

## Phase 8 — Heuristics · measured on 2026-08-30

**Measurement setup.** Two corpora from the Majestic Million (CC-BY 3.0), both under
`corpus/` and both not in the repo:

| File | Ranks | Purpose |
|---|---|---|
| `train-500k.txt` | 100,001 – 600,000 | training the DGA model |
| `top-100k.txt` | 1 – 100,000 | measurement |

Reproducible with `cargo test --release --test detect_corpus -- --ignored
measure --nocapture`; instructions for obtaining them are in the header of the same file
and in [TESTING.md](TESTING.md).

### Why two corpora, and what the first attempt cost

The first measurement run trained and measured on the **same** top-100k and reported
0.001 % false positives. On unseen names it was **0.54 %** — a factor of 500. A model
recognises the names it was built from. Since then training happens on the ranks behind
and measurement on the top-100k, which never entered the model a single time.

The number below is the second, not the first.

### DGA detection

Threshold 0.75. **77 of 100,000 names reported — 0.077 %**, against a promise of 0.1 %.

| Threshold | False positives | alphanumeric | necurs-like | conficker-like |
|---:|---:|---:|---:|---:|
| 0.70 | 0.098 % | 93.8 % | 47.0 % | 33.2 % |
| **0.75** | **0.077 %** | **92.8 %** | **40.6 %** | **28.6 %** |
| 0.80 | 0.039 % | 38.8 % | 31.9 % | 18.7 % |

0.75 sits immediately before the edge: at 0.80 the alphanumeric family collapses from
92.8 % to 38.8 %, while the false alarms only fall from 77 to 39. At 0.70 no reserve would
remain under the promise.

**Hit rate per family** (5,000 rebuilt names each, threshold 0.75):

| Family | Avg. surprise | Hit rate |
|---|---:|---:|
| alphanumeric | 7.92 bit | 92.8 % |
| necurs-like | 6.17 bit | 40.6 % |
| conficker-like | 6.01 bit | 28.6 % |
| kraken-like (pronounceable) | 4.79 bit | 0.5 % |
| suppobox-like (dictionary) | 3.45 bit | 0.0 % |

For comparison, grown names: median 3.69 bit, 99 % at 6.24, maximum 10.73.
**The distributions overlap** — a pronounceably generated name *is*, statistically, a
grown name. The last two rows are therefore not a gap in the implementation but the limit
of the method (FEATURES.md D3), and they stand as a test in
`dga::tests::a_word_list_dga_is_honestly_not_detected`.

**Two classes of systematic false alarm have disappeared in the process:**

* *Punycode.* Four of the twenty most conspicuous names in the first run were IDNs —
  `xn--vhqrb498dfmcffp24qfocl09dqkh.cn` looks like base32 to a Latin character model.
  They are no longer assessed; a named blind spot is better than a false alarm for entire
  language areas.
* *Private suffixes.* `d1a2b3c4e5.cloudfront.net` is a generated name — it is just that the
  provider hands them out that way, and `cloudfront.net` is itself in the Public Suffix
  List. Below a private suffix nothing is assessed any more.

What remains is for the most part **Pinyin abbreviations**: `hnqxdzkj.com`, `lzdsxxb.com`,
`pzhsdqfybjfwzx.cn`. Grown names from the initial letters of Chinese syllables,
indistinguishable from randomness for a model over Latin text.

### Tunnelling detection

Threshold 0.75. **0 of 100,000 names reported — 0.0000 %.**

The corpus is fed in here the way it would look in the worst case: all 100,000 names
within a window of five minutes. That is far more traffic than a household generates.

| Traffic | Score |
|---|---:|
| 20 hosts under one zone, each asked five times | 0.17 |
| `dnscat2`-like (hex, 36 characters, TXT) | 0.81 |
| `iodine`-like (base32, 58 characters, TXT) | 1.00 |

The threshold sits at the **lower** edge of the hits and not in the middle of the gap:
`dnscat2` encodes in hexadecimal and gets no further than 4 bits of entropy, so it lies
just above 0.8. With 0.8 as the threshold, the second decimal place would decide whether
the most widespread tunnel stands out.

### Typosquat guard

Protection list with five domains against the same 100,000 names: **18 reports,
0.018 %**. None of them is one of the protected domains itself — that is the part of the
criterion that counts.

### What the detectors cost the request path

The question is not rhetorical: tunnelling detection takes a `Mutex` over a table on
**every** request, and CLAUDE.md B.3 rule 5 says "no global mutex in the request path".

Measured with the load generator from phase 2, all new names — only then do the detectors
really run on every request (`cargo test --release --test load -- --ignored
throughput_with_and_without_detectors`). Three runs:

| Run | without detectors | with four detectors | Share |
|---|---:|---:|---:|
| 1 | 96,191 /s | 87,065 /s | 90.5 % |
| 2 | 100,707 /s | 85,689 /s | 85.1 % |
| 3 | 97,127 /s | 88,064 /s | 90.7 % |

**Around 10 % throughput**, 15 % in the worst run. That is not noise but a price — and it
is affordable: 87,000 requests per second is three orders of magnitude above what a
household resolver will ever need.

The mutex therefore stays for now. The reversal condition is the same as with
[ADR-0008](adr/0008-hashmap-statt-bloom-und-trie.md): if this measurement one day falls
below two thirds — for instance because more detectors are added or the resolver runs on
weaker hardware — the zone table belongs behind an `ArcSwap` or a split by hash. Before
that it would be optimisation without measurement, and phase 4 explicitly forbids that.

### Memory

| | |
|---|---|
| DGA model in the binary | 109,744 bytes |
| Tunnelling state, upper bound | 4,096 zones × at most 256 hashes per zone |

Both bounds are hard and stand as a test (`the_table_does_not_grow_with_traffic`,
`the_unique_set_per_zone_is_bounded`): the table lies in the request path and must not grow
with traffic.

---

## Phase 9 — Per-client rate limiting · measured on 2026-08-30

```bash
cargo test --release --test load -- --ignored --nocapture --test-threads=1 \
    throughput_with_and_without_rate_limiting a_flooding_client
```

The load here comes from **16 different sender addresses** (`127.0.0.1` to `127.0.0.16`)
and not, as in the earlier runs, from one. Anything else would make the measurement
pointless: from a single source the only question would be how fast a bucket empties.

### What rate limiting costs the request path

The limit is set so high here (1,000,000/s) that nothing is discarded — what is measured is
the way through the token bucket, not its effect.

| Request path | Requests/s | p50 | p99 |
|---|---:|---:|---:|
| without rate limiting | 85,788 /s | 145 µs | 663 µs |
| with rate limiting | 84,078 /s | 147 µs | 674 µs |

**2 % throughput**, p99 worse by 11 µs. That is the price for a mandatory piece
(CLAUDE.md B.5), and it is not one: rate limiting sits *before* parsing, a discarded packet
costs one hash and one comparison.

RSS of the test process over the whole run: 31,272 KiB → 39,732 KiB, with 16 observed
clients. The bookkeeping itself is capped (LRU, default 8,192 clients); the growth here is
the cache with 32,000 new names, not the limiter.

### One troublemaker next to normal clients

One client fires for ten seconds as fast as it can against a limit of 200/s (burst 400).
Alongside it the 16 measurement clients run their usual load.

| | |
|---|---:|
| The troublemaker sent | 2,746,620 requests |
| of which discarded | 2,742,924 (99.87 %) |
| The other clients | 3,804 requests/s, p50 157 µs, **p99 765 µs** |

The finding that matters is the **p99 of the other clients: 765 µs against 674 µs in the
undisturbed run**. Under a flood of 274,000 packets per second, response time for everyone
else stays practically unchanged — that is what "other IPs unaffected" is supposed to mean.

The throughput of the other clients drops from 84,000 to 3,800 requests/s, and that is
**not** a result about rate limiting: the troublemaker runs as a task in the same process
on the same cores and burns them with its own send loop. Over a real network the
competition would be different. The number stands here because it stands in the test output
— not as a statement about operation.

### Requests/s, p99, RSS at a glance

The three numbers that ROADMAP phase 9 step 7 demands, in the delivery state (cache on,
rate limiting on, no detectors, all new names — that is, the expensive case in which the
cache never helps):

| | |
|---|---:|
| Throughput | **84,078 requests/s** |
| p99 | **674 µs** |
| RSS at the end of the run | **39,732 KiB** |

---

## TCP connection setup under the new limits · measured on 2026-09-16

```bash
cargo test --release --test load -- --ignored --nocapture --test-threads=1 \
    tcp_connection_setup_with_the_limits
```

The UDP throughput above says nothing about this change: not a single connection is set up
there. The limits from TODOS no. 1 sit in the accept path, so this run measures
**connections per second with one request each**, from 16 sender addresses with 500
connections each. Every client comes from its own address — otherwise the per-address
allowance kicks in first and the measurement measures the wrong thing.

The comparison is against the state before the change. Because the old state does not know
the counters, the measurement there ran against a stand-in with the same signature
(`with_tcp_stats` without effect); the accept path itself was left untouched.

| | Connections/s |
|---|---:|
| before (median of 7 runs) | 31,068 |
| after (median of 7 runs) | 29,030 |
| Spread per side | 27,300 – 33,900 |

**The difference lies within the spread.** A single run says nothing here: the values of
the same variant fluctuate by up to 20 %, measured against each other in alternating
order. What remains is the statement the setup does support: one semaphore access, one hash
entry and one `Arc` per connection are not measurable against a TCP handshake. The limits
cost nothing in connection setup that this machine could resolve.

No wonder: they only take effect once it is already too late. In normal operation the price
is a branch that is never taken.
