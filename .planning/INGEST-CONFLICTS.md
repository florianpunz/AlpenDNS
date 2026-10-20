## Conflict Detection Report

Ingest mode: new
Precedence: ADR > SPEC > PRD > DOC (no per-doc precedence overrides)
Classifications consumed: 25 (25 ADR, 0 DOC, 0 PRD, 0 SPEC, 0 UNKNOWN)
Cycle detection: DFS three-color marking over the in-set `cross_refs` graph; 25 nodes; no cycles; maximum depth 4 (cap 50 not reached).

### BLOCKERS (0)

None. No LOCKED-vs-LOCKED contradiction, no unresolved UNKNOWN classification, and no reference cycle was found in the ingest set.

### WARNINGS (0)

None. No PRD-classified documents were present, so there are no competing acceptance variants to resolve.

### INFO (3)

[INFO] Auto-resolved: ADR-0007 wins over ADR-0002 on the upstream transport crate
  Found: docs/adr/0002-hickory-proto-statt-eigenem-parser.md says "`hickory-resolver` is used for the upstream transports (DoT/DoH/DoQ) where it fits"; docs/adr/0007-hickory-net-statt-hickory-resolver.md says "`hickory-net` for the transports" and "`hickory-resolver` is not used".
  Note: ADR-0002's own status line reads "partly superseded by ADR-0007 (transport crate)" and the classification marks ADR-0002 `locked: false`. Locked ADR-0007 wins on the transport crate; the "no parser of our own" decision in ADR-0002 stands unchanged.

[INFO] Auto-resolved: locked ADR-0019 wins over proposed ADR-0025 and ADR-0026 on detector defaults
  Found: docs/adr/0025-rebinding-auf-block.md proposes rebinding default `block`; docs/adr/0026-andere-detektoren-bleiben-auf-flag.md proposes DGA, tunneling, typosquat and NRD stay on `flag`; docs/adr/0019-heuristiken-melden-statt-blocken.md (locked, accepted) fixes all five detectors to `flag`.
  Note: ADR-0025 and ADR-0026 are both marked "proposed, not decided" and state that the code default stays `flag` (ADR-0019). The locked ADR wins; both proposals remain recorded as `proposed` in `.planning/intel/decisions.md` for the maintainer's pending decision.

[INFO] Ingest set omits four ADRs referenced by ingested documents
  Found: ADR-0006 references ADR-0005; ADR-0012 entails an addendum to ADR-0009; ADR-0004 references ADR-0015; ADR-0029 replaces part of ADR-0023. ADR-0005, ADR-0009, ADR-0015 and ADR-0023 are not in `.planning/manifests/adr-core.yaml`.
  Note: Their decisions are absent from `.planning/intel/decisions.md`; only the superseding, refining or referencing ADRs were synthesized. No content was invented for the missing documents and no conflict was auto-resolved against them.
