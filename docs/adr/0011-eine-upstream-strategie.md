# ADR-0011: `split_by_zone` is the only upstream strategy

**Status:** accepted · **Date:** 2026-08-30 · **Replaces part of:** [ARCHITECTURE.md §5](../ARCHITECTURE.md)

## Context

`upstream_pool.strategy` knew three values. ARCHITECTURE.md §5 describes them and
disqualifies two of them in the same breath:

* `fastest` — "fast, but one resolver ends up seeing almost everything",
* `round_robin` — "every upstream still learns everything eventually",
* `split_by_zone` — "**the interesting case**".

A setting whose documentation calls two of its three values unfit for the
project's purpose is not a setting but a trap. Anyone who chooses `fastest`
because it sounds "fast" cancels the feature AlpenDNS is built for (FEATURES.md
P2). The price for it is lower latency to an upstream that a cache hit skips
anyway.

On top of that: the three strategies were not equally expensive in code. `fastest`
needed a sort by EWMA (`strategy::by_latency`), `round_robin` a rotating pointer
in the pool (`next: AtomicUsize`) — both exclusively for values nobody in their
right mind sets.

## Decision

`fastest` and `round_robin` are removed. `split_by_zone` remains as the only value
of `upstream_pool.strategy`.

Both names remain **recognised** in the configuration: `Strategy` gets a
hand-written `Deserialize` that says, for `fastest` and `round_robin`, what
applies instead. A derived `Deserialize` would only have reported "unknown
variant", and a configuration from yesterday would have left the operator
guessing.

`strategy::round_robin` remains as a function, because `by_zone` queues the
fallback paths behind the responsible upstream. `by_latency` is gone.

The EWMA of the response time also remains: it feeds
`alpendns_upstream_rtt_seconds` and the status display. It just no longer drives
the selection.

## Consequences

* The `strategy` key now has exactly one permitted value. That is an open
  remainder — see Alternatives.
* The failure-detection tests in the pool used to run via `round_robin`, because
  there it is fixed who is asked first. With `split_by_zone` that depends on the
  seed; the tests therefore use `seed_starting_at` to find a seed at which the
  test name lands on the desired upstream. More cumbersome, but it tests what
  actually runs in operation.
* Anyone who ran `fastest` until now gets higher latency to some of the domains
  after the update, and in exchange the split they installed the resolver for.
* A pool with *one* resolver behaves unchanged: `zone_index` always returns 0 when
  `count <= 1`.

## Alternatives

* **Keep both strategies and advise against them in the documentation.** The
  status quo. It costs code nobody should set, and it invites exactly the
  misconfiguration that cancels the feature.
* **Drop `strategy` entirely.** More consistent: a key with one permitted value is
  no choice. Not done, because the task explicitly named only the two values, and
  `deny_unknown_fields` would otherwise reject every existing configuration with
  `strategy = "split_by_zone"` at startup — for a gain of one line. The candidate
  is noted, not carried out.
* **Keep `fastest` as a test construct**, to measure it against `split_by_zone`.
  Checked: `tests/load.rs` does not use it. A comparative measurement nobody runs
  is dead code with ceremony.
