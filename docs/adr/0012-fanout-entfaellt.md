# ADR-0012: `fanout` is dropped — exactly one upstream is always queried

**Status:** accepted · **Date:** 2026-08-30 · **Entails:** addendum to [ADR-0009](0009-decision-trace-mit-mutex.md)

## Context

`upstream_pool.fanout` determined how many resolvers are queried *at the same
time*. The comment in the example configuration said what that costs:
"`>1` costs privacy, saves latency."

That is phrased too kindly. With `fanout = 2` every request sees *two* providers
instead of one. That cancels `split_by_zone` — the feature whose whole purpose is
that each provider sees only a fraction of the domains (FEATURES.md P2). A pool
with three resolvers and `fanout = 3` sends every request to all three: that is
the opposite of what the pool is there for.

The gain, by contrast, is small. What is saved is the latency of *one* upstream,
and even that only on a cache miss; the cache answers the majority of requests
without any upstream at all (BENCHMARKS.md, Phase 2). For the failures `fanout`
might otherwise help against, there is already the passive health tracking: after
three failed attempts an upstream is skipped, and even before that, on the first
failure, the next one in line takes over.

`fanout` also had a side effect deep in the architecture. It was the **only**
reason the decision trace sat behind a `Mutex`: several concurrent upstream tasks
wanted to write to the same trace, and there are no two exclusive references for
that (ADR-0009).

## Decision

`fanout` is removed. The pool queries the upstreams **one after another**, in the
order `split_by_zone` prescribes, and stops at the first success.
`FuturesUnordered` and the batching in the pool fall away; what remains is one
`for` loop.

**From that follows the rollback of the trace.** `ResolveBackend::resolve` takes
`&mut Ctx` again, `Ctx::record` takes `&mut self`, `Ctx::steps` returns `&[Step]`.
The sketch in ARCHITECTURE.md §2 was right from the start; only `fanout` stood in
its way.

The key remains **recognised**: `UpstreamPool` keeps a private field `fanout`,
which raises an error with a reason during validation. Without that,
`deny_unknown_fields` would only report "unknown field `fanout`", and anyone who
had set it would not know whether the server now queries more or less.

## Consequences

* A cache miss costs, in the worst case, the latency of a dead upstream plus that
  of the next one, instead of the maximum of both in parallel. That is the price,
  and it is the latency of a timeout — not that of a request.
* The `Mutex` on the request path is gone. It barely cost anything (ADR-0009 did
  the math), but `Ctx::steps()` cloned the entire vector on **every** request,
  because a mutex cannot hand out a reference. That copy is gone as well.
* A mutex on the request path would at some point have been read as "this is how
  we do things here". The signature now says what holds: one request, one owner.
* `Pool::new` and `Pool::with_seed` have one argument fewer.
* Anyone with `fanout = 1` in their configuration — the default — has to delete
  the line on update. Ugly, but B.1 rule 5 permits no silently ignored keys, and a
  key that does nothing any more is exactly that.

## Alternatives

* **Keep `fanout` and pin it to 1.** A key with one permitted value that continues
  to justify `FuturesUnordered` and the `Mutex` in the trace. The worst of both.
* **Allow `fanout` only on an upstream failure** ("hedged requests" after a time
  window). Defensible — the second request then only goes out if the first is
  hanging anyway. But it is a new feature with its own time parameter, and the
  task was to shrink the surface. Noted, not built.
* **Leave the trace behind the `Mutex` as a precaution**, in case something
  concurrent comes later. That is speculation, and it costs a copy of the step
  vector on every request. Should the case arrive, the way back is one commit.
