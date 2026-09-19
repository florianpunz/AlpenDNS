# ADR-0009: The decision trace collects through a mutex and carries names instead of IDs

**Status:** accepted · **Date:** 2026-08-29 · **Refined:** [ARCHITECTURE.md §2](../ARCHITECTURE.md)

## Context

ARCHITECTURE.md §2 sketches the trace as `SmallVec<[Step; 8]>` with steps that
refer to IDs (`AllowlistHit { list: ListId, rule: RuleRef }`). Implementing it in
Phase 5 raised two questions the sketch leaves open.

**First: how does the trace get through the pipeline?** The obvious answer is an
exclusive reference — `resolve(&self, request, ctx: &mut Ctx)`. That works up to
the upstream pool: with `fanout > 1` it queries several resolvers **at the same
time**, and each of those tasks wants to record its step. Two exclusive references
to the same trace do not exist.

**Second: who resolves the IDs?** A step carrying `ListId(3)` is not readable
without the configuration next to it. But that is exactly what
`alpendns policy test` needs — and, from Phase 6, the UI, which is supposed to
answer "why was this blocked?" without anyone shipping a lookup table alongside.

## Decision

**The trace sits behind a `Mutex` and is shared as `&Ctx`**, not passed through as
`&mut Ctx`.

**The steps carry `Arc<str>` with the name**, not the ID: `BlocklistHit {
list: Arc<str>, line: u32, matched: String }`. A trace is thereby readable on its
own.

Instead of `SmallVec`, a `Vec::with_capacity(8)` is used. That is one allocation
per request — the same order of magnitude as cloning the message, which happens
anyway, and it saves a dependency.

## Consequences

* An uncontended mutex costs a few nanoseconds; a request answered from the cache
  takes 28 µs (BENCHMARKS.md). At five steps the share is under one per mille. The
  alternative would have been to exempt `fanout > 1` from the trace — that is, to
  leave unexplained precisely the case in which several upstreams are involved.
* The trace is usable immediately after the request, without a registry.
  `explain()` returns the chain of reasoning as text, and the CLI prints it
  unchanged.
* `Arc<str>` clones per step instead of `Copy` IDs. An `Arc` clone is one atomic
  counter; next to `matched: String`, which allocates anyway, it does not weigh
  much.
* **The trace contains query names.** That is intentional and the reason it comes
  into being independently of the log mode, per ARCHITECTURE.md §2: only the
  logging layer decides what happens to it. Until that layer exists in Phase 6,
  nobody may write it to the log — the places that log today write only list names
  and line numbers (B.1 rule 3).

## Alternatives

* **Pass `&mut Ctx` through.** Zero cost, but it fails on the concurrent fanout. A
  special path for this one case would be worse than a mutex everywhere.
* **Every layer returns its steps and the caller assembles them.** Changes every
  signature in the pipeline and spreads the ordering across all layers instead of
  having it in one place.
* **IDs with a registry.** Saves memory per step and costs every reader of the
  trace a dependency on the configuration it came from. For a project whose
  purpose is explainability, the wrong trade.

---

## Addendum, 2026-08-30: the mutex is gone

The first of the two questions above had exactly one answer, and it was called
`fanout`. With `fanout > 1` the pool queried several resolvers at the same time,
and two exclusive references to the same trace do not exist — so, a `Mutex`.

`fanout` is removed ([ADR-0012](0012-fanout-entfaellt.md)). The pipeline is
therefore a chain without branching: every layer passes the context on to exactly
one next layer, the pool asks one upstream after another. A `Mutex` that is never
contended and protects nothing that is accessed concurrently is not protection but
ceremony — and it obscures the fact that access is exclusive.

`resolve` therefore gets the context back as `&mut Ctx`, as ARCHITECTURE.md §2
originally sketched it. `Ctx::record` takes `&mut self`, `Ctx::steps` returns
`&[Step]` instead of a copy — the caller in `server::handle_request` used to copy
the steps once per request just to read them.

**The second decision of this ADR stays unchanged:** steps carry `Arc<str>` with
the name, not IDs with a registry. `fanout` changes nothing about that.
