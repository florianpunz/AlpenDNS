# ADR-0001: Architecture decisions are written down

**Status:** accepted · **Date:** 2026-08-29

## Context

In a project that grows on the side over months and that a coding agent works on, the
*why* of a decision gets lost faster than the *what*. The agent has no memory across
sessions, and in six months the author won't either.

The most expensive pattern in this: a decision made deliberately is later perceived as
"weird code" and refactored away.

## Decision

Every decision that is hard to reverse or that looks wrong from the outside gets a short
document in `docs/adr/`, numbered consecutively.

Format: context, decision, consequences, alternatives. One page, no more. An ADR is not
changed, it is replaced by a new one (`Status: superseded by ADR-00xx`).

When an ADR is due:

* choice or change of a central dependency
* data structure on the hot path
* anything that touches the configuration format or wire compatibility
* every deliberate deviation from an RFC
* license

No ADR for: formatting, naming, anything that is torn down again in an afternoon.

## Consequences

Some writing work with every larger decision. In return, every new agent session can read
up in `docs/adr/` why something is the way it is, instead of "tidying it up".
