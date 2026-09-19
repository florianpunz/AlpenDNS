# Roadmap

The plan is cut into phases. Every phase has a **Goal**, a **checklist in verify
format** (see CLAUDE.md, part A.4) and an **acceptance criterion**. A phase counts
as done when the acceptance criterion is met — not when the code compiles.

**Current phase: 9** — the code is in place, acceptance is running on the real
network: the first observation period has been evaluated, the second has been
running since 2026-09-16.

The order is chosen so that **after phase 4 there is a server you can use
productively on your own network**. Everything after that makes it better, not
usable in the first place. If motivation flags along the way: phase 4 is a good
place to stop.

Time estimates are rough guesses for evening work with agent support. They are
orientation, not a promise.

---

## Phase 0 — Scaffolding · ~1 evening

**Goal:** A repository in which `cargo test` runs and CI is green.

```
1. Create the workspace (Cargo.toml, crates/alpendns) → verify: cargo build runs through
2. Pull the rustfmt/clippy configuration out of the workspace → verify: cargo clippy --all-targets -- -D warnings is green
3. Initialize the git repo, first commit → verify: git log shows one commit, git status is clean
4. Check the CI workflow → verify: push to GitHub, the Actions run is green
5. Set up cargo-deny (deny.toml) → verify: cargo deny check runs without errors
```

**Acceptance:** All four commands from the Definition of Done run through locally
and in CI.

---

## Phase 1 — A server that answers · ~2–3 evenings

**Goal:** UDP and TCP on port 53, the query is passed on to *one* fixed configured
upstream, the answer goes back. No cache, no filter, no policy.

That is the "Hello World" of a DNS server. From here you can run
`dig @127.0.0.1 -p 5353 example.com` against your own code, and that is the point
at which the project becomes real.

```
1. tokio runtime + UDP socket on a configurable port → verify: integration test sends bytes, server receives them
2. Parse the message with hickory-proto, extract the query → verify: unit test with a recorded query packet
3. Query the upstream over UDP, send the answer back → verify: dig @127.0.0.1 -p 5353 example.com returns an A-record answer
4. Validate the answer against the question (ID, QNAME case-insensitive, QTYPE, QCLASS) → verify: test with a manipulated answer → discarded
5. TCP listener incl. 2-byte length prefix → verify: dig +tcp returns the same result
6. Truncation: an answer > udp_payload_size sets the TC flag → verify: test with a large TXT answer, dig without +tcp shows TC, with +tcp the full answer
7. Load the config from TOML, deny_unknown_fields → verify: test with a typo in the key → startup aborts with a clear message
8. Graceful shutdown on SIGTERM/SIGINT → verify: a running query is still answered, then exit 0
```

**Acceptance:** `dig` over UDP and TCP returns correct answers. A fuzz target on
the request path runs for 5 minutes without a crash.

**Done on 2026-08-29.** With one deviation: the fuzz run is crash-free in the
delivery configuration (`cargo +nightly fuzz run -O`). With overflow checks
active it finds a panic in `hickory-proto 0.26.1` itself, which we cannot repair.
Assessment, conditions and the way back:
[ADR-0005](adr/0005-tsig-panic-in-hickory-proto.md). The CI run that was still
open at the time from phase 0 now runs permanently green in GitHub Actions.

**Pitfalls:** Port 53 needs privileges — use 5353 during development. UDP has no
connection: the answer must go back to exactly the source address the query came
from, and over the same socket.

---

## Phase 2 — Cache · ~2 evenings

**Goal:** Repeated queries come from memory. The upstream no longer sees them.

```
1. Cache key (name lowercase, QType, QClass), value = answer + expiry time → verify: unit test insert/lookup/expiry
2. Clamp TTL to [min_ttl, max_ttl], negative answers separately (RFC 2308) → verify: property test, TTL never larger than at insert time
3. Hang the cache into the pipeline → verify: integration test, the second query produces no upstream request
4. Size limit with LRU eviction → verify: test fills beyond max_entries, RSS stays stable
5. Query deduplication for simultaneous identical queries → verify: 100 parallel queries → fake upstream counts exactly 1
6. serve-stale (RFC 8767) → verify: switch the upstream off, the expired entry is still served
7. Prefetch at 85% of the TTL → verify: test with time-lapse, the entry is refreshed without a client waiting
```

**Acceptance:** The cache hit rate is visible as a metric. With a repeated corpus
`dnsperf` shows a clearly higher rate than in phase 1.

**Accepted on 2026-08-29.**

* **Hit rate visible:** `Cache::stats()` counts hits, stale hits and misses. The
  server writes the tally to the log every five minutes — but only if something
  has happened since the last line, so that an idle server stays quiet — and
  additionally on shutdown. Only totals, no names. A *queryable endpoint* for
  that is phase 6, step 2, and was deliberately not brought forward (CLAUDE.md
  B.8).
* **Throughput:** `dnsperf` is not installed on the development machine. Its
  place is taken by a load generator in the repo
  (`crates/alpendns/tests/load.rs`, only runs with `--ignored`), so that the
  numbers are reproducible instead of one-off. Results and measurement setup are
  in [BENCHMARKS.md](BENCHMARKS.md): 32,000 queries reach the upstream with
  all-new names, exactly **one** with a repeated corpus; throughput factor 3.3.
* **Addendum on step 4:** "RSS stays stable" was initially not checked, only the
  number of entries. Now measured: 160,000 new names at `max_entries = 10,000`
  grow memory by 764 KiB, not linearly with it.

**Pitfalls:** Time must be injectable (the `Clock` trait), otherwise all TTL tests
are `sleep`-based and slow. Do not call `SystemTime::now()` directly in the cache.

---

## Phase 3 — Encrypted upstreams · ~2–3 evenings

**Goal:** No more cleartext DNS to the outside. Several upstreams with a selection
strategy.

```
1. DoT transport (hickory-resolver, rustls) → verify: tcpdump shows port 853 TLS, no cleartext DNS
2. DoH transport (HTTP/2) → verify: integration test against a local DoH fake
3. Upstream pool with several resolvers, strategy fastest → verify: test with two fakes of different latency, the faster one wins
4. Passive health tracking, a dead upstream is skipped → verify: fake stops answering, queries go to the second, the metric shows the outage
5. Strategy split_by_zone with a seed at startup → verify: unit test, the same domain always the same upstream; different seed → different distribution
6. forward_zone for internal zones (cleartext allowed) → verify: a query for home.arpa goes to the LAN server, everything else encrypted
7. Privacy basics: strip ECS, EDNS padding, DNS cookies, 0x20 → verify: one unit test each; the 0x20 test checks round-trip and case-insensitive comparison
8. DoQ transport → verify: integration test against a local DoQ fake
```

**Acceptance:** `tcpdump port 53` on the uplink shows not a single cleartext DNS
packet anymore. If an upstream fails, no client notices.

**Accepted on 2026-08-29.** That closes the last open deviation from B.1 rule 7:
`udp://` in an `upstream_pool` is now a startup error, no longer the normal case.

* **No cleartext to the outside:** `tcpdump` needs root and was not available.
  Checked via `ss` instead, which connections the running process has:
  `9.9.9.9:853` (DoT) and `194.242.2.4:443` (DoH), not a single one on port 53.
  Plus an integration test that sets up a cleartext resolver as a trap and checks
  that it is never contacted.
* **An outage stays unnoticed:** unit tests in the pool cover this — a dead
  upstream is skipped after three failed attempts, recovers after the block, and
  if all count as dead, queries are still made (B.1 rule 6).
* **A pitfall that was expensive:** `hickory-net` rewrites the query ID on a
  multiplexed connection and does not give it back in the answer. The fakes only
  checked answer content and RCODE and were therefore green, while `dig` against
  the real process reported "ID mismatch" and ran into the timeout. The test
  `every_transport_returns_the_clients_query_id_and_question` now pins this down.
  Lesson: a fake that only checks what you expect checks too little.
* **Left open:** `split_by_zone` derives the registrable domain from the last two
  labels. For `example.co.uk` that is too coarse — the consequence is an uneven
  distribution, not a privacy hole. The clean solution needs the Public Suffix
  List and is planned as phase 7, step 1, where the distribution is measured
  anyway.
* **Subsequently removed (phase 7):** step 3 built `fastest`, to which
  `round_robin` and `fanout` were added. All three are gone again —
  `split_by_zone` is the only strategy, and exactly one upstream is always
  queried. [ADR-0011](adr/0011-eine-upstream-strategie.md),
  [ADR-0012](adr/0012-fanout-entfaellt.md).

**Pitfalls:** Not all upstreams tolerate 0x20 — make it switchable per pool and
deactivate it automatically on failure, instead of letting queries fail.

---

## Phase 4 — Blocklists · ~3–4 evenings · **this is where the server becomes usable**

**Goal:** Import lists, match, block, keep them current. From here AlpenDNS
replaces a Pi-hole on your own network.

```
1. Parser for the hosts format → verify: unit tests incl. comments, CRLF, IPv6 lines, garbage lines
2. Parser for domains and wildcard → verify: unit tests, leading dots and *. are normalized correctly
3. ~~Parser for Adblock syntax~~ → removed again in phase 7, [ADR-0014](adr/0014-adblock-und-rpz-parser-entfallen.md)
4. ~~Parser for RPZ zone files~~ → removed again in phase 7, [ADR-0014](adr/0014-adblock-und-rpz-parser-entfallen.md)
5. Matcher, v1 as a HashSet with suffix lookup, returns a RuleRef → verify: property test wildcard semantics; notexample.com never matches because of example.com
6. Allowlist with precedence over blocklists → verify: integration test, a domain on both lists is let through
7. Synthesize the block answer (nxdomain / zero_ip / sinkhole) → verify: one test each, RCODE and answer content correct
8. List download with ETag/If-Modified-Since, cache on disk → verify: the second fetch against a local HTTP fake returns 304, no reprocessing
9. Atomic swap via ArcSwap, no outage during an update → verify: load test during an update, no error answers, no latency spike
10. A first start without a reachable list aborts, a later outage does not → verify: two tests for both cases
11. Benchmark the matcher, measure RSS at 2 million entries → verify: the numbers are in docs/BENCHMARKS.md
```

**Acceptance:** 2 million entries loaded, p99 latency for a cache hit under 1 ms,
RSS documented.

**Accepted on 2026-08-29.**

* **Numbers met and clearly:** two million entries loaded, p99 for a query from
  the cache **28 µs** instead of the required 1 ms. The matcher needs 135 MB,
  around 69 bytes per entry; lookup p99 880 ns in the expensive case (no match,
  all suffix levels). All in [BENCHMARKS.md](BENCHMARKS.md).
* **Pitfall answered:** the measurement justifies neither a Bloom filter nor an
  inverted trie. Decision and reversal condition in
  [ADR-0008](adr/0008-hashmap-statt-bloom-und-trie.md) — memory footprint is the
  number that would tip it, not latency.
* Checked against the real StevenBlack list: 79,747 entries, `doubleclick.net`
  returns NXDOMAIN, a second start reports `origin=NotModified` — the ETag works.

**Deferred:** continuous operation on the real network ("a week as the only
resolver in the LAN") originally stood here. It belongs to phase 9: before that
there is no systemd unit, and without one the server does not run on port 53 and
does not survive a restart. Running a resolver in the LAN out of a shell would not
be a practical test but a different piece of work.

**Pitfalls:** Measure first, then optimize. Bloom filter and inverted trie
(ARCHITECTURE.md §3) only come if step 11 shows they are necessary.

---

## Phase 5 — Clients and policies · ~3 evenings

**Goal:** No longer one rule for everyone. Different devices, different rules,
time windows.

```
1. Client identification via IP/subnet → verify: integration test, two source IPs get different verdicts
2. Policy model (lists, allowlists, actions) + evaluation order → verify: table-driven test over all combinations
3. Fill the decision trace completely → verify: test checks the sequence of steps for a blocklist match
4. Schedules with an injectable clock → verify: the same query at two simulated times, two results
5. Temporary exemptions with TTL via the API → verify: exemption for 60 s, query allowed; blocked again after expiry
6. Regex rules per policy, with length and runtime limits → verify: fuzz test, no catastrophic backtracking runtime
7. Policy simulation as a CLI: alpendns policy test <domain> --client <name> → verify: the output shows the verdict plus the complete chain of reasoning
```

**Acceptance:** One device on the network has a stricter policy than the rest,
including time windows, and `alpendns policy test` explains every decision without
looking into the log.

**Accepted on 2026-08-29.** Against the real StevenBlack list:

```
$ alpendns -c … policy test doubleclick.net
Verdikt:  GEBLOCKT
  1. no client entry matches, 'default' applies
  2. Policy 'default'
  3. Blocklist 'stevenblack-unified' line 7092: 'doubleclick.net'
  4. Answer synthesized locally, mode Nxdomain

$ alpendns -c … policy test www.spiele.example --client kids-tablet
Verdikt:  GEBLOCKT
  3. Regex rule from policy 'kids': /(?:^|\.)spiele\./
```

The same domain without `--client` passes through — the rule belongs to that one
policy only.

* **Structural:** `resolve` now gets a `Ctx` with client address and trace. It
  initially sat behind a mutex, because with `fanout > 1` several upstream tasks
  entered it at the same time; since `fanout` was dropped it is passed through
  exclusively again ([ADR-0009](adr/0009-decision-trace-mit-mutex.md) including
  the addendum, [ADR-0012](adr/0012-fanout-entfaellt.md)).
* **Step 5 caught up** with the API from phase 6. Against the running server:
  `doubleclick.net` returns NXDOMAIN, after `POST /api/allow` NOERROR, after
  expiry NXDOMAIN again.
* **On backtracking (step 6):** the regex engine works with finite automata.
  `(a+)+$` against 10,000 characters runs in under a millisecond instead of
  exponentially. That is a property of the engine, not a precaution.

---

## Phase 6 — Visibility: API, metrics, web UI · ~4–5 evenings

**Goal:** You see what the server does without logging in.

```
1. HTTP API (axum) with token auth: status, statistics, lists, policies, temporary exemptions → verify: integration tests per endpoint, 401 without a token
2. Prometheus endpoint → verify: promtool check metrics is satisfied; counters for queries, blocks, cache hit rate, upstream RTT, errors
3. Logging layer with the four modes from ADR-0004 → verify: test per mode; in none and aggregate no query name appears in the output
4. Ring buffer for the ring mode → verify: test, entries older than ring_seconds are gone, RAM does not grow
5. Live query stream via Server-Sent Events → verify: test client receives events; with log mode none only counters come
6. Web UI skeleton, served by the server itself, no external resources → verify: the page loads with the network connection disconnected; the browser's network tab shows no third-party requests
7. UI view "Why was this blocked?" based on the decision trace → verify: manual — a blocked domain shows list, rule, policy, timestamp
8. UI start page answers Is it running? / What was blocked? / Why? without a click → verify: screenshot review against the requirements in CLAUDE.md B.6
```

**Acceptance:** An outsider opens the UI and understands in 30 seconds what the
server is currently doing. The page works without internet access.

**Implemented on 2026-08-29; accepted on 2026-08-31.**

Checked against the running server:

```
/api/status without a token     → HTTP 401
/api/status with a token        → {"logging_mode":"ring","queries":2,"blocked":1, …}
/api/recent                     → names plus the complete chain of reasoning
POST /api/allow                 → the blocked domain is let through
GET  /                          → HTTP 200, the UI
/metrics without a token        → alpendns_queries_total 3, no names
```

Added on 2026-08-30 and likewise checked against the running server:

```
/api/top                        → {"threshold":5,"domains":[{"name":"ads.example.com","count":6,…}],
                                   "below_threshold_queries":1,"below_threshold_names":1}
/api/history                    → {"bucket_seconds":300,"upstreams":["quad9","mullvad"],"buckets":[…]}
                                   — only counters, no field for a name
/api/explain?domain=…           → the same chain as `alpendns policy test`
```

* **The most important test** is `no_query_name_leaves_the_process_in_the_quiet_modes`:
  it drives a query through and greps everything the process can output —
  counters, top domains, ring buffer, file — for the query name. In `none` and
  `aggregate` it must not appear anywhere. That is the automated proof of the
  project's central promise and now runs with every `cargo test`.
* **k-anonymity:** initially via a Count-Min sketch whose threshold was checked
  against the *lower* estimate bound. Measured and replaced in phase 7: the error
  bound grew so fast with traffic that at one million queries no domain appeared
  in the statistics at all. Now counted exactly, under a salted hash instead of
  under the name ([ADR-0015](adr/0015-exakte-zaehlung-statt-sketch.md)).
* **Decisions on the interface** (two listeners, token in the SSE URL, UI without
  a build step, Prometheus by hand): [ADR-0010](adr/0010-api-ui-und-metriken.md).

**Step 8 — accepted on 2026-08-31.** A human checked the rendered page against the
requirements in CLAUDE.md B.6: the three questions without a click, both color
schemes, the "calm" impression, the "Flagged" panel. What can be checked
automatically runs as a test anyway: no outward references, the three questions
present as headings, the semantic colors in both schemes and only in selectors
with meaning, centered container, 8-px spacing, four metric cards, sparkline as
inline SVG without a library, explained empty areas, tabular figures, no
`innerHTML`.

The design was revised on 2026-08-30: centered container (max. 1400 px), metrics as
cards with a sparkline of the last 60 seconds, upstreams as rows with a status dot
and latency, badges and latency thresholds in the log, real empty states. The
single accent tone was replaced by the four semantic colors from B.6, plus
`--brand` for the wordmark alone; data sources and endpoints stayed unchanged.

**As of 2026-09-13 — the second revision: glass as a surface.** The interface now
lies as a translucent pane over a background of four color fields (sky, alpenglow,
meadow, shadow). That changes exactly one rule from B.6: the background may carry
color without meaning — it repeats no state and lies below the text threshold.
Everything else stays: the semantic colors, the grayscale ramp for stacked areas,
the centered container, the 8-px spacing, the one kind of container (now as a
glass edge with an 18px radius). Added is a switch between light and dark in the
header; the mode sits as `data-theme` on the root element instead of in a media
query, so the button can override the system preference. Rationale and rejected
alternatives: [ADR-0022](adr/0022-glas-als-flaeche.md).

What the rule "color is exclusively semantic" used to protect as an intention is
now protected by a computation: `the_text_stays_readable_on_every_field` in
`crates/alpendns/src/api/ui.rs` checks every text color against every field in
both modes and fails below 4.5:1. On its first run the test found a real bug —
`--faint` came out at 3.2:1 on the bare sky. Since then every surface with text is
a pane, including the login page.

Data sources, endpoints and the structure of the page stayed unchanged; the work
took place exclusively in `web/` and in the UI tests.

**Pitfalls:** This is the phase in which an agent is most likely to slide into
generic dashboard design. CLAUDE.md B.6 exists for that; refer to it explicitly in
every UI task.

---

## Phase 7 — Privacy build-out · ~3 evenings

**Goal:** The mechanisms that distinguish AlpenDNS from "Pi-hole with DoH".
Details on each point in [FEATURES.md](FEATURES.md).

```
1. Harden split_by_zone: seed rotation, measure the distribution → verify: test over 10k domains, deviation per upstream under 5%
2. Privacy budget: count per upstream what share of queries went there → verify: the API delivers the distribution, the UI shows it
3. Own DNSSEC validation instead of believing the upstream's AD bit → verify: test vectors with a valid, an invalid and a missing signature
4. Oblivious DoH as a client (RFC 9230) → verify: integration test against a local ODoH proxy fake
5. Aggregated statistics with a k-anonymity threshold → verify: a domain with fewer than k hits appears in no API response
6. Ensure that no code path outputs query names in none/aggregate mode → verify: the test drives a session and greps the entire output for the test name
```

**Acceptance:** Point 6 is the most important — it is the automated proof of the
project's central promise and must run permanently in CI.

**As of 2026-08-30 — points 2 and 5 have arrived in the interface.** The UI was
built out along ADR-0004, guiding principle "show everything, remember nothing":

* The privacy strip in the header permanently states the mode, the k threshold and
  whether anything lands on disk. The last item comes from the running process
  (`QueryLog::writes_to_disk`), not from an assumption — in `full` mode it says
  "writes to disk".
* Two curves over 24 hours (queries against blocked, cache hit rate) from
  `crate::history`: 288 buckets of five minutes in RAM, fed from the counter
  snapshot every 30 seconds. No new persistence; a restart resets the series. The
  structure only accepts `Sample` and therefore has no field into which a name
  could ever fit.
* Point 2 visible: the split over time as a stacked area per upstream, plus the
  transport mix and the privacy counters (ECS removed, padding, 0x20, cookies).
  What is counted is the *effect* — `strip_ecs` only counts if an option was
  actually removed, otherwise the number would merely show that a switch is on.
* Point 5 complete: `/api/top` delivers names exclusively above the threshold and
  next to it the sum of what stays below (`below_threshold_queries`,
  `below_threshold_names`). Without that sum, a server with lots of rare traffic
  would look like one with no traffic.
* New panel "Block reasons" via `logging::BlockReason`, derived from the trace.
  Categories today: Blocklist, Regex rule, Schedule, unassigned. **The DGA,
  tunneling and rebinding heuristics are missing from it because they do not exist
  yet** — they are phase 8, points 2 to 4. The enumeration will then take them up
  without a rebuild of counters, API or UI.
* A click on a log line queries `/api/explain` and gets the same evaluation as
  `alpendns policy test`: both call `policy::explain`, so that command line and UI
  do not give different answers to the same question.
* All counters in German number format (`de-AT`).

Followed up the same day: the live stream was built to send one message per query.
In a load test that slowed the interface to a crawl. Now the server caps at 25
messages per second and reports the omitted ones as a number (`skipped`), so the
sparkline does not lie; the browser collects events and draws once per frame.
Numbers in
[BENCHMARKS.md](BENCHMARKS.md#phase-7--live-stream-under-load--measured-on-2026-08-30).

Plus two new tests that pin down the boundary:
`every_label_key_comes_from_a_closed_set` nails the Prometheus label keys to an
allowlist (previously only three spellings were forbidden, a fourth would have
slipped through), and `no_metric_label_carries_a_domain_or_client_name` drives
real traffic through all four log modes and greps the rendered metric for query
and client names.

**Implemented on 2026-08-30 — all six points.** The four commands of the
Definition of Done run through. What was added:

**Step 1 — `split_by_zone` hardened** ([ADR-0018](adr/0018-public-suffix-list-und-seed-rotation.md)).
The registrable domain now comes from the Public Suffix List (`psl`, compiled in,
no network fetch, no runtime file) instead of the approximation "last two labels" —
which delivered the ineffective `co.uk` for `shop.example.co.uk`. The seed is
redrawn every 24 hours by default (`[[upstream_pool]] seed_rotation`, `"0s"`
switches it off); before, it held until a restart, and the sentence from
FEATURES.md P2 "over time nobody learns a stable picture" was only true for
someone who also restarts. The acceptance criterion is a test and not a one-off
measurement: `ten_thousand_domains_stay_within_five_percent_per_upstream` drives
10,000 domains — a fifth of them under multi-part suffixes — across four pool
sizes and eight seeds, largest deviation per upstream **under 5%**. The metric
carries `alpendns_zone_seed_rotations_total`, so that "the assignment rotates" is
a number in operation and not a claim.

**Step 3 — own DNSSEC validation** ([ADR-0016](adr/0016-dnssec-validierung-im-forwarder.md)).
On by default; an answer whose zone declares itself signed and whose chain does not
close is discarded (SERVFAIL, RFC 4035). The three test vectors in `tests/dnssec.rs`
run with **real** signatures through the same check as production: valid → `Secure`
and let through, flipped bit → `Bogus` and discarded, missing signature in a signed
zone → `Bogus` and discarded. That closes the open point from THREAT-MODEL.md A3.

Three decisions that are justified in the ADR and look arbitrary from the outside:
summarization is pessimistic (one lazy record makes the answer lazy, the authority
section included); `Bogus` is terminal and does *not* count as an upstream failure
(otherwise a broken zone would mark the whole pool as dead after three queries);
the signatures are only passed on to clients that asked for them with DO.

**Step 4 — Oblivious DoH** ([ADR-0017](adr/0017-oblivious-doh.md)), off by default.
`tests/odoh.rs` builds the complete chain on loopback — a proxy that forwards
without being able to decrypt, and a target with a real key pair. The most
important test searches the bytes that went through the proxy for the labels of
the query name; they are not in there. Also checked: the target's key is fetched
exactly once, an answer altered by the proxy is discarded, a dead proxy yields an
error instead of a hang. DNSSEC also applies over ODoH — the transport is wrapped
as a `DnsHandle` so that the validating handle can be hung in front of it;
otherwise `dnssec = true` with ODoH switched on would silently do nothing.

**Deviations and prices that belong on the record:**

* **`time` is now a production dependency.** `hickory-proto/dnssec-ring` pulls it
  in, and with that the old justification of the advisory exception
  RUSTSEC-2026-0009 ("is not in the binary at all") no longer holds. The exception
  stays, with a narrower justification — the vulnerable path (RFC 2822 date
  parsing) is not entered; hickory uses only `OffsetDateTime` from `time` for
  RRSIG timestamps, which come as numbers off the wire. Fully in `deny.toml`. It
  falls away as soon as the MSRV rises to 1.88; against that stands the Debian
  packaging from phase 9 (Debian 13 ships `rustc 1.85`).
* **The ODoH key fetch goes directly to the target**, not via the proxy — the
  target sees the address once per process start, but no query. Via the proxy it
  would not work: it only accepts ODoH messages.
* **Three bugs only came out when running against real upstreams** and are fixed:
  a discarded `dnssec-failed.org` was not counted and was charged to the upstream
  as a failed attempt (hickory delivers this case as an error, not as a stamped
  message); `dig` got the signature chain without having asked for it (AD in the
  query is, per RFC 6840 §5.7, not a request for records, only DO is); and a
  `dig +dnssec` got zero signatures because a `dig` without had been there before —
  stripping happened below the cache, which holds one answer for everyone. In
  detail in [ADR-0016](adr/0016-dnssec-validierung-im-forwarder.md). Re-checked
  against the running server:

  ```
  dnssec-failed.org            → SERVFAIL, quad9 without a failed attempt
  cloudflare.com               → NOERROR, ad flag, no RRSIG in the answer
  cloudflare.com +dnssec       → the same cache line, RRSIG included
  gnu.org                      → NOERROR, no ad flag (unsigned zone)
  ```

  **Postscript from phase 8:** a fourth bug only came out in continuous operation.
  If the upstream itself answers with an empty SERVFAIL, hickory likewise reports
  `Bogus` for lack of NSEC records — and because `Bogus` is terminal, a single
  wobble at the upstream became a hard SERVFAIL for the client, with no fallback
  attempt. `wikipedia.org` came back once as SERVFAIL this way and as NOERROR on
  the next attempt. The distinction is now made on what the answer contains;
  details in the postscript to
  [ADR-0016](adr/0016-dnssec-validierung-im-forwarder.md). The number `bogus=1`
  above has thereby dropped to 0: Quad9 validates itself, we never see a lazy
  signature — the old 1 was the mislabel.
* **The DNSSEC vectors pin the test zone's key as a trust anchor,** instead of
  building a chain up to the real root. Nothing is shortcut in the arithmetic;
  that the validating handle really is interposed in the transport is pinned by
  its own test in `encrypted.rs`.
* **The transport fakes in `encrypted.rs` now run without validation.** They
  answer every question with the same A record, including one for DNSKEY — for a
  validating handle that is not an unsigned zone but a broken chain. What is
  checked there are the transports.
* **Done:** the CI run now runs permanently green in GitHub Actions (fmt, clippy,
  test, cargo-deny). That fulfills this phase's acceptance criterion, point 6
  *permanently in CI*; the test runs with every `cargo test` locally anyway.
* **Done on 2026-08-31:** a human's look at the rendered UI (phase 6, step 8).
  Added there were an entry in the privacy strip ("DNSSEC checked locally" or
  "trusted the upstream") and two counters in the privacy tile, among them the
  discarded answers — the only number in the series that raises a question in
  operation.

---

## Phase 8 — Heuristics without a cloud · ~4–5 evenings

**Goal:** Detection of patterns that no list knows. Everything local, everything
explainable, everything by default only `flag`.

**Groundwork is in place:** `logging::BlockReason` and the UI panel "Block reasons"
have existed since phase 7. A new detector needs one variant there plus its step
in the trace — counters, metric label and diagram come along automatically.

```
1. Framework: detector trait, score 0.0–1.0 + reason, action from the config → verify: a dummy detector runs through the pipeline and lands in the trace
2. Rebinding protection (private IPs for public names) → verify: test, the exception list works
3. Tunneling detection (entropy, label length, rate, TXT/NULL share per zone) → verify: the iodine/dnscat sample corpus is detected, top-100k corpus produces under 0.1% false positives
4. DGA detection via character n-grams, model in the binary → verify: detection rate per DGA family documented, false positive rate on the top 100k under 0.1%
5. Typosquat detection against the protect list (Damerau-Levenshtein + Unicode confusables) → verify: constructed variants of two protected domains are detected, the originals never
6. NRD awareness from a local file → verify: test file with dates, a domain under max_age is flagged
7. UI: flagged queries with score and reason, one click to block or allow → verify: manual
```

**Acceptance:** One week of operation on the real network with all detectors on
`flag`. The false-positive list is reviewed; only after that may a detector go to
`block`.

**Note:** This is the substantive bridge to your AlpenShield project — the pipeline
built there (CT logs, zone data, classifier) can supply the NRD and reputation
files that AlpenDNS reads in locally here. The interface between them is
deliberately a simple file, not an API call: the resolver must not depend on a
second service running.

**Implemented on 2026-08-30 — all seven points.** The four commands of the
Definition of Done run through. What is **not** done is acceptance: it requires a
week of operation on the real network, and no test can replace that (see below).

**Step 1 — framework** ([ADR-0019](adr/0019-heuristiken-melden-statt-blocken.md)).
`crate::detect` with two traits: `NameDetector` sees the question, `AnswerDetector`
the answer. Two and not one, because there are two places in the pipeline —
rebinding protection needs the answer (ARCHITECTURE.md §1, layer 5). Four levels
`off`/`log`/`flag`/`block`; the difference between `log` and `flag` is who notices
them. The acceptance criterion stands as a test:
`a_detector_runs_through_the_pipeline_and_lands_in_the_trace`.

**Step 2 — rebinding.** Private addresses for public names, including the trap
`::ffff:192.168.1.1` and the glue records in the additional section. The
`forward_zone` entries automatically go into the exception list: whoever routes a
zone into their own network has already said that private addresses from there are
fine — without that, the protection would be a trap on first start.

**Step 3 — tunneling.** What is scored is the *zone* over a time window, not the
individual query: five signals, weighted, with one-off subdomains as the heaviest.
Measured: ordinary traffic under one zone stays at 0.17, a `dnscat2`-like stream
sits at 0.81, an `iodine`-like one at 1.0. On the top 100k, **0.0000% false
positives**.

**Step 4 — DGA.** 3-gram model in the binary, 107 KiB. On the top 100k, **0.077%
false positives** against a promise of 0.1%. Detection rates per family:
alphanumeric 92.8%, necurs-like 40.6%, conficker-like 28.6%, pronounceable 0.5%,
dictionary-based 0.0%. The last two are the documented limit of the method and no
surprise (FEATURES.md D3).

**Step 5 — typosquat.** Damerau-Levenshtein plus Unicode confusables plus Punycode
resolution. Four kinds of match with their own score; the originals are never
reported, and that is the more important part of the criterion.

**Step 6 — NRD.** Local file, score falls linearly with age. A missing file is
**not** a startup error: the resolver does not depend on AlpenShield having run.

**Step 7 — UI.** New panel "Flagged" with detector, score and reason, plus one
button each to allow and to block. The block is the counterpart to the temporary
exemption from phase 5 and uses the same structure.

**Deviations and prices:**

* **The measurement corpus is not in the repo.** Two files under `corpus/`
  (gitignored): training is on ranks 100,001–600,000 of the Majestic Million,
  measurement on the top 100k. The separation is not cosmetic — on the first
  attempt both ran on the same list, and the false positive rate was better by a
  **factor of 500** (0.001% against 0.54%). Origin, license and generation are in
  `src/detect/dga/model.bin.md`.
* **The DGA families are rebuilt, not recorded**, as is the iodine/dnscat corpus.
  What counts is the alphabet and the length range; the real seed produced the
  same distribution. That is how it stands in `tests/detect_corpus.rs`.
* **Punycode and private suffixes are exempted from DGA detection.** On the first
  measurement run, four of the twenty most notable names were IDNs, and
  `d1a2b3.cloudfront.net` counted as a generated name — it is one, but the
  provider hands it out that way. Both are now named blind spots instead of
  systematic false alarms for entire language areas.
* **Pinyin abbreviations remain a false alarm.** `hnqxdzkj.com` is a grown name
  made of the initial letters of Chinese syllables and, for a model over Latin
  text, indistinguishable from chance. They make up the largest part of the
  remaining 0.077%.
* **A DNSSEC bug from phase 7 came out here** and is fixed — see the postscript
  above.
* **Done:** the CI run runs permanently green in GitHub Actions (phase 0).
* **Done on 2026-08-31:** a human's look at the rendered UI (phase 6, step 8).
  Added was the "Flagged" panel.

**Acceptance is running:** "one week of operation on the real network with all
detectors on `flag`" needs a week and a real network. The practical test began on
2026-08-30 — since then the server has been the only resolver in the homelab. It
is the same run as the practical test from phase 9 (OPERATIONS.md §6). Only after
reviewing the false-positive list may a detector go to `block`.

**First period evaluated on 2026-09-16** (30.08. to 15.09., 220,659 queries, 4,025
distinct names). That these numbers exist at all is thanks to `mode = "full"`:
OPERATIONS §6 recommended `ring` for the observation, and the ring buffer would
not have survived the week — `/api/flagged` reads it, not the log. §6 has been
updated accordingly.

* **Rebinding — 2,595 reports, all false alarms.** Without exception sinkholes and
  telemetry: 661× an Amazon device identifier, `a.gslb.aaplimg.com`,
  `settings-win.data.microsoft.com`, `unagi-eu.amazon.com` — 194 distinct names,
  no real attack in 16 days. Fixed in
  [ADR-0021](adr/0021-rebinding-nur-erreichbare-adressen.md); after that the
  detector reported nothing more.
* **DGA — 7 findings, no false alarm.** All four names carry a generation pattern;
  the most notable is `cdn.deepseek.com.436b7a4e.cdnhwcqwg14.com` — a real name as
  a label in front of a generated domain. At the lab rate of 0.077%, around three
  findings would have been expected across 4,025 names.
* **Tunneling — never triggered.** No false alarm, but also no proof: in operation
  the detector never showed that it triggers correctly. Its effect is only
  evidenced by the corpus (BENCHMARKS.md).
* **Typosquat and NRD — ran empty.** `protect = []` and no `nrd.txt` respectively.
  Both evaluated nothing in this period; for them, "no false alarms" is not a
  statement but a zero without a basis.

**Second period since 2026-09-16 16:24.** It runs with a filled `protect` list and
decides on typosquat, confirms rebinding and DGA. Tunneling needs a controlled
test for that instead of another passive run; NRD stays `off` without data from
AlpenShield and therefore unevaluated.

---

## Phase 9 — Operation and packaging · ~3 evenings

**Goal:** Installed and hardened on a fresh Debian VM in five minutes.

```
1. systemd unit with CAP_NET_BIND_SERVICE, without root → verify: ps shows an unprivileged user, port 53 is listening
2. Hardening directives → verify: systemd-analyze security alpendns shows a score under 3.0
3. alpendns check as ExecStartPre → verify: a broken config prevents startup, the old instance keeps running
4. .deb package with cargo-deb, config under /etc/alpendns → verify: installation on a fresh Debian VM, the service starts
5. Rate limiting per client IP → verify: load test with one IP above the limit is throttled, other IPs unaffected
6. A first installation sets safe defaults (listeners only on private addresses) → verify: after installation the server is not reachable from outside
7. Document the load test and the numbers → verify: docs/BENCHMARKS.md contains queries/s, p99, RSS
8. Operations doc: installation, upgrade, backup, troubleshooting → verify: someone else installs it afterwards without asking questions
```

**Acceptance:** Fresh VM, `apt install ./alpendns.deb`, a working hardened resolver
with no manual follow-up work.

**Plus the practical test that was moved here from phase 4:** the server runs a
week as the only resolver in the LAN, without anyone complaining. Only here is it
set up for that in the first place — on port 53, as a service, across restarts.
Whatever stands out in the process belongs documented as a false-alarm list or a
configuration change; "worked on my machine" is not an acceptance criterion.

**Implemented on 2026-08-30 — all eight points.** The four commands of the
Definition of Done run through, the package builds and has been checked as
unpackable. What is **outstanding** is the part of acceptance that needs a second
machine and several days (see below). Guide for that:
[OPERATIONS.md](OPERATIONS.md).

**Step 1 and 2 — unit and hardening.** `packaging/systemd/alpendns.service`.
Port 53 comes via `AmbientCapabilities=CAP_NET_BIND_SERVICE`; the process starts
directly as `alpendns` and was never root. `systemd-analyze security` says **1.5** —
under 3.0 was required. What remains is what a resolver naturally needs: network
access and port 53.

One deviation from CLAUDE.md B.5, and it has to be one: `RestrictAddressFamilies`
additionally carries `AF_NETLINK`. When resolving a hostname, glibc asks via
Netlink which address families the machine has; without that line the blocklist
download fails on some systems, and silently at that. Also checked and removed
again: `PrivateUsers=yes` — in its own user namespace, `CAP_NET_BIND_SERVICE` no
longer carries through to port 53.

**Step 3 — `alpendns check`.** Runs as `ExecStartPre`. Checks the configuration,
the blueprint (references between clients, policies and lists) and the three
directories that must be written to — the latter by writing to them, not by
computing permissions. Explicitly **nothing that needs network**: a startup script
that waits on the internet is a startup script that will hang at some point.

**Step 4 — .deb.** `cargo deb -p alpendns`, metadata in
`crates/alpendns/Cargo.toml`, maintainer scripts in `packaging/debian/`. The
package creates the system user; **systemd** creates the directories under `/var`
via `StateDirectory=` and its siblings — that way there is exactly one place that
sets permissions, and an upgrade cannot break it. `/etc/alpendns/alpendns.toml` is
a conffile.

**Step 5 — throttling.** Token bucket per client, 16 slots with one mutex each (no
global lock in the request path, B.3 rule 5), LRU-capped. Above the limit it is
**dropped, not rejected** — [ADR-0020](adr/0020-rate-limiting-verwirft.md). IPv6 is
grouped by /64, otherwise a laptop with privacy extensions would have a fresh
allowance every hour. On by default.

**Step 6 — safe defaults.** The shipped `packaging/alpendns.toml` listens on
loopback; freshly installed, the server is not reachable from outside. That stands
as a test (`the_packaged_configuration_listens_nowhere_public`), and `0.0.0.0`
counts as public in it — the wildcard also binds to an interface that will be on
the internet tomorrow. At the end, `alpendns check` says whether a listener
reaches beyond your own network.

**Step 7 — numbers.** 84,078 queries/s, p99 674 µs, RSS 39,732 KiB; throttling
costs **2% throughput**. Under a flood of 274,000 packets/s from one source, the
p99 of the other clients stays at 765 µs. [BENCHMARKS.md](BENCHMARKS.md), phase 9
section.

**Step 8 — operations doc.** [OPERATIONS.md](OPERATIONS.md): installation, release
for the LAN, upgrade, backup, troubleshooting, what the hardening directives mean,
observation week, uninstall. Ships in the package under `/usr/share/doc/alpendns/`.

**What is outstanding — and what is meanwhile evidenced:**

* **Installation on a real system — verified, with a deviation.** On 2026-08-30 the
  `.deb` was installed on an **Ubuntu container**: the service runs, `check` is
  green, since then all homelab DNS traffic goes through the server. That proves
  the sequence `adduser` → `deb-systemd-helper` → first start on a foreign system,
  which the run of the unpacked binary had left open before — and it refutes the
  worry that `SystemCallFilter` hampers the process in operation.

  A **deviation from the criterion** remains: what is required is a *fresh Debian
  VM*, what was delivered is an Ubuntu container. Ubuntu is Debian-related (the
  `.deb` targets Debian/Ubuntu anyway, `rustc 1.85` = Debian 13), and a container
  differs from a bare VM in systemd and capabilities behavior. Accepted for now; a
  fresh Debian VM will be caught up with at some point.
* **The practical test is running.** Begun on 2026-08-30: the server is the only
  resolver in the homelab, all detectors on `flag`. Guide in OPERATIONS.md §6; the
  run also covers the acceptance of phase 8 (observation week with all detectors
  on `flag`), because both are the same run. At the end stands the review of the
  false-positive list — only after that may a detector go to `block`.
* **Done:** the CI run runs permanently green in GitHub Actions (phase 0).
* **Done on 2026-08-31:** a human's look at the rendered UI (phase 6, step 8).

---

## Phase 10 — Optional, discardable at any time

No order, no obligation. As the mood and the need take you:

* **Recursion** via `hickory-recursor` behind the existing `ResolveBackend` trait
  ([ADR-0003](adr/0003-forwarder-first.md)), incl. QNAME minimisation.
* **DDR/DNR** (RFC 9462/9463): LAN clients find your encrypted endpoint
  automatically and switch from cleartext to DoH/DoT ([FEATURES.md](FEATURES.md), P5).
* **Blocklist diff review** before applying a list update (O3).
* **Two instances** with a synchronized policy state for fault tolerance.
* **Own DNS parser** as a pure learning project, checked via differential testing
  against `hickory-proto` — deliberately outside the production path.

---

## How to work with the agent

* **One phase = one work session**, no more. "Build me phases 4 through 8" reliably
  leads to 3000 lines that nobody reviews anymore.
* **Every step first as a test.** The verify columns above are already the test
  descriptions; pass them on verbatim.
* **After every step, the four commands** from the Definition of Done.
* **Let it stop on ambiguity.** CLAUDE.md B.8 lists when that applies — when in
  doubt, refer to it explicitly.
* **After every phase:** update the roadmap (current phase), note open points and
  deviations, write an ADR when a decision has been made.
