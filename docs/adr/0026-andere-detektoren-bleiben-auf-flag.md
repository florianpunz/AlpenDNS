# ADR-0026: DGA, tunneling, typosquat, NRD stay on flag

**Status:** proposed · **Date:** 2026-10-16 · **Affects:** phase 8 acceptance, detector defaults

> **Proposed, not decided.** The code default stays `flag` (ADR-0019). The
> decision is made by the maintainer after the false-positive review of the
> observation period; until then this document is a proposal.

## Context

Phase 8 (heuristics) shipped with all five detectors on `flag` by default
([ADR-0019](0019-heuristiken-melden-statt-blocken.md)). The first observation
period (2026-08-30 to 2026-09-15, [ROADMAP.md](../ROADMAP.md) phase 8) produced mixed results:

* **Rebinding:** 2,595 reports, all false positives — corrected by [ADR-0021](0021-rebinding-nur-erreichbare-adressen.md), proposed for `block` ([ADR-0025](0025-rebinding-auf-block.md)).
* **DGA:** 7 findings, no false alarms — promising, but the sample size is too
  small to generalize.
* **Tunneling:** Never triggered in real traffic — only validated against
  corpus, not proven in operation.
* **Typosquat:** Ran empty — `protect` list was not configured.
* **NRD:** Ran empty — no `nrd.txt` from AlpenShield.

The second observation period (since 2026-09-16) has:

* Typosquat: Now configured with a filled `protect` list, needs evaluation.
* Tunneling: Still needs a controlled test to prove it triggers.
* NRD: Still empty, pending AlpenShield.
* DGA: Confirmed 0.077% false positive rate on top-100k corpus, but real
  traffic validation is still limited.

## Decision

**DGA, tunneling, typosquat and NRD remain on `flag`** until their respective
observation criteria are met.

### DGA

**Criterion:** 1-2 weeks of operation with `flag` on real traffic, reviewing the
false-positive list.

**Current status:** Measured false positive rate of 0.077% on top-100k corpus
(BENCHMARKS.md). The primary remaining false positives are Pinyin abbreviations
(e.g., `hnqxdzkj.com`), which are indistinguishable from random generation for
a model trained on Latin text.

**Next step:** After reviewing the false-positive list from real traffic,
if the rate is below 0.1% (the documented threshold), the detector can move
to `block`.

### Tunneling

**Criterion:** A controlled test that proves the detector triggers on real
malicious traffic.

**Current status:** The iodine/dnscat corpus triggers correctly (hit rate
0.81–1.0), but ordinary traffic stays at 0.17 — both good signs. However,
the detector never triggered in real operation, which means "no false positives"
but also "no proof it works".

**Next step:** Set up a controlled test in the lab: deliberately send
iodine/dnscat traffic through the resolver and verify the detector reports
it. Once proven, move to `block`.

### Typosquat

**Criterion:** One week of operation with a filled `protect` list (second
observation period).

**Current status:** The second period has been running since 2026-09-16 with
a filled `protect` list. Needs evaluation of the findings.

**Next step:** After evaluating the second period, if the false-positive rate
is acceptable, move to `block`.

### NRD

**Criterion:** Data from AlpenShield. The detector runs empty until the
`nrd.txt` file is provided.

**Current status:** The file path is configurable (`detection.nrd.source`).
The detector stays `off`
without data.

**Next step:** Once AlpenShield provides regular `nrd.txt` files, run the
detector on `flag` for an observation period, then evaluate.

## Rationale for staying on flag

The central insight from [ADR-0019](0019-heuristiken-melden-statt-blocken.md):
*a detector that breaks the internet gets switched off — and with it all the
others.* Four heuristics together are worth more than any single one. Moving
to `block` is a one-way door: once a detector blocks a site the user wants,
they switch it off entirely and may not come back.

Rebinding is the exception ([ADR-0025](0025-rebinding-auf-block.md)) because
it is not a heuristic: it is a binary rule with no false positives after
[ADR-0021](0021-rebinding-nur-erreichbare-adressen.md). The others require
more validation.

## Reversal condition

Any of these detectors can move to `block` individually once:

* It has run on `flag` in production for at least one week (or equivalent
  controlled testing for tunneling).
* The false-positive list has been reviewed.
* The false-positive rate is below 0.1% (or acceptable for that detector's
  purpose).

A detector that moves to `block` and then causes problems reverts to `flag`
and stays there until the next major version.
