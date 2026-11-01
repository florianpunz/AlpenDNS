# ADR-0033: No product generations — SemVer is the only version

**Status:** accepted · **Date:** 2026-11-01 ·
**Concerns:** `README.md`, `CLAUDE.md` B.0, `docs/ROADMAP.md`,
`config/alpendns.example.toml`, `crates/alpendns/src/resolve.rs`

## Context

The documentation labeled the product with a generation number in prose, next
to the SemVer number in `Cargo.toml`. Two version schemes for one product invite
a question that neither the code nor the documentation answers: which features
belong to the next generation, and when does it begin? The label also presented
the current scope — a forwarding resolver — as a first installment rather than
as the product.

## Decision

**The product is called AlpenDNS. Its only version is the SemVer number in
`Cargo.toml`.** There are no product generations and no plan for one. No
generation label appears in the documentation, the configuration or the code.

Recursion is not deferred to a later generation. It is an optional feature that
is not planned ([ADR-0003](0003-forwarder-first.md)); the slot behind the
`ResolveBackend` trait stays open, exactly as it was.

Dated ADRs keep their wording. They record what was decided and written at the
time, and rewriting them would make the record look more consistent than the
history was.

## Consequences

* A release is identified by its SemVer number, not by a generation label.
* The change alters no behaviour, no configuration key and no API. It is a
  wording change in documentation and doc comments.
* "Forwarding resolver" describes the product, not a stage on the way to
  something else.

## Alternatives

* **Keep the generation label and announce a successor for recursion.**
  Rejected: it makes an unplanned feature look scheduled and invents a release
  boundary that nobody defined.
* **Spell the SemVer number out in prose as "AlpenDNS 1.x".** Rejected: the
  number already says that, and a second spelling of it can drift.
