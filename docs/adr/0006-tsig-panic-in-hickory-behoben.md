# ADR-0006: The TSIG panic is fixed upstream — we wait for 0.27 instead of reporting

**Status:** accepted · **Date:** 2026-08-29 · **Supersedes:** [ADR-0005](0005-tsig-panic-in-hickory-proto.md)

## Context

[ADR-0005](0005-tsig-panic-in-hickory-proto.md) accepted phase 1 even though the fuzzer
had found a panic in `hickory-proto 0.26.1`
(`src/rr/rdata/tsig.rs:387`, `end_idx - decoder.index()` runs below zero with a TSIG
record whose `RDLENGTH` is too small). The third condition there was: report it upstream.

Before reporting, we checked whether the bug was already known there. Result:

* **In `main` it no longer occurs.** The minimal repro runs through there with overflow
  checks active. Proven on 2026-08-29 against `hickory-proto 0.27.0-alpha.1` (`6c18ce30`)
  as a direct git dependency.
* **The reason is a rework, not a targeted fix.** In `main`, `end_idx` is formed from
  `decoder.len() + decoder.index()` instead of from `RDLENGTH`. That makes
  `decoder.index() <= end_idx` structurally true. The two subtractions are still in the
  code unchanged, but can no longer underflow.
* **There is no release with the fix.** `0.26.1` from 2026-05-01 is the newest stable
  state; `main` is an alpha on the way to 0.27.

## Decision

The third condition from ADR-0005 is dropped: **no issue is opened.** A report about a bug
that is already gone in the development branch costs the maintainers time, and us too.

The first two conditions from ADR-0005 still hold unchanged:

1. The acceptance criterion of phase 1 is met in the delivery configuration
   (`cargo +nightly fuzz run -O parse_request`). Proof from 2026-08-29: 2.698.186 runs in
   301 seconds, no crash, no new artifact.
2. The case stays as a known crash in `crates/alpendns/fuzz/known-crashes/`.

Added to that is the condition that ends this state:

3. **When `hickory-proto 0.27` appears, we update and re-check:**

   ```bash
   cargo +nightly fuzz run parse_request fuzz/known-crashes/parse_request
   ```

   If that runs through without aborting, `known-crashes/`, this ADR and ADR-0005
   disappear together. We do **not** move up to an alpha for it: a resolver that parses
   packets from the network does not hang on a pre-release to avoid a bug that does not
   hit the shipped path.

## Consequences

* The state is now bounded in time and tied to a concrete event, instead of holding
  indefinitely. That was the actual weakness of ADR-0005.
* Until then it stays the same: debug builds can be taken down with a single packet,
  release builds cannot. Whoever runs a debug binary has a different problem.
* We carry the risk that 0.27 takes a long time. `0.26.0` came in April 2026, a year's gap
  would not be unusual. Should the alpha become attractive earlier for other reasons, that
  is a decision of its own with its own ADR.
* The prepared report text is dropped. It is in the history should the assessment change:
  `git show 8f878c1:crates/alpendns/fuzz/known-crashes/UPSTREAM-ISSUE.md`

## Alternatives

* **Report it anyway.** Conceivable, so that `0.26.x` gets a backport. For a bug that only
  strikes with overflow checks active, a backport is unlikely though, and the work would
  fall on the people who already fixed it.
* **Switch to `0.27.0-alpha.1` now.** Fixes the bug immediately and buys an unstable API in
  the parser of a network service. Wrong trade.
* **Our own fork with a one-line fix via `[patch.crates-io]`.** Technically clean, but we
  would maintain a fork, `cargo deny` would have to allow a git source
  (`unknown-git = "deny"`), and all that for a bug that does not hit us in operation.
