# ADR-0025: Rebinding detector switches to block

**Status:** proposed · **Date:** 2026-10-16 · **Affects:** phase 8 acceptance, detector defaults

> **Proposed, not decided.** The code default stays `flag` (ADR-0019). The
> decision is made by the maintainer after the false-positive review of the
> observation period; until then this document is a proposal.

## Context

Phase 8 (heuristics) shipped with all detectors on `flag` by default
([ADR-0019](0019-heuristiken-melden-statt-blocken.md)). The first observation
period (2026-08-30 to 2026-09-08) found that the rebinding detector reported
1,763 findings — **all false positives**. The hit list was dominated by sinkhole
addresses (`0.0.0.0`, `::`) and the NAT64 range (`192.0.0.0/16`), none of which
are actual attacks.

[ADR-0021](0021-rebinding-nur-erreichbare-adressen.md) corrected this by
restricting the detector to addresses where a device can actually sit:
RFC 1918, loopback, link-local, broadcast, CGNAT and benchmark ranges. Removed
were `0.0.0.0`, `::` and `192.0.0.0/16`.

The second observation period (since 2026-09-16) with the corrected list has
reported **zero findings** in eight and a half days of real traffic.

## Decision

**The rebinding detector defaults to `block`.**

Rationale:

1. **Rebinding is not a heuristic.** It is a yes-no rule: either the answer
   contains a private address for a public name, or it does not. There are no
   false positives with the corrected address list ([ADR-0021](0021-rebinding-nur-erreichbare-adressen.md)).

2. **The attack surface is real.** Rebinding attacks (DNS rebinding) are a
   known technique for bypassing same-origin policies in browsers.

3. **The cost of a false positive is manageable.** Any site that legitimately
   returns a private address for a public name belongs in the `allow_zones`
   exception list — which is a deliberate configuration, not an accident.

4. **Consistency with the self-protection promise.** AlpenDNS explicitly
   filters private addresses; a public name resolving to one is exactly the
   case the filter is meant for.

## Consequences

* Rebinding is the first detector to move from `flag` to `block`.
* The `allow_zones` mechanism (already in place) becomes the escape hatch for
  legitimate use cases.
* Other detectors (DGA, tunneling, typosquat, NRD) remain on `flag` until their
  own observation period confirms sufficiently few false positives.

## Reversal condition

If a legitimate use case emerges that cannot be handled by `allow_zones` and
that breaks real-world usage, the default reverts to `flag` and the issue
is reassessed.
