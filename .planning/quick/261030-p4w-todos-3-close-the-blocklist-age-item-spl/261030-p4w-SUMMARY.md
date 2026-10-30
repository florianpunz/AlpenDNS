---
phase: 261030-p4w
plan: 01
status: complete
subsystem: docs
tags: [docs, todos, blocklists, metrics, web-ui]

requires:
  - phase: 06
    provides: the API status source and the self-served UI
  - phase: 04
    provides: blocklist loading with the Age timestamps

provides:
  - "TODOS.md item 3 split: (a) closed with its four landing sites, (b) kept open and described"
  - "a bundle line that points at the remaining half instead of the finished one"

affects:
  - docs/TODOS.md
---

# Quick task 261030-p4w — split item 3

## What happened

Item 3 turned out to be half-built, not missing. Part (a), the age display, was already
in the tree at every one of its four steps; part (b), the refresh scheduler, does not
exist — `blocking.<list>.refresh` is parsed and ignored, and only `SIGHUP` reloads lists
today.

The heading now says `— (a) done, (b) open` and the Result paragraph names the four
sites: the mtime touch on `NotModified` in `filter/source.rs`, `fetched_at` and
`published_at` in `ListInfo`, the `Intl.RelativeTimeFormat` line in `web/app.js` with its
rule test, and `alpendns_blocklist_age_seconds`.

Part (b) stays visible as the remaining work: steps 5 and 6 are its specification, it
needs an ADR before it is built, and CLAUDE.md B.3 puts the update scheduler in a crate
of its own. The bundle line at the end points at 3(b).

## Verification

Docs-only; the four claims were checked against the tree before the edit and the three
plan greps pass. No code changed, so the definition-of-done commands were not re-run.

## Notes

- The bundle had said "(b) before (a)". The Result paragraph records why (a) was still
  built first rather than silently contradicting the recommendation.
- Item 8 (block page) remains stale in the same way; left alone.
