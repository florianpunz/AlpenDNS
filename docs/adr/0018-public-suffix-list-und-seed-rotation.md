# ADR-0018: Public Suffix List and seed rotation for `split_by_zone`

**Status:** accepted · **Date:** 2026-08-30 · **Addendum to:** [ADR-0011](0011-eine-upstream-strategie.md) · **Affects:** [FEATURES.md](../FEATURES.md) P2

## Context

`split_by_zone` determines the upstream via `hash(seed, registrable domain)`.
Two things about it had been noted as a deviation since Phase 3.

**The registrable domain was guessed.** What was taken were the last two
labels. For `www.example.com` that is right; for `shop.example.co.uk` it
yields `co.uk`. That sent every `.co.uk` name to the same hash value and
landed them on a single upstream — no privacy hole, but exactly the
skewed distribution that the acceptance criterion of Phase 7 (below 5 % deviation)
wanted to measure. The same holds for `.com.au`, `.ac.at`, `.co.jp` and some
hundred more.

**The seed lasted until restart.** FEATURES.md P2 sells it as a property:
*"after a restart every provider sees a different third. Over time none of them
learns a stable picture."* The second sentence is only true if someone
restarts. A service that runs for half a year — which is the stated goal of
Phase 9 — gives every provider the same slice for half a year.

## Decision

### The registrable domain comes from the Public Suffix List

Dependency `psl` (MIT/Apache-2.0). The list is compiled in: no network fetch,
no runtime file, no second kind of state that has to be kept current. It is
updated by a version update of the crate, like any other dependency.

Where the list yields nothing — a single label like `localhost`, or a name
that consists only of a suffix — the name itself counts as the unit. That is the
safe direction: when in doubt, split *less*, so that two
providers do not suddenly see the same namespace.

A table of one's own for the most common multi-part suffixes would have been the
dependency-free alternative. It would be a list that someone would have to
maintain and that nobody would maintain — and its going stale would not be
noticed, because the symptom is a slightly skewed distribution and not an error.

### The seed is drawn fresh on a regular basis

New key `[[upstream_pool]] seed_rotation`, default `"24h"`, `"0s"`
restores the behavior from Phase 3. The rotation runs as its own
task: one that hung off requests would never fire on a quiet server and
constantly on a loud one.

**The trade-off behind it is not obvious.** Rotation makes the thing
*worse* in one respect: counted over a month, every provider sees
more different domains than without it. It makes it better in the decisive
respect: none of them keeps a picture that stays stable beyond the rotation. A
profile arises from recognition over time — "this address has been asking for
the same twenty domains for months" — not from individual queries.

24 hours is the compromise: often enough that no provider sees
the same picture over weeks; rarely enough that the assignment stays
stable within a browsing day and does not hand the same domain to a second
provider in the middle of a session.

The answer cache remains untouched. It sits in front of the pool and knows no
upstreams (ARCHITECTURE.md §1); a rotation costs nothing in hit
rate, it only changes whom the next cache miss asks.

The seed lives in the pool as an `AtomicU64`, not behind a lock: it is
read on every query, written at most once a day. That a query in the middle
of a rotation still sees the old value has no consequence — the upstream it
picks with it was the right one a second earlier.

## What is verified

`ten_thousand_domains_stay_within_five_percent_per_upstream` is the
acceptance criterion itself: 10,000 domains, a fifth of them under multi-part
suffixes, across eight seeds and four pool sizes, maximum deviation per upstream
below 5 %. `a_multi_label_suffix_no_longer_lands_on_one_upstream` is the
counter-check with nothing but `.co.uk` names — with the old approximation the
deviation would have been 100 %.

`rotation_keeps_the_distribution_even` checks that every rotation yields an
even distribution and not merely a different one.
`the_rotation_task_keeps_rotating_until_shutdown` pins down the cadence and the
end.

The metrics `distribution` and `max_deviation` live in
`upstream::strategy` and not in the test code: a number that exists
only in the test quietly disappears on the next rework. The metric exposes
`alpendns_zone_seed_rotations_total` — which makes "the assignment rotates" not a
claim in the configuration but a number that rises in operation.
