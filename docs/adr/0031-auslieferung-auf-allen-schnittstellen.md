# ADR-0031: The shipping configuration listens on every interface

**Status:** accepted · **Date:** 2026-10-22 · **Concerns:**
`packaging/alpendns.toml`, `config/alpendns.example.toml`,
`crates/alpendns/src/config.rs` (the packaged-configuration tests),
`docs/OPERATIONS.md`, `docs/ROADMAP.md`, `README.md`, `CLAUDE.md` B.0

## Context

Until now the package shipped `127.0.0.1` and `[::1]`, argued in ROADMAP phase 9
step 6: a resolver listening to the internet unasked is an amplification
reflector, so a fresh installation must not be reachable from outside. The
argument is sound, and it produced the wrong product. A DNS server is installed
to answer queries; the loopback default made the first step after `apt install`
an edit, and the host on which it was forgotten answered nobody while looking
healthy — the harder failure to notice. The acceptance criterion could be met
that way, but every real installation had to change it.

## Decision

Ship `0.0.0.0:53` for UDP and TCP. Freshly installed, the server answers every
IPv4 client in the LAN without an edit. The warning moves from the default to
three places, all of them in front of the operator: the first block of
`packaging/alpendns.toml`, OPERATIONS.md under "After installation", and
`alpendns check`, which names every listener beyond the own network on every
start.

Two things stay as they were, because they carry the weight the loopback
default carried before: per-client rate limiting is on by default, and the API
keeps listening on loopback only — it shows query names (B.6), which is a
different listener and a different decision.

The alternatives were rejected:

* **Loopback, as before**, is safe and unusable in the common case; the
  operator who forgets the edit either debugs DNS for an afternoon or reaches
  for `0.0.0.0` in a hurry — with no warning in the file, because the file
  warned about the other direction.
* **`[::]:53` alone** carries IPv4 as well on a Linux with the default
  `bindv6only=0`, but on a host with `bindv6only=1` it loses IPv4 entirely, and
  in every case the IPv4 clients appear as v4-mapped addresses, which `IpNet`
  does not match against an IPv4 `[[client]]` net. Per-client policy would
  silently fall back to `default`.
* **Both wildcards** in one list do not work: the v6 socket takes the v4 one
  along and the second bind fails with `address already in use`, so the server
  would not start on a default Linux.
* **A public listener only with an explicit opt-in key** would be an
  incompatible configuration change (B.8) and a second thing to forget.

## Consequences

* On any host with a public address, a fresh installation is an open resolver —
  a reflector — until the operator acts. That is the accepted cost, and it is
  why the warning stands in the file, in the docs and in `check` instead of in
  one place.
* `packaging/alpendns.toml` is a conffile. An upgrade leaves an edited file
  alone, but replaces an **unmodified** one: a host that never changed the old
  loopback default becomes reachable with the upgrade. Whoever upgrades an
  installation that was meant to stay local should set the concrete address
  before the upgrade or check `alpendns check` right after it.
* The IPv6 omission is documented in both configuration files with the reason;
  the concrete IPv6 address is the way forward.
* No code path changed: `public_listeners()` and the `check` output already
  existed, and `is_local_address` still counts the wildcard as public. The only
  change in the tests is which side of that predicate the shipped file sits on,
  pinned next to the throttling test so the pair cannot drift apart.
