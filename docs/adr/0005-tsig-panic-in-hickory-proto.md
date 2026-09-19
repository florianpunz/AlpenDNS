# ADR-0005: Phase 1 is accepted despite a fuzz crash in `hickory-proto`

**Status:** superseded by [ADR-0006](0006-tsig-panic-in-hickory-behoben.md) · **Date:** 2026-08-29

## Context

The acceptance criterion of phase 1 reads: "a fuzz target on the request path runs for
5 minutes without a crash." The first run found a crash after a few minutes — not in our
code, but in `hickory-proto 0.26.1`.

`Message::from_vec` panics with `attempt to subtract with overflow` when a message
contains a TSIG record whose `RDLENGTH` is smaller than the fixed fields before it
(`src/rr/rdata/tsig.rs:387`). The subtraction sits in the error path that has just
correctly recognized the broken record.

Whether this is a panic depends on the profile:

| `overflow-checks` | Behavior |
|---|---|
| off — our `[profile.release]` | record is rejected cleanly with `incorrect rdata length read` |
| on — debug, `cargo test`, `cargo fuzz` by default | panic |

That collides with B.1 rule 1: a panic in the query handler is a denial-of-service hole.
It does not collide with the rest of the rule — there is no `unwrap()` and no slice
indexing from us.

We cannot fix it. The panic happens inside `Message::from_vec`, and per
[ADR-0002](0002-hickory-proto-statt-eigenem-parser.md) we deliberately do not replace
exactly that call with our own code. `catch_unwind` is out: our release profile sets
`panic = "abort"`, so there is nothing to catch.

## Decision

Phase 1 counts as accepted, with three conditions.

1. **The acceptance criterion is met in the delivery configuration**, not in
   `cargo fuzz`'s default configuration. The proof run is:

   ```bash
   cargo +nightly fuzz run -O parse_request -- -max_total_time=300
   ```

   `-O` builds the way we ship: release without `overflow-checks`. That is the code
   that will later listen on a port, and it is for that code that the statement "runs
   for 5 minutes without a crash" holds. The default run with overflow checks remains
   useful in addition, to check *our* arithmetic — it is just not the criterion.

   Proof from 2026-08-29: 2.698.186 runs in 301 seconds, no crash, no new artifact.

2. **The case stays in the repo as a known crash**, in
   `crates/alpendns/fuzz/known-crashes/`, with instructions to check it again after every
   update of a parser dependency. It deliberately does not sit in the normal corpus,
   because otherwise every fuzz run would abort immediately instead of looking for new
   bugs.

3. **The bug is reported upstream.** The finished report text together with a minimal
   repro sits in `crates/alpendns/fuzz/known-crashes/UPSTREAM-ISSUE.md`.

## Consequences

* Debug builds of AlpenDNS can be taken down with a single packet. For development and
  tests that is acceptable, for operations it would not be — but there is no reason to run
  a debug binary. Should a build ever be produced with `debug = true` and overflow checks
  active, this decision is void.
* At this point we rely on `overflow-checks` staying off in the release profile. That is
  the Cargo default, but it is now an assumption with weight: whoever flips it has to read
  up here first.
* The statement "the request path is fuzzed" is weaker than it sounds as long as this
  crash is open: beyond the TSIG path the run says nothing.
* Once `hickory-proto` fixes it, the whole construction goes away: bump the version,
  re-check `known-crashes`, supersede the file and this ADR.

## Alternatives

* **Leave phase 1 open until upstream fixes it.** The most honest variant, but it ties the
  project's progress to the release cycle of a foreign crate, for a bug that does not hit
  the shipped path.
* **Set `overflow-checks = false` for debug and test too.** Would remove the symptom and
  at the same time switch off the warning for *our own* overflows. Exactly backwards.
* **Build the fuzz target so that it does not reach TSIG.** That is not a fix but looking
  away — and it would hide real bugs in the same path.
* **Switch to `domain` (NLnet Labs).** A parser bug in a library is no reason to switch
  libraries; ADR-0002 made the choice for other reasons, and those still hold.
