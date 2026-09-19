# ADR-0004: Aggregated logging as the default, query log only on request

**Status:** accepted · **Date:** 2026-08-29

## Context

Every DNS server in a LAN sees the complete browsing history of every device. The usual
solutions log that by default, because the statistics view is the selling point:
"top domains", "queries per client", "last 24 hours".

So a box in the basement holds a file that says more about a household than most other
data on the network — searchable, copyable, seizable, and instantly exfiltratable if the
device is compromised.

But without any recording at all, the server is practically unusable: "why does this page
not load anymore" needs context.

## Decision

Four modes, default `aggregate`:

| Mode | What is stored | What for |
|---|---|---|
| `none` | only global counters | maximum restraint |
| `aggregate` | counters + domain frequencies in a table without names; a domain appears in any output only from `aggregate_k` hits (default 5) | default |
| `ring` | additionally the last `ring_seconds` (default 300) in a RAM ring buffer, never on disk | debugging |
| `full` | additionally structured lines on disk | deliberate decision by the operator |

The k-anonymity threshold is the decisive part: a domain requested once appears nowhere.
Exactly the one-off requests are the telling ones.

The active mode is displayed permanently in the UI, not hidden away in a settings dialog.

## Consequences

* "Show me all requests from yesterday" does not work by default. That is the point.
* The debugging experience stays good anyway, because the decision trace exists
  independently of the log mode (see ARCHITECTURE.md §2) and is queryable in RAM for five
  minutes in `ring` mode. For "what just happened" that is almost always enough.
* More implementation effort than a simple log file: counter table, ring buffer,
  threshold logic.
* For the case that somebody needs real query logging (corporate environment, forensics),
  `full` is there — as a decision that is visible in the configuration file and shown in
  the UI, not as a silent default.

## Alternatives

* **Logging on by default, short retention:** common, but the file exists anyway.
* **Only `none` and `full`:** simpler, but then in practice everybody turns on `full`,
  because otherwise they see nothing — and leaves it on.

---

## Addendum, 2026-08-30: the counting structure, not the decision

The four modes and the default stay. Only *what* `aggregate` counts with was swapped:
instead of a count-min sketch, an exact table under a salted hash. At realistic traffic
the sketch was so imprecise that the top-domain output stayed empty. Numbers and
rationale: [ADR-0015](0015-exakte-zaehlung-statt-sketch.md).
