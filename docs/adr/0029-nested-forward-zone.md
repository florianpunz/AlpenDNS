# ADR-0029: The more specific zone wins, whatever its type

**Status:** proposed · **Date:** 2026-10-17 · **Replaces part of:**
[ADR-0023](0023-lokale-zonen.md) · **Concerns:**
`crates/alpendns/src/config.rs` (`validate_local_zones`),
`crates/alpendns/src/local.rs` (`LocalBackend`),
`crates/alpendns/src/main.rs` (`check`, startup log),
`config/alpendns.example.toml`,
`docs/ARCHITECTURE.md`, `docs/OPERATIONS.md`, `docs/TODOS.md` items 11 and 13

## Context

On dns1 the RFC 6303 reverse zones are closed with
`[[local_zone]] fallback = "nxdomain"`, and the domain controllers host exactly
one reverse zone: `1.168.192.in-addr.arpa`. Forwarding the whole
`168.192.in-addr.arpa` to a DC makes every other reverse name loop
DC → AlpenDNS → DC until the query timeout; measured on dns1 in the
observation period that started 2026-10-17T14:13:34Z (`queries.jsonl`,
14:13:34Z–14:51:22Z): 6 SERVFAILs with ~3 s, while the journal shows 5
upstream timeouts in the same window.

Narrowing the forward zone to `1.168.192.in-addr.arpa` removes the loop, but
the rest of `168.192` then goes upstream. There, Quad9 answers its own RFC 6303
NXDOMAIN without NSEC or NSEC3; a validating resolver cannot prove that answer
and hickory correctly calls it Bogus. In a sample at 14:52:34Z, three of ten
names outside the DC zone returned SERVFAIL in 60–100 ms. The right
configuration is therefore: close `168.192` locally *and* forward `1.168.192`
to the DC.

`Config::validate` refuses exactly that. It rejects every overlap of a
`[[local_zone]]` and a `[[forward_zone]]`, including a forward zone that is
strictly more specific than the local one. The two needs — "do not ask
strangers about my LAN" and "ask my own nameserver for the part it owns" —
cannot be expressed together.

## Decision

**Longest match wins, independent of the type.** For a question, the most
specific configured zone decides, whether it is local or forwarded. Two zones
of the same name remain a startup error: there is no longest match between
equals, and which one wins would be a guess.

Implementation, in the order the request passes through:

* `Config::validate` rejects only `local.zone == forward.zone`.
* `LocalBackend` takes the `[[forward_zone]]` names in its constructor
  (required, so a forgotten wiring is a compile error). It keeps only those
  that lie deeper than some local zone; the rest reach the `ZoneRouter`
  anyway. Before it answers from its table, it checks whether a forward zone
  is **strictly more specific** than the best matching local zone and
  contains the name; if so, it delegates to the `ZoneRouter`. The router
  itself does not change: it still owns the forwarding. Names under
  `resolver.arpa` are excluded from this delegation, so DDR keeps its place.
* `alpendns check` lists the forward zones (zone → upstream), warns about
  duplicates, and prints which zone answers for every nested local/forward
  pair. A configuration whose precedence is not visible at startup would just
  move the guessing to the operator. The start log names the nested pairs;
  `check` shows the order.
* **The zone choice stands before the table.** A local entry in a less
  specific zone does not apply under a more specific forward zone. This
  refines [ADR-0023](0023-lokale-zonen.md): the rule "an entry is more
  specific than a zone" holds *inside the winning zone*; a zone of another
  type that lies deeper wins first.
* **An open local zone keeps its fallback.** `fallback = "upstream"` means
  "entries here, everything else normal". A less specific forward zone can
  therefore answer names without an entry; that is the fallback, not a
  precedence rule. `check` says so instead of claiming the local zone owns
  the whole subtree.

## Consequences

* **The dns1 case is expressible:** one closed `168.192.in-addr.arpa`, one
  forwarded `1.168.192.in-addr.arpa`. Nothing on dns1 changes with this ADR;
  the change is proposed and stays on a branch (B.8).
* **`fallback = "upstream"` parents:** the pass-through code path already
  existed, but `validate` made the configuration unreachable. It is reachable
  now. The new part is the closed parent (`fallback = "nxdomain"`), which used
  to answer NXDOMAIN for everything.
* **A local entry under a nested forward becomes dead text.** It is not an
  error: the two zones may be edited independently, and a rule that silently
  stops applying is worse than one that is visibly overridden. `check` prints
  the direction, and `docs/OPERATIONS.md` names the rule.
* **No new rebinding exemption.** Every `[[forward_zone]]` was already in the
  rebinding allow list (`Config::rebinding_allow_zones`), and a closed local
  zone already exempted its whole subtree, so nesting does not add a name.
  What changes is that answers now exist in that subtree. Accepted: the
  forward target is explicitly configured. Note that the configuration does
  not verify that it is a LAN address — a pre-existing gap, not introduced
  here.
* **Equal zones still fail at startup.** Case differences do not help: zone
  names are lowercased and made absolute before the comparison.
* **`check` lists the forward zones and warns on duplicates.** That is the
  listing half of TODOS 13; failover between several servers per zone remains
  open.
* **An older binary refuses a nested configuration at startup.** The previous
  `validate` rejects the containment; a rollback of dns1 to 2.0.0 needs the
  pre-ADR configuration. Same class as ADR-0023's older-binary note.
* **`LocalBackend::new` grows by a parameter.** The crate is a library, but its
  API has no stability promise; the required argument turns a forgotten wiring
  into a compile error instead of a silent return to the old behaviour.
* **TODO 11 (RFC 6303 defaults) still needs the same rule:** a configured
  zone, local or forwarded, overrides a built-in default, and the most
  specific one wins. Until then the operator writes the zones explicitly.
* **`resolver.arpa` (DDR) keeps its special position.** The delegation check
  skips it explicitly, so a forward zone below `resolver.arpa` is never asked
  (and `check` shows it as configured, but dead). No practical case is known;
  named here so the gap is not discovered by accident.

## Alternatives

* **Reject nesting and write 255 `/24` local zones around the DC zone.**
  Rejected: unmaintainable, easy to get wrong, and it does not generalise to
  any other nested case.
* **Move `LocalBackend` below the `ZoneRouter`.** Rejected: the router would
  have to know the local table, and the rule "an entry beats a zone" would
  disappear — the opposite of what ADR-0023 established.
* **An `except`/`parent` key on `[[local_zone]]`.** Rejected: a second way to
  express the same nesting, more configuration surface, and still a special
  case for one type.
* **Let the more specific zone win only within the same type.** Rejected:
  that is the current state, and it cannot express the dns1 configuration.
