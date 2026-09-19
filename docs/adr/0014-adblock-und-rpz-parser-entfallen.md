# ADR-0014: The parsers for Adblock syntax and RPZ are dropped

**Status:** accepted · **Date:** 2026-08-30

## Context

`blocklist.format` knew five values. Two of them were explicitly subsets of a
foreign format, and both subsets had the same construction flaw.

**Adblock.** `||name^` was supported. Everything else was skipped, among it
`@@||name^` — the *exception rule*. Such lines sit in Adblock lists for exactly
one reason: a previous block rule takes in a domain that was not meant, and the
exception brings it back. Whoever loaded such a list into AlpenDNS got the block
rules **without** the corrections that belong to them. Rules with `$` options
were skipped as well, which works in the other direction — together that yields a
list whose effect has nothing to do with its author's intent any more.

**RPZ.** `<name> [ttl] [class] CNAME .` was recognized, the NXDOMAIN rule from
RFC 8611 §2.1. `rpz-passthru` was skipped (again: the exception), along with
`rpz-drop`, `rpz-tcp-only` and all triggers that do not hang off the name
(`rpz-client-ip`, `rpz-ip`, `rpz-nsdname`, `rpz-nsip`). On top of that, RPZ is
distributed as a zone transfer, not as a text file over HTTPS — the format is
built for a path that AlpenDNS does not take.

Both fall under the same sentence: **a list was loaded and was sharper than
intended in the process.** That is the most unpleasant kind of error, because it
looks like success. The `skipped` counter was in the log, but for these formats
it is *expectedly* non-zero — it does not distinguish between "comments and
element filters, no problem" and "forty exceptions thrown away, your bank site is
blocked now".

The mandate was: either support the full relevant syntax or remove it.

## Decision

Both parsers are **removed**. `hosts`, `domains` and `wildcard` remain.

**Why not the full syntax?** Because "full" for both formats means supporting
*exceptions*, and that is not a parser topic. An `Entry` would have to carry a
polarity, the `Matcher` would have to decide on a hit whether block or exception
wins (and to do so find the more specific rule instead of the first one), and the
evaluation order in `policy::Engine` — today allowlist before blocklists — would
have to get a second, in-list level. That is an intervention in the hot path and
in the trace, not planned in any phase of the roadmap, and it builds a second
exception system alongside the allowlists that already exist and are there for
exactly that. For a task whose goal is to shrink surface area, that is the wrong
direction.

There is also the benefit side: `||name^` is semantically identical to a line
`name` in a `wildcard` list. Whoever wants to use an Adblock list whose operator
offers no wildcard version converts it with a one-line `sed` — and sees for
himself what he does with the `@@` lines. That is more honest than a parser that
makes that decision silently.

Both names remain recognized in the configuration: `Format` gets a hand-written
`Deserialize` that names the reason and points to `wildcard` and `hosts`
respectively.

## Consequences

* The parser shrinks by around 130 lines and two formats for which there was no
  entry in `config/alpendns.example.toml`.
* `format = "adblock"` or `"rpz"` is a startup error with a reason, not a silent
  ignore (B.1 rule 5).
* **What is lost:** lists that exist only in Adblock syntax are no longer usable
  without preprocessing. That is the price, and it is paid deliberately: not
  readable is better than read wrongly.
* `!` at the start of a line still counts as a comment. That came from the
  Adblock world, but by now it is common in downloaded lists of every kind, and
  changing behavior nobody asked for does not belong in this commit.

## Alternatives

* **Full Adblock syntax with exceptions.** See above: matcher with polarity, new
  evaluation order, second exception system. If that is ever built, then as its
  own feature with its own phase — and then for *one* format, not two.
* **Remove only RPZ, keep Adblock.** The mandate left that open. What speaks
  against it is that both have the same flaw; to fix only one of them would mean
  deliberately leaving the other standing.
* **Keep the subset and warn louder on load** (for instance: error if more than
  *x* percent of lines were skipped). Does not solve the problem but shifts it to
  a threshold nobody can justify — and would be another configuration key.
