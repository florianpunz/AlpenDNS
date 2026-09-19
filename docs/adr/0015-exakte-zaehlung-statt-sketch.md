# ADR-0015: Exact counting instead of a count-min sketch

**Status:** accepted · **Date:** 2026-08-30 · **Addendum to:** [ADR-0004](0004-logging-default-aggregiert.md)

## Context

In `aggregate` mode AlpenDNS counts how often a domain was queried and only
outputs it from `aggregate_k` hits on. That was implemented with a count-min
sketch: 2^18 counters × 4 rows × 4 bytes, a fixed 4 MiB.

A sketch **overestimates** — the value read is never smaller than the true one,
because foreign names fall on the same counters. The threshold therefore
correctly tested against the lower estimate bound, `estimate − error bound`, with
the error bound `e · queries / width`. FEATURES.md P1 had explicitly named that
bound.

The catch: **the error bound grows with the number of queries, not with the
number of names.** A household resolver has few names and many queries — exactly
the combination the sketch is bad at. Measured (BENCHMARKS.md, phase 7,
`cargo test --release --test load -- --ignored`):

| `k = 5`, every name queried five times | sketch | exact |
|---|---:|---:|
| 250,000 queries, 50,000 names · error bound | 2 | 0 |
| … of those above the threshold | **45** | **50,000** |
| 1,000,000 queries, 200,000 names · error bound | 11 | 0 |
| … of those above the threshold | **0** | **200,000** |

At a million queries — one day in a well-equipped household — the top-domain
output is **empty**, no matter what was asked. The mode that is the default and
that ADR-0004 describes as the compromise between benefit and restraint then
delivers only the restraint.

That is not an implementation error. The lower bound is exactly right; the sketch
fails on the safe side, as the module documentation described it. It is the
choice of structure: a count-min sketch is built for data streams whose
cardinality does not fit in memory. On a household resolver it does fit — the
same reasoning that made [ADR-0008](0008-hashmap-statt-bloom-und-trie.md) not
build the Bloom filter.

## Decision

The sketch is replaced by a `HashMap` with **exact** counters
(`logging::counts::Counts`). That drops the error bound, the lower estimate bound
and the explanation of it.

Two properties of the sketch had to be preserved in the process — they were the
actual reason for it, not the memory:

**First: no name below the threshold in memory.** The domains queried exactly
once are the telling ones (FEATURES.md P1); a `HashMap<String, u32>` would keep
every one of them until restart and cash in the project's central promise. The
key is therefore a `u64` hash of the name with a salt drawn at random at startup
(`RandomState`), not the name. No name can be read off the table; the plaintext
lands in `reportable` only once the threshold is reached — as before.

A collision in the 64-bit space would add two names together and thus
overestimate, that is, make the same error the sketch made. With 200,000 entries
its probability, by the birthday problem, is around 10⁻⁹, against an error bound
of 11 for the sketch. The difference is not a degree but an order of magnitude at
which one stops calculating.

**Second: an upper bound on memory.** A sketch has a fixed size; a `HashMap`
grows with the number of distinct names, and that number is controllable from the
outside — DNS tunneling produces random subdomains, but so does a browser with
NXDOMAIN probes. The table is therefore capped at `MAX_TRACKED = 200_000` (around
6.5 MB, the same order of magnitude as the sketch's 4 MiB). Once it is full, new
names get **no** counter and thus never reach the threshold: fail closed, as
B.1 rule 6 demands for policy decisions.

`MAX_TRACKED` is a constant, not a configuration key. Whoever reaches it does not
have a settings problem.

## Consequences

* The statistics are right again. A name queried five times shows up at `k = 5`,
  no matter how much other traffic runs.
* Counting costs around 45 ns instead of 100 ns, lookup 35 ns instead of 90 ns
  (BENCHMARKS.md). Both are meaningless next to 28 µs for a query from the cache;
  worth mentioning only because the probabilistic structure was not faster here
  either.
* Memory now hangs off the traffic instead of being fixed: 1.4 MB at 50,000
  names, 6.5 MB at full build-out. For a Raspberry Pi with 1 GB that is
  bearable. **That is the number that would overturn this decision** — not the
  latency.
* The test `a_rare_name_stays_hidden_among_many_others` remains, although with
  exact counting it no longer *can* fail. It checks the guarantee, not the
  implementation, and would be sharp again immediately on a rollback.
* `Counts::dropped()` counts queries that got no counter any more. The test
  `the_table_stops_growing_and_says_so` uses it to check that the cap bites. As a
  metric toward the outside the value does not go yet — as long as it does not, a
  full cap in operation cannot be told apart from "little is being asked right
  now". Noted as a candidate, not part of this decision.

## Alternatives

* **`HashMap<String, u32>` with the name as key.** Obvious and evidently exact —
  and it stores every domain asked once. That is exactly what `aggregate` exists
  against. Not negotiable.
* **Keep the sketch and make it wider.** The error bound is
  `e · queries / width`; to stay below 1 at a million queries it would take 2^22
  counters per row — 64 MiB, ten times the exact table, for a structure that
  still only estimates.
* **Reset the counters regularly** to keep the error bound small. Shifts the
  problem onto a window whose length would again be a configuration key — and a
  domain queried `k` times spread over two windows then disappears from the
  statistics.
* **No cap on the table.** More convenient and a memory leak that every device in
  the LAN can trigger.
