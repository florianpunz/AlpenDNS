# Synthesis Summary

Single entry point for `gsd-roadmapper`. Ingest mode: `new`. Precedence: ADR > SPEC > PRD > DOC.

## Documents consumed

- Total classifications: 25
- ADR: 25
- SPEC: 0
- PRD: 0
- DOC: 0
- UNKNOWN: 0
- Low-confidence UNKNOWN: 0
- Source manifest: `.planning/manifests/adr-core.yaml`

## Decisions

- Extracted to `decisions.md`: 25 ADRs
- Locked (22):
  - docs/adr/0001-adr-prozess.md
  - docs/adr/0003-forwarder-first.md
  - docs/adr/0004-logging-default-aggregiert.md
  - docs/adr/0006-tsig-panic-in-hickory-behoben.md
  - docs/adr/0007-hickory-net-statt-hickory-resolver.md
  - docs/adr/0008-hashmap-statt-bloom-und-trie.md
  - docs/adr/0010-api-ui-und-metriken.md
  - docs/adr/0011-eine-upstream-strategie.md
  - docs/adr/0012-fanout-entfaellt.md
  - docs/adr/0013-block-modus-refused-entfaellt.md
  - docs/adr/0014-adblock-und-rpz-parser-entfallen.md
  - docs/adr/0016-dnssec-validierung-im-forwarder.md
  - docs/adr/0017-oblivious-doh.md
  - docs/adr/0018-public-suffix-list-und-seed-rotation.md
  - docs/adr/0019-heuristiken-melden-statt-blocken.md
  - docs/adr/0020-rate-limiting-verwirft.md
  - docs/adr/0021-rebinding-nur-erreichbare-adressen.md
  - docs/adr/0022-glas-als-flaeche.md
  - docs/adr/0024-config-kompatibilitaet-bei-umbenennungen.md
  - docs/adr/0027-aufloesungsfehler-kurz-cachen.md
  - docs/adr/0028-falsches-bogus-hickory-cname.md
  - docs/adr/0029-nested-forward-zone.md
- Not locked (3):
  - docs/adr/0002-hickory-proto-statt-eigenem-parser.md (accepted in substance; status line reads "partly superseded by ADR-0007 (transport crate)"; classification `locked: false`)
  - docs/adr/0025-rebinding-auf-block.md (proposed, not decided)
  - docs/adr/0026-andere-detektoren-bleiben-auf-flag.md (proposed, not decided)

## Requirements

- Extracted to `requirements.md`: 0
- Requirement IDs: none
- Reason: 0 PRD classifications were present. No requirements were invented.

## Constraints

- Extracted to `constraints.md`: 0
- Type breakdown: api-contract 0, schema 0, nfr 0, protocol 0
- Reason: 0 SPEC classifications were present. No constraints were invented.

## Context

- Extracted to `context.md`: 0 topics
- Reason: 0 DOC classifications were present in this ingest set. No context was carried over from documents outside the set and none was invented.

## Conflicts

- Blockers: 0
- Competing variants: 0
- Auto-resolved: 2
- Other informational entries: 1
- Detail: `.planning/INGEST-CONFLICTS.md`

## Cycle detection

- Directed graph built from the classifications' `cross_refs`.
- In-set edges: ADR-0007 -> ADR-0002; ADR-0013 -> ADR-0011; ADR-0016 -> ADR-0002, ADR-0003; ADR-0017 -> ADR-0002; ADR-0018 -> ADR-0011; ADR-0025 -> ADR-0019, ADR-0021; ADR-0026 -> ADR-0019, ADR-0021, ADR-0025; ADR-0028 -> ADR-0016; ADR-0029 -> ADR-0028.
- No cycles found. The longest in-set path is 4 edges, well below the traversal cap of 50.
- References outside the ingest set (e.g. ARCHITECTURE.md, FEATURES.md, ROADMAP.md, ADR-0005, ADR-0009, ADR-0015, ADR-0023) are external nodes and were not traversed.

## Status

READY — safe to route. No blockers and no competing variants.

## Files

- `.planning/intel/decisions.md`
- `.planning/intel/requirements.md`
- `.planning/intel/constraints.md`
- `.planning/intel/context.md`
- `.planning/intel/SYNTHESIS.md`
- `.planning/INGEST-CONFLICTS.md`
