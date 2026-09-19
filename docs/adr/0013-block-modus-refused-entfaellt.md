# ADR-0013: The block mode `refused` is dropped

**Status:** accepted · **Date:** 2026-08-30

## Context

`blocking.mode` knew four values. The documentation of `refused` in the code read:

> "I will not answer that." Honest, but some clients then ask the next resolver
> in their list — and that one answers.

That is not a marginal note but the description of a block mode that does not
block. RCODE 5 (REFUSED) means to a client: *this* server does not want to, ask
someone else. Resolvers behave exactly that way — systemd-resolved, Android and
most operating-system resolvers move on to the next configured server on REFUSED,
while they accept NXDOMAIN as a final answer. A device with a second DNS entry
thus bypasses the filtering entirely, and the AlpenDNS log still shows a clean
block.

The most expensive part of that is not the missing block but that it fails
invisibly. Whoever configures `refused` sees block counters rise and believes it
works. That contradicts B.1 rule 6 ("fail closed for policy") at exactly the
point where the rule applies.

## Decision

`refused` is removed. `nxdomain` (default), `zero_ip` and `sinkhole` remain —
three answers that the client treats as final.

`BlockMode` gets a hand-written `Deserialize`, as `Strategy` did (ADR-0011),
which on `refused` explains why it is gone and what applies instead.

The test `no_mode_answers_with_refused` records that **no** remaining mode
produces REFUSED. That is the actual guarantee: not "the value has disappeared
from the enum", but "the RCODE no longer leaves the process as a block answer".

## Consequences

* Whoever had `refused` configured must switch to `nxdomain` on update — and
  then has filtering that also bites.
* REFUSED remains possible as an RCODE elsewhere (for instance what an upstream
  sends); the guarantee concerns only the block answers we generate ourselves.
* Three modes remain with visibly different prices: NXDOMAIN lies politely,
  `zero_ip` can trigger a timeout, `sinkhole` breaks on the certificate for
  HTTPS. These differences are real trade-offs and justify the setting — unlike
  a fourth value that switches the function off.

## Alternatives

* **Keep it and warn in the documentation.** The status quo, and the warning was
  already there. It did not prevent the value from being selectable.
* **Keep it, but only together with a check** that the client has no second
  resolver. A DNS server cannot know that.
