# ADR-0008: The `HashMap` stays — Bloom filter and inverted trie are not coming

**Status:** accepted · **Date:** 2026-08-29

## Context

[ARCHITECTURE.md §3](../ARCHITECTURE.md) describes as the target model for the
blocklists: reverse and intern labels, put a Bloom filter in front, behind it an
exact structure that is only queried on a Bloom hit. The roadmap, by contrast,
explicitly prescribes for Phase 4: "v1 as a HashSet with suffix lookup",
and the pitfall for it reads "Measure first, then optimise. Bloom filter and
inverted trie only come if step 11 shows they are needed."

Step 11 is measured. With two million entries
(`cargo test --release --test load -- --ignored`, numbers in
[BENCHMARKS.md](../BENCHMARKS.md)):

| | |
|---|---|
| Lookup, hit | p50 230 ns, p99 620 ns |
| Lookup, **no** hit | p50 390 ns, p99 880 ns |
| Memory of the matcher | 135 MB, around 69 bytes per entry |
| Build from parsed entries | 0.8 s |
| p99 of a request answered from the cache, with two million entries loaded | 28 µs |

The expensive case — a name that is on no list and therefore walks through every
suffix level — costs under a microsecond. The phase's acceptance criterion demands
a p99 under one millisecond for a request answered from the cache; measured are 28
microseconds, so thirty-five times the headroom.

## Decision

The `HashMap` with suffix lookup stays. Bloom filter, label reversal and inverted
trie are **not** built.

ARCHITECTURE.md §3 is not deleted; it points to this ADR: the target model stays
documented as a considered plan, together with the measurement that makes it
superfluous for now.

## Consequences

* Around 135 MB for two million entries. On a Raspberry Pi with 1 GB that is a
  lot, but bearable; at four million entries it would no longer be.
  **That is the number that flips this decision** — not the latency.
* The Bloom filter would above all have cost memory, not saved it: it comes
  *in addition to* the exact structure and only pays off once that structure grows
  so large it no longer fits in cache and every access is a memory miss.
  At a p99 of 880 ns that point is visibly not reached.
* The code is around 150 lines instead of several hundred, and the lookup is
  explainable in one paragraph. For a data structure on the hot path that is not a
  side note.
* The measurement is reproducible in the repo (`tests/load.rs`). Anyone who wants
  to flip the decision has the baseline numbers.

## Alternatives

* **Build the Bloom filter and trie now.** More code, more memory, and no number
  that justifies the effort. Exactly the case the pitfall in the roadmap warns
  about.
* **Intern domains to save memory.** More obvious than the Bloom filter when
  memory becomes the problem: shared suffixes like `.example.com` currently sit in
  memory a hundred times over. That would be the first step if 135 MB becomes too
  much — not the Bloom filter.
