# ADR-0019: Heuristics report, they do not block

**Status:** accepted · **Date:** 2026-08-30 · **Affects:** [FEATURES.md](../FEATURES.md) D1–D6, [ROADMAP.md](../ROADMAP.md) Phase 8

## Context

Phase 8 brings five detectors: DGA, tunneling, rebinding, typosquatting and
newly registered domains. Four of them are heuristics in the proper sense — they
guess, with reasons, but they guess.

The example configuration showed `tunneling` and `rebinding` on
`block` from the start. The roadmap says, for the same phase, "everything by
default only `flag`", and CLAUDE.md B.8 explicitly demands asking before a
detector sits on `block`. One side had to win.

## Decision

**All five default to `flag`.** They report, they do not block. The
example configuration was aligned, not the code.

The reason is in FEATURES.md, section D, and it is not caution for
caution's sake: *a detector that breaks the internet gets switched off — and
with it all the others.* Anyone who has once seen the bank fail to load switches
`[detection]` off as a whole and does not come back. The four heuristics are
together worth more than any single one, and the way there leads through an
observation week.

### Four levels and not two

`off` · `log` · `flag` · `block`. The difference between `log` and `flag` is
the only one that needs explaining: both count and appear in the trace, but
only `flag` puts the query on the list that someone looks at. Whoever
wants to get to know a heuristic first takes `log`; whoever wants to judge it,
`flag`.

### The thresholds live in the code, not in the configuration

Every detector brings its own `DEFAULT_THRESHOLD`. A threshold makes
sense only together with the arithmetic that produces the score — writing it into
an example file and maintaining it there would mean keeping two things in sync
that belong together. The configuration can override it; the default comes
from where it can be justified.

All thresholds are **measured and not guessed**. How, is in
[BENCHMARKS.md](../BENCHMARKS.md); with what, in
`crates/alpendns/tests/detect_corpus.rs`.

### Two traits, because there are two places in the pipeline

`NameDetector` sees the question, `AnswerDetector` the answer. The
rebinding protection checks whether a private address comes back for a public
name — for that, resolution must already have run (ARCHITECTURE.md §1,
layer 5). A shared trait with an `Option<&Message>` would
conceal that the two run at different points, and the first
mix-up would be a detector that always gets `None` and never finds anything.

### The heuristics come last

Order in `Engine::evaluate`: timed allow → allowlist → timed
block → blocklists → regex → schedule → **heuristics**.

They are the least precise instrument in the house, and everything that a clear
rule can decide should be decided beforehand. Otherwise the trace would hold a
score where a line from a list belongs. An allow beats every detector:
whoever has explicitly allowed a domain wants to reach it, no matter what a score
says about it.

### A detector without its data is not wired in at all

The typosquat guard needs a protection list, the NRD detector a file.
If they are missing, the detector appears in the status as `off` instead of as
switched on. A detector that looks switched on and can find nothing is an
assurance that does not come true (B.1 rule 5).

### `forward_zone` is automatically exempt from rebinding protection

Whoever routes a zone into their own network on purpose has thereby already said
that private addresses from there are fine. Without this addition, the
rebinding protection would be a trap on first start: the LAN nameserver naturally
answers for `home.arpa` with `192.168.x.y`, and that is exactly the hit the
detector waits for.

## What it costs

**State in the query path.** Tunneling detection is the first part of the
project that remembers something beyond queries: one time window with
a few counters per zone. That is unavoidable — the whole point of D2 is to score
*per zone* instead of per query. It is bounded twofold: at most 4096 zones,
at most 256 remembered subdomains per zone, and both fall away after the time
window. What is stored are **salted hashes** of the subdomains, not the names.

**A model in the binary.** 107 KiB table for DGA detection. Origin,
license and generation are documented in `src/detect/dga/model.bin.md`.

**Names in the reasons.** A `Finding` carries the query name — it is meant to
carry it, otherwise a false positive is not debuggable (FEATURES.md D6). That
makes the reason the newest place where a name could escape. It is subject to
the same rules as everything in the trace (B.1 rule 3), and the leak test
`no_query_name_leaves_the_process_in_the_quiet_modes` has, since Phase 8, also
been scanning the list of notable queries.

## Reversal condition

After the observation week from the acceptance criterion: whoever has gone
through the false-positive list and set the exceptions may put individual
detectors on `block`. The obvious first one is `rebinding` — it is the only one
that is not a heuristic but a yes-no rule, and its false positives are named and
fixable via `allow_zones`.

This ADR does not stand in the way of that: it fixes the **shipped state**,
not what someone sets for their own network.
