# TODOS — open items with an implementation plan

This list stood here before as a bare enumeration. It still stands further down
unchanged in substance, but now with a plan each: **finding** (what the code really does
today, with a reference), **design**, **steps in the verify format** as in the roadmap,
what is deliberately *not* being built, and effort.

None of these items belongs to a phase with acceptance criteria. Everything here is
phase 10 material in the sense of [ROADMAP.md](ROADMAP.md): discardable at any time, no
order except the one recommended at the end.

All file and line references date from 2026-09-01. Line numbers age faster than the
rest; the module and function names are the durable part.

---

## Three findings that cut across several items

While going through the code for this plan, three things stood out that are in no TODO
but affect several of them. They belong settled first, otherwise several plans rest on a
wrong assumption.

**B1 — There is no `SIGHUP` reload — done.** [ARCHITECTURE.md](ARCHITECTURE.md) §7
describes a reload in which a broken configuration is discarded and the old one stays
active. That is now built: `reload_on_hangup` in `main.rs` reloads the policy layer on
`SIGHUP` — clients, policies, regexes, schedules and list sources, swapped in atomically
— and leaves the old state standing when the configuration is broken. What is not
hot-reloadable (listeners, upstreams/TLS, cache, rate limiting, block mode, detectors)
is named by the log on every reload. §7 has been adjusted accordingly; the
cross-references to B1 in items 7, 9 and 10 are thereby obsolete.

**B2 — Blocklists are loaded only at startup.** In
[filter/](../crates/alpendns/src/filter/) and [main.rs](../crates/alpendns/src/main.rs)
there is no update scheduler; the "update scheduler" named in CLAUDE.md B.3 is the
target picture, not code. The lists come in once via `filter::source`, which writes
`<name>.list` and `<name>.meta` (with `etag:` and `last-modified:`) into the
`CacheDirectory`. For item 3 that means: "last updated 2 days ago" today measures
essentially how long the process has been running. The item is therefore only half as
big as it looks — and the other half (reloading regularly) is the part that is really
missing.

**B3 — The query log is written synchronously in the request path.** `QueryLog::new`
([logging/mod.rs:367-375](../crates/alpendns/src/logging/mod.rs#L367-L375)) opens the
file once with `create` + `append` and puts it behind a `Mutex`; writes go with
`writeln!` straight into the record
([logging/mod.rs:483-490](../crates/alpendns/src/logging/mod.rs#L483-L490)) —
unbuffered, under the same lock for all requests. Only affects `mode = "full"`, so it is
not the default, but it is exactly the kind of lock in the request path that B.3 rule 5
wants to avoid. Belongs to item 2 and is done there along with it.

---

## 1. TCP slowloris — done.

> Timeout on the body read + a cap on concurrent connections. In a LAN one compromised
> device is enough to bring the resolver to its knees.

**Built** in [server/tcp.rs](../crates/alpendns/src/server/tcp.rs): `BODY_TIMEOUT = 5 s`
around the body read, in the `select!` with `shutdown`; a `Semaphore` with
`MAX_CONNECTIONS` whose permit is taken **before** the `accept` and hangs on the task;
`MAX_PER_CLIENT = 8` per source IP via a `HashMap` behind a mutex in connection setup.
Three counters (`alpendns_tcp_connections_rejected_total`, `..._at_capacity_total`,
`..._body_timeouts_total`), numbers in [BENCHMARKS.md](BENCHMARKS.md), operational side
in [OPERATIONS.md](OPERATIONS.md) §4.

Three deviations from the plan, each with its reason:

* **`MAX_CONNECTIONS = 64` instead of 256.** The number is small enough that the test can
  check the *real* constant without needing 256 descriptors — and 64 concurrent TCP
  connections for DNS is a lot in a household LAN. Incidentally, "permit before
  `accept`" means one is permanently reserved: 63 are served at the same time.
* **A third counter.** The two planned ones would have left the ceiling invisible: it
  rejects nothing, it makes things wait. Without `at_capacity`, "the limit holds" would
  look exactly like "the limit was never reached". It is counted **before** the wait, not
  after — counted afterwards, the number would only arrive once the slot frees up.
* **Testing is split in two.** The body timeout runs over `tokio::io::duplex` with the
  clock halted (`handle_connection` became generic over the stream for that) — on real
  sockets the automatically advancing test clock would race epoll. The two ceilings run
  on real sockets without a clock, because no time is involved there; the counter is
  waited for briefly instead of being assumed, because the accept path trails the kernel
  backlog.

**Finding.** Two gaps, both in [server/tcp.rs](../crates/alpendns/src/server/tcp.rs):

1. The `IDLE_TIMEOUT` of 10 s sits only on reading the **length prefix**
   ([tcp.rs:70-79](../crates/alpendns/src/server/tcp.rs#L70-L79)). One line further the
   body is read with `stream.read_exact(&mut packet).await?`
   ([tcp.rs:85](../crates/alpendns/src/server/tcp.rs#L85)) **without any time limit**.
   Anyone who sends two bytes `0xFF 0xFF` and then goes silent holds a task, a 64 KB
   vector and a descriptor indefinitely. The `select!` on `shutdown` is missing here as
   well — such a connection additionally delays shutdown until `TimeoutStopSec=10s`
   takes effect.
2. `tracker.spawn` in the accept loop
   ([tcp.rs:43](../crates/alpendns/src/server/tcp.rs#L43)) has **no ceiling**. The limit
   today is the process's descriptor limit.

The existing rate limiting does not help: `RateLimiter` is consulted in `handle_request`,
that is, **per request** — and a slowloris connection never makes a request. The second
gap is the more dangerous one; without it, the first only costs a task.

On the server side there is only UDP and TCP (`server/` contains `udp.rs`, `tcp.rs`,
`mod.rs`); DoT/DoH/DoQ are upstream transports only. The fix touches exactly one file.

**Design.** Three constants in the module, no configuration — in the sense of B.2,
measured values in the code instead of another switch that nobody turns:

* `BODY_TIMEOUT = 5 s` around the `read_exact` of the body, in the same `tokio::select!`
  with `shutdown` as the prefix read. Five seconds for at most 64 KB from the LAN is
  generous.
* `MAX_CONNECTIONS = 256` as an `Arc<Semaphore>`. The permit is taken **before** the
  `accept` (`acquire_owned().await`), not after: then the connection never builds up at
  all, instead of being accepted and closed immediately, and the kernel backlog
  throttles by itself. The permit hangs on the task and falls with it.
* `MAX_PER_CLIENT = 8` concurrent connections per source IP. Without it, one device
  occupies all 256 slots, and the ceiling itself becomes a weapon. A small
  `HashMap<IpAddr, u32>` behind a mutex in the accept loop is enough — that mutex is not
  in the request path but in connection setup.

All three only take effect once it is already too late; in normal operation they cost one
semaphore access per connection.

**Steps.**

```
1. BODY_TIMEOUT around the body read, in the select! with shutdown → verify: test sends the
   prefix, then nothing; the connection is closed after 5 s, other connections keep being
   served
2. Semaphore before accept, permit on the task → verify: test opens MAX_CONNECTIONS+1 silent
   connections; the last one only gets through when an earlier one drops
3. Limit per source IP → verify: one IP with MAX_PER_CLIENT+1 connections does not block a
   second IP; its request is answered normally
4. Counters in metrics.rs: rejected connections, body timeouts
   → verify: /metrics shows both after the load test
5. Numbers in BENCHMARKS.md → verify: throughput before/after the change, deviation named
```

Testing happens without `sleep`: `tokio::time::pause()` plus `advance()` makes the
timeouts deterministic; the semaphore needs no time anyway.

**Deliberately not:** no closing of existing connections when the limit is reached
(whoever is already talking talks to the end), no configurability, no `slip` behaviour.

**Effort:** S, one evening. **Risk:** low, the change is local. No ADR needed.
**Priority: high** — the only item on the list that closes an exploitable gap, and the
cheapest one at that.

---

## 2. Log rotation in `full` mode

> logging/mod.rs writes append-only into a file that the process opens itself —
> systemd/journald does not reach it. Over months the query log grows without bound until
> the disk is full.

**Finding.** Correct, see B3. A retention period exists nowhere; the
`[privacy.logging]` section in [config.rs](../crates/alpendns/src/config.rs) has no
corresponding key. An entry is a JSON line with name, type, client, RCode, reason,
duration and findings — roughly 200 bytes. At 50 requests/s that is around **850 MB per
day**. The disk fills not in months but in days.

**The decisive point is not a space problem.** This file is the only place in the system
where query names survive the process (B.1 rule 3, ADR-0004). "Rotation" here means
above all: **how long are names kept.** That is a privacy decision and therefore belongs
in the configuration, not in a constant.

**Design — `logrotate`, not a homegrown solution.** Because the file is opened with
`.append(true)`, every `write` goes to the current end of the file, regardless of the
descriptor's offset. That makes `logrotate` with `copytruncate` work **without any code
change and without a reopen signal** — no hole in the file, no lost line except those in
the window between copy and truncate. That is exactly why a homegrown solution gains
nothing here.

Into the package goes `packaging/logrotate/alpendns`:

```
/var/log/alpendns/queries.jsonl {
    daily
    rotate 7
    maxsize 100M
    compress
    delaycompress
    copytruncate
    missingok
    notifempty
    create 0640 alpendns alpendns
}
```

`rotate 7` is a default, not a law of nature — it belongs in OPERATIONS.md with the
sentence that it is the **retention period of the names**. `create 0640` matches
`LogsDirectoryMode=0750` and `UMask=0077`; without that line the new file inherits the
permissions of the old one, which here would happen to be right but not guaranteed.

Plus two additions that come with the same move:

* **A `BufWriter` around the file** (B3). The request path then writes into the buffer
  instead of into a syscall; a task flushes it once a second and on shutdown. A crash
  costs at most one second of log — for the query log that is no loss anyone would
  mourn.
* **A hint in `alpendns check`** when `mode = "full"` is configured and
  `/etc/logrotate.d/alpendns` is missing. That is the case in which someone today fills
  the disk without noticing.

**Rejected alternatives.**

* *Rotation in the process* (size, age, count): around 150 lines that Debian already has,
  including the error cases around renaming while writing is in progress.
* *Handing it to journald:* violates B.1 rule 3. Retention would then follow the journald
  configuration instead of the project configuration, the names would sit in a place that
  every `journalctl` reads, and `SystemMaxUse` is a size, not a deadline.
* *`maxsize` alone without a `rotate` limit:* bounds the space, not the retention.

**Steps.**

```
1. logrotate file into the package, registered in Cargo.toml (assets) → verify: dpkg -c shows
   it under /etc/logrotate.d/; logrotate -d reports no error
2. Test that writing continues after copytruncate → verify: empty the file with truncate,
   keep writing, the size grows from 0 instead of from the old offset
3. BufWriter plus a flush task once a second → verify: existing logging tests green; one test
   proves that an entry is in the file after the flush
4. Hint in alpendns check when mode=full without a logrotate file → verify: check reports it,
   but does not fail because of it
5. OPERATIONS.md: rotate 7 = seven days of names, and how to change it → verify: the sentence
   stands in the section on the log
```

**Effort:** S–M, one evening. **Priority: high as soon as someone turns on `full`** —
until then zero, because in the default mode no file is opened at all.

---

## 3. Age of the blocklists in the UI

> For blocklists, record "age" in the UI — last updated (2 days…) ago

**Finding.** `ListInfo` ([api/mod.rs:70-76](../crates/alpendns/src/api/mod.rs#L70-L76))
has `name`, `entries`, `format` — **no timestamp**; it is built in
[main.rs:329](../crates/alpendns/src/main.rs#L329). The loader knows `Origin` (`File` /
`Network` / `NotModified` / `StaleCache`,
[filter/source.rs:41-52](../crates/alpendns/src/filter/source.rs#L41-L52)), but only as
the origin of that one load, not as a point in time. On disk lie `<name>.list` and
`<name>.meta` with `etag:` and `last-modified:` — the latter is the **publisher's**
statement, not the time of retrieval.

Plus finding B2: **there is no refresh at runtime.** Without it, "last updated" only
answers when the service was last restarted — a display that is of no use exactly when
you need it.

**Design.** The item falls into two, and the second is the more important one.

**(a) Show the age.** Two times per list, because they answer two different questions:

* *fetched at* — the time of the last successful retrieval. The source is the mtime of
  `<name>.list`; it survives the restart and therefore need not be held in the process.
  On `Origin::NotModified` the mtime is touched, because "the server says it is
  unchanged" means: it is current.
* *published at* — the `last-modified:` value from the `.meta` file, provided the
  publisher supplies it. A list that has been unchanged for eight months is a different
  problem from one that has not been fetched for eight months.

`ListInfo` gets two `Option` fields as absolute points in time (RFC 3339). **The
formatting into "2 days ago" is done by the browser**, not the server: a relative
statement goes stale while the page is open, and the page reloads periodically anyway.
`Intl.RelativeTimeFormat` is part of the platform and therefore not an external
dependency (B.6).

On colour: per B.6, `--warn` is reserved for high latency, and the rule stands as a test
in [api/ui.rs](../crates/alpendns/src/api/ui.rs). A stale list is therefore **not a
colour** but text — "fetched 34 days ago" says everything red would say too. Whoever
wants emphasis uses font weight.

Plus a metric `alpendns_blocklist_age_seconds{list="…"}`, so that the state is visible
without an open UI.

**(b) The actual point: reload regularly.** A task that reloads each list at a configured
interval (default 24 h), builds the `Matcher` and swaps it in via `ArcSwap` (B.3 rule 5).
A failure is not an error: the old list stays active, the counter rises, the age grows
visibly — exactly the fail-open case from B.1 rule 6.

That is the larger change and belongs **decided, not built on the side**: it is the
update scheduler that CLAUDE.md B.3 envisages as its own crate.

**Steps.**

```
1. Touch the mtime on NotModified → verify: test with a local 304 server, the mtime is new
   afterwards
2. Two time fields in ListInfo, source mtime + .meta → verify: /api/lists delivers both; a
   test with a missing .meta delivers null instead of an error
3. UI: line "geholt vor …" per list, formatting in the browser → verify: ui.rs rule tests
   green, in particular the colour check; then a human look
4. Metric alpendns_blocklist_age_seconds → verify: /metrics shows one value per list
5. [decide separately] refresh task with ArcSwap swap → verify: test with two list states
   behind a local server; after the interval the new state takes effect without any request
   failing in between
6. [separate] A failure keeps the old list → verify: server answers 500, matcher unchanged,
   counter rises, age grows
```

**Effort:** (a) S, half an evening. (b) M, one to two evenings, plus an ADR.
**Priority:** (a) low, (b) medium — a blocklist that is never updated is the quiet failure
of the core promise.

---

## 4. Document the dependency on the system clock

> The project's own validation (ADR-0016) presupposes a correct host clock — a wrong
> clock makes every signed zone "bogus".

**Finding.** [dnssec.rs](../crates/alpendns/src/dnssec.rs) does not check the signature
times itself; `hickory-net` does that during validation, against the system clock. So
there is **no skew buffer here that one could configure** — and that is as it should be:
a buffer would water down exactly the property ADR-0016 was written for.

Three things are concretely missing:

1. **The unit does not order itself after time synchronisation.**
   [alpendns.service](../packaging/systemd/alpendns.service) has
   `After=network-online.target`, but **no `After=time-sync.target`**. At boot the
   resolver can therefore start before the clock is set. That is the practically most
   common case of a wrong clock — not the attack, but a box without an RTC (Raspberry Pi)
   that comes up with the epoch date.
2. **The failure mode is hard.** Bogus means SERVFAIL. A wrong clock therefore paralyses
   *every* signed zone, that is, the larger part of the net, and looks in the metrics
   exactly like an attack. That is the kind of outage where you search in the wrong
   direction for an hour.
3. **It is written down nowhere.** Neither OPERATIONS.md §4 (troubleshooting) nor
   THREAT-MODEL.md mentions the clock.

Side finding: `ProtectClock=yes` is in the unit. The service cannot set the clock — as it
should be, and precisely for that reason setting it is the system's job and belongs in
the runbook.

**Design — three small things, no feature.**

* **`After=time-sync.target` into the unit.** One line. Explicitly **not** `Wants=`:
  whoever has deliberately switched off `systemd-timesyncd` and sets the time otherwise
  should not have to start the resolver with it. `After=` without `Wants=` only orders
  when the target runs anyway — that is the right strength of the statement.
* **A section in OPERATIONS.md §4**, title roughly *"Almost everything is SERVFAIL"*.
  Content: DNSSEC validation computes signature times against the system clock; if the
  clock is wrong by more than the signature window, every signed zone is bogus and
  therefore SERVFAIL. Check with `timedatectl` (`System clock synchronized: yes`). NTP is
  a prerequisite, not a comfort. On devices without an RTC the clock is wrong after every
  power failure until the network is up. Whoever wants to prove the cause sees it in the
  counter of bogus answers: from some point on it jumps and never falls again.
* **A hint in `alpendns check`:** if DNSSEC is active and the clock is not synchronised
  according to `/run/systemd/timesync/synchronized` or `adjtimex`, `check` says so — as a
  **hint, not as an error**. It must not prevent startup: a resolver that does not start
  at all with an unset clock is, when booting without a network, exactly the
  chicken-and-egg problem that `check` deliberately avoids according to
  [main.rs:812-822](../crates/alpendns/src/main.rs#L812-L822).

**Rejected:** a skew buffer (waters down ADR-0016); automatically switching off validation
when the clock is unsynchronised (an attacker who can move the clock thereby switches off
DNSSEC — the inversion of the protection goal); a NTP query of our own (violates B.1
rule 4, the process contacts exactly three kinds of targets).

**Steps.**

```
1. After=time-sync.target into the unit → verify: systemd-analyze verify alpendns.service
   without errors; systemctl list-dependencies --after shows the target
2. Section in OPERATIONS.md §4 → verify: someone with the symptom "everything SERVFAIL"
   finds it via the heading
3. Clock hint in alpendns check → verify: test with a faked synchronisation status; check
   gives the hint and still returns exit 0
4. Sentence in THREAT-MODEL.md: a wrong clock as an availability risk that is not protected
   against → verify: the item stands in the list of deliberately open points
```

**Effort:** S, half an evening. **Priority: medium** — step 1 is one line with real
benefit on every device without an RTC.

---

## 5. Time-limited grants and blocks do not survive a restart

> Grants/denials are only in RAM (temporary.rs). A restart loses them. That is defensible,
> but it belongs documented as a deliberate decision.

**Finding.** Confirmed: `Temporary` holds a `Mutex<HashMap<String, Instant>>`
([policy/temporary.rs:26-30](../crates/alpendns/src/policy/temporary.rs#L26-L30)),
instantiated twice — once for grants, once for blocks.

**Why persistence here is more expensive than it looks** is already in the module header:
the deadline runs over `Instant`, that is, over **monotonic** time, explicitly so that a
moved system clock does not extend a grant. An `Instant` does not survive a process and
cannot be serialised. Persistence would mean: converting to wall-clock time when writing,
back when loading — and thereby introducing exactly the clock dependency that the module
avoids. Not a knockout argument (the deadline is short, the damage bounded), but the core
of the decision, and that is how it belongs written down.

**Design — document, do not build.** Two places, no change to the logic:

1. **A paragraph in the module header of `temporary.rs`**, directly after the existing
   paragraph on monotonic time, roughly:

   > **None of this survives a restart, and that is deliberate.** The entries are
   > time-limited; persisting them would mean converting their deadline to wall-clock
   > time and back when loading — and thereby introducing exactly the clock dependency
   > that the paragraph above avoids. A grant that outlives a reboot is, moreover, no
   > longer a time-limited grant but an allowlist with an expiry date; whoever wants that
   > enters the name in an allowlist.

2. **One sentence in the interface.** That is the actual fix of the TODO: the user should
   not notice it after the reboot but read it on clicking. In the grants panel, small and
   in `--muted`: *"Grants last until they expire or until the service is restarted."*
   Costs one line of HTML and removes the surprise completely.

Plus a footnote in OPERATIONS.md §3 (backup): grants and blocks are not in the backup,
because they do not lie on disk.

**If persistence is wanted after all** — then like this and not otherwise: written on
every change (not on shutdown; a crash is the case you build it for) into
`StateDirectory`, atomically via a temporary file plus `rename`. What is stored is the
**expiry time as wall-clock time**; on loading, everything expired is discarded and
everything remaining is converted to `now() + remaining lifetime`, capped at `MAX_GRANT`.
If the clock jumps backwards, that cap is the only safeguard — and therefore not
negotiable. Around 80 lines plus tests. The honest advice: do not build it before someone
has complained about it.

**Effort:** S, one hour for variants 1+2. **Priority: high** — the cheapest item on the
list, and it removes a real surprise.

---

## 6. Continuous dependency triage

> cargo deny runs, but who reacts to new RUSTSEC advisories?

**Finding.** [.github/workflows/ci.yml](../.github/workflows/ci.yml) has exactly two
triggers: `push` to `main` and `pull_request`. **No `schedule`.** But the advisory
database changes without anyone committing — in a repo that is worked on irregularly in
the evenings, a new advisory therefore only comes to light on the next commit, possibly
weeks later. That is the actual gap, and it costs five lines.

The second half is the triage path, and its pattern already exists and is good: the
exception for `RUSTSEC-2026-0009` in [deny.toml](../deny.toml) carries a rationale, a
reachability analysis of the vulnerable path and a **reversal condition** ("falls away as
soon as the MSRV rises to 1.88"). That is exactly how an exception belongs written. What
is missing is only the occasion to look at it regularly.

**Design — a `schedule` job, not a bot.**

```yaml
on:
  push:
    branches: [main]
  pull_request:
  schedule:
    - cron: '0 6 * * 1'     # Mondays 06:00 UTC
```

plus in the `deny` job `issues: write` and a step that on failure creates **one** issue —
or updates an existing one, instead of generating a new one every week.

**Why no Dependabot and no Renovate.** Both create pull requests. In a one-person project,
a flood of PRs that nobody merges is worse than no bot: after three months there are forty
open PRs, and the one security-relevant among them gets lost — the report becomes noise,
and noise gets ignored. Add the supply-chain side: a bot with write access in the repo is
another way for code to come in that no human wrote. For this project the order holds:
**first the report, then maybe the automation.**

If later it is to be Dependabot after all, then like this:
`open-pull-requests-limit: 0` (security updates only, no version upkeep), `groups` for the
rest, `interval: monthly`. That is the compromise that does not litter. But only after
that.

**The triage flow**, three sentences for OPERATIONS.md:

1. The issue comes in. First question: **is the vulnerable path reachable from here?**
   That is answered as with the `time` exception — not "is the crate in the tree" but "is
   the affected function called".
2. Reachable → fix it, if need be by replacing the dependency. Not reachable or not
   fixable → an entry in `deny.toml` with a rationale **and a reversal condition**.
3. Every exception is read along with the next weekly failure. An exception without a
   reversal condition is not an exception but a capitulation.

**Steps.**

```
1. schedule trigger in ci.yml → verify: after the first Monday, Actions shows a run with no
   associated commit
2. Issue step on failure of the deny job → verify: a test run with an artificially inserted
   vulnerable version creates exactly one issue; a second run creates no second one
3. Write down the triage flow → verify: the three steps stand linked next to the existing
   time exception
```

**Effort:** S, one hour. **Priority: high** — the best effect per line in the whole list.

---

## 7. Config versioning

> At the first breaking change (renaming a key) an old installation breaks with no
> migration path.

**Finding.** [config.rs](../crates/alpendns/src/config.rs) has **no** `version` field and
uses `serde(alias)` in **no** place. When a key is renamed, the operator today gets
exactly what serde says: `unknown field 'x'` — and thanks to `deny_unknown_fields` the
start fails, which is right (B.1 rule 5), but does not reveal what to do.

**The item mixes three things.** They have to be separated, otherwise you build the wrong
one:

**(a) A better error message.** On an unknown key, suggest the most similar known one
("unbekannter Schlüssel `blocklists` — meintest du `blocklist`?"). A Levenshtein
comparison against the field list, which serde supplies in the error anyway. Solves by
far the most common case: the typo. **Effort: S.**

**(b) Renaming without breakage: `serde(alias)`.** The old name stays as an alias for one
version and produces a `tracing::warn!` line with the new name when loading. `alias` and
`deny_unknown_fields` do **not** exclude each other — an alias is a known name, not an
unknown one. That is the 90 % solution of the actual TODO, and it costs one line per
rename. **Effort: S per case, zero in advance.**

**(c) A `version` field.** Sounds like the solution, but is the weakest of the three: it
tells the server what it notices anyway, and asks the operator to maintain a number he
does not understand. It would only be useful with a real migration behind it — and that
is blocked here: `/etc/alpendns/alpendns.toml` is a **Debian conffile**. A program that
rewrites a conffile by itself breaks the conffile contract; on the next upgrade `dpkg`
asks about local changes that the operator never made. Automatic migration at startup is
therefore **ruled out**, not merely unattractive.

**Recommendation: build (a) and (b), not (c).** If a big conversion does come later, the
right way is an explicit command `alpendns migrate --in <old> --out <new>` that writes to
`stdout` or into a named file and does not touch the conffile — the operator copies it
himself. Then the contract holds and the migration is visible.

**Steps.**

```
1. Field suggestion on an unknown key → verify: a test with "blocklists" instead of
   "blocklist" names the right name in the error text
2. Record the pattern for renames (serde(alias) + warning), with an example
   → verify: a test loads a config with the old name, the result is identical, the warning
   is logged
3. The decision against a version field and automatic migration as an ADR → verify: the ADR
   is in docs/adr/ and names the conffile contract as the reason
```

**Effort:** S, one evening for (a)+(b)+ADR. **Priority: medium** — (a) helps immediately,
(b) is provision that costs nothing until it is needed.

---

## 8. Block page

> When a website is on a blocklist → a nice block page from the DNS server ("Diese Seite
> wurde blockiert weil…")

**Finding.** The mechanism is partly there already:
[filter/block.rs:26-44](../crates/alpendns/src/filter/block.rs#L26-L44) knows three modes
— `Nxdomain` (default), `ZeroIp` and **`Sinkhole`**, which already returns a configured
IPv4 and IPv6 address. For a block page "only" the HTTP server on that address is missing.

**And now the unpleasant side, which has to be settled before building.** A block page
only works over **HTTP**. Over HTTPS — that is, on practically every request a human makes
— the user sees not a page but a **certificate warning**: the sinkhole cannot present a
valid certificate for `www.beispiel.de`. On a domain with HSTS (all the big ones) the
browser does not even offer a "continue anyway" but aborts hard. The result is therefore
not "a nice page instead of an error" but **"a certificate error instead of a name
error"** — for the layman the worse of the two messages, because it looks like an attack.

Three further points that are real:

* **Non-browser clients** — apps, update services, telemetry — are the majority of blocked
  requests. They see no page but hang in the TCP connection setup to the sinkhole and run
  into timeouts instead of failing immediately. `Nxdomain` is the friendlier answer for
  them.
* **Interaction with the rebinding detector**
  ([detect/rebinding.rs](../crates/alpendns/src/detect/rebinding.rs)): the sinkhole is by
  definition a private address as the answer to a public name — exactly the pattern the
  detector reports. Both active at the same time means: every blocked domain produces a
  finding, and the findings list becomes unreadable. That has to be exempted.
* **Getting the reason onto the page** touches B.1 rule 3. From the browser the HTTP
  server gets the `Host:` header, that is, the name — it would have to ask the policy
  again ("why would this name be blocked?"). That works without any storage via the
  existing `explain` path ([api/mod.rs:386](../crates/alpendns/src/api/mod.rs#L386)),
  which already answers exactly this question: no new storage, no linking of name and
  client. Important: the page needs `no-store`, otherwise the browser keeps it beyond the
  end of the block.

**Recommendation: build the cheaper half, not the expensive one.** What the user really
wants is not the page but the **answer to "why does this not work?"** — and that already
exists via `explain`. Only the way there is cumbersome. Proposal:

* A search field in the interface into which one types a name and gets the reason ("blocked
  by list X, rule Y"), next to it the existing "Allow" button. The `explain` endpoint
  can already do that; the way into the UI is missing.
* If the block page is wanted anyway, then **explicitly as an option for HTTP only**, with
  a sentence in the docs that names the certificate warning instead of keeping quiet about
  it. A certificate from an own CA rolled out on all devices solves it technically — and
  is an imposition in a household.

**Steps (for the recommended half).**

```
1. Search field in the UI that queries /api/explain → verify: entering a blocked name shows
   the list and the rule; an unknown name shows "not blocked"
2. Empty state of the field per B.6 (no empty box but a sentence) → verify: ui.rs rule tests
   green, then a human look
3. Rebinding exemption for the configured sinkhole address → verify: a test with an active
   sinkhole and an active rebinding detector produces no finding
```

**Effort:** search field S. Full block page M–L plus an ADR, with doubtful yield.
**Priority: low** — and that is a deliberate recommendation against the original wish, not
forgetfulness. **B.8 decision for the author.**

---

## 9. Configuration wizard in the web UI

**Finding.** The API today is read-only plus four writing endpoints for time-limited
entries (`/api/allow` and `/api/deny`, each POST and DELETE,
[api/mod.rs:110-131](../crates/alpendns/src/api/mod.rs#L110-L131)), secured by a bearer
token, compared in constant time. **It cannot write configuration.**

Three obstacles, all real:

1. **`ProtectSystem=strict`** makes the whole file system read-only except the
   `*Directory=` paths. `/etc/alpendns/` is **not** among them. A writing wizard therefore
   demands `ReadWritePaths=/etc/alpendns` — a softening of the hardening that belongs
   justified and accepted per B.5.
2. **No reload** (finding B1). Even a written config only takes effect after `systemctl
   restart` — and the service cannot trigger that itself without getting a way to systemd
   that `SystemCallFilter` and `CapabilityBoundingSet` precisely prevent. A wizard that
   writes and then says "please restart now" is half a wizard.
3. **conffile.** The same trap as in item 7.

**Design — a wizard that does not write.** That sounds like retreat and is the better
solution: the wizard leads through the questions (listener addresses, upstreams,
blocklists, first policy) and from that generates **a TOML preview** with comments, to
copy. Next to it stand the three commands:

```
sudo tee /etc/alpendns/alpendns.toml     # paste the content
sudo alpendns -c /etc/alpendns/alpendns.toml check
sudo systemctl restart alpendns
```

That bypasses all three obstacles, and the operator sees what he is installing — with a
file whose content decides whether his network is filtered, that is not an inconvenience
but the point. The **validation** still runs server-side: an endpoint
`POST /api/config/validate` takes the TOML text, parses it through the same `Config`
structure and the same blueprint checks as `alpendns check`, and answers with errors
including the line number — without storing anything. That way the preview is
demonstrably valid before anyone pastes it.

Design per B.6: the page does not scroll, so a multi-step wizard is **one view with a step
indicator**, not a long form — the question on the left, the growing preview on the right.
One container, no new one.

**If writing is to happen after all**, then in this order and not otherwise:
`ReadWritePaths=/etc/alpendns` into the unit (with a rationale in OPERATIONS.md §5),
writes atomically via a temporary file plus `rename`, beforehand a copy to
`alpendns.toml.bak-<timestamp>` in the StateDirectory, validation **before** the write,
and the conffile question decided explicitly (for instance: take the file out of the
conffile set and instead create it in `postinst` when it is missing).

**Steps.**

```
1. POST /api/config/validate, parses and discards → verify: valid TOML → 200 with a
   summary; unknown key → 400 with the key name and line; nothing on disk
2. A ceiling on the endpoint's body size → verify: a 10 MB body is rejected, no panic
3. Wizard view with step indicator and growing preview → verify: ui.rs rule tests green; the
   page does not scroll at 1400 px width
4. Copy button and the three commands below it → verify: a human look
5. [only if writing is wanted] B.8 decision on ReadWritePaths and conffile → verify: the
   author has decided; without a decision nothing is built
```

**Effort:** M, two to three evenings for the write-free version. **Priority: low** — it is
a convenience feature for the first installation, that is, for an event that happens once
per operator.

---

## 10. Resilience with two instances

> Resilience stands as a phase 10 option (two instances).
> [ROADMAP.md](ROADMAP.md): "Two instances with a synchronized policy state."

The largest item on the list, hence more thorough — and unrolled from the back: first the
question of which failures exist at all, then what of them a second instance solves, and
only then the how.

### 10.1 Which failures are there, and what already catches them today?

| Failure | Caught today by | Does a second instance help? |
|---|---|---|
| Process crash | `Restart=on-failure`, `RestartSec=2s` | **No.** The service is back in ~2 s. No stub resolver switches over in 2 s. |
| Crash loop (5×/300 s) | `StartLimitBurst=5` → the service stays down | **Yes**, but only on a second host. |
| Broken config after a change | `ExecStartPre=alpendns check` — the old process keeps running | Partly: only when the change is made **one after the other**. |
| Broken blocklist update | disk cache, `Origin::StaleCache` | **No.** Both instances load the same URL. What helps against that is diff review (roadmap O3), not redundancy. |
| Upstream dead | pool with several upstreams, `serve_stale` ([cache.rs](../crates/alpendns/src/cache.rs)) | **No**, unless the pools are deliberately configured differently. |
| Package upgrade | — | **Yes.** The case with the best ratio: upgrade one after the other, both are never down. |
| Host reboot | — | **Yes**, second host. |
| Hardware / power | — | **Yes**, second host — and only with separate power. Two boxes on the same power strip are one box. |
| Network partition | — | Depends on where. Only with a second segment. |

**The honest balance:** of nine kinds of failure, a second instance catches three, and all
three presuppose a **second host**. A second instance on the same machine is nearly
worthless — the most common failure (a crash) is fixed by systemd faster than a client
switches over. **Whoever builds this builds a second machine, not a second process.** That
is the most important sentence in this section.

### 10.2 The place where it usually fails: the client side

Two servers are of no use if the clients do not switch over. That is the part that is not
in the code and that has to be measured **before** building:

* **glibc** (`resolv.conf`): tries the servers in order, default `timeout:5`, `attempts:2`,
  and **does not remember the failure without `options rotate`**. The failure of server 1
  then means: every request waits five seconds before server 2 gets its turn. That is not
  "unnoticed", it is "it works, but everything hangs".
* **systemd-resolved** sticks to the working server after a few seconds — distinctly
  better.
* **Windows** knows a preferred and an alternate server with a failure memory in minutes.
  **Android** fails over most inconspicuously with short timeouts.
* **The most common case in the homelab is none of these:** all devices ask the router,
  the router forwards to AlpenDNS. Then only the failover **of the router** counts — and
  many consumer routers either carry only one forwarder, or they fall back to the provider
  DNS on failure. In the second case the consequence of the failure is **unfiltered
  plaintext DNS**, and the second server is never asked. That is worse than a visible
  failure.
* A device with an existing DHCP lease learns of the second address only at the next renew
  — possibly not for hours.

**That is why step one is not a line of code but a measurement.** Without it you do not
know whether you are solving a problem or adding one.

### 10.3 What would have to be synchronized — and what explicitly not

| State | Where | Sync? |
|---|---|---|
| Configuration, policies, clients | file, `Config::load` | **No — keep them equal by hand.** A sync channel that carries config distributes a broken config to both instances and turns two failure zones into one. That contradicts the purpose (B.1 rules 5 and 6). |
| Blocklists | separate download per instance | No. Same content, loaded independently. |
| Answer cache | RAM | No. Divergence has no consequences. |
| Log, ring buffer, counters, 24 h history | RAM, [history.rs:10-14](../crates/alpendns/src/history.rs#L10-L14) | No — and **not in principle**: the module header explicitly names the volatility as an assurance, not a defect. Consequence: each UI shows half the traffic. That belongs written into the UI. |
| Rate limit buckets | RAM, per instance | No. But: two instances effectively double the budget per client. `per_client_qps` then belongs halved on both — and nothing in the code enforces that. |
| Tunneling window | RAM, per instance | **Cannot be.** It is traffic statistics over names, that is, exactly what per B.1 rule 3 should not go over the wire. Consequence: every detector sees half the traffic and becomes duller. **Redundancy costs detection quality here.** There is no good answer for that, only the honest mention. |
| NRD file | file from AlpenShield, [detect/nrd.rs](../crates/alpendns/src/detect/nrd.rs) | Not a sync problem but operational work: the file has to lie on both hosts. The detector learns nothing, it only reads — a freshly started instance decides identically. If the file is missing, it simply flags nothing there. |
| **Time-limited grants and blocks** | RAM, `Instant`, [temporary.rs](../crates/alpendns/src/policy/temporary.rs) | **The only candidate.** It is the only state that a human creates in the UI and that stands nowhere else. |

That makes the task sharp: "synchronized policy state" from the roadmap means in practice
**a single kind of data**, not "state replication".

### 10.4 Staged plan

Each stage brings a gain on its own, and after each one you can stop.

**Stage 0 — measure and document. No code.**

The second server is set up before anything is built, and then measured: switch off
instance A, determine with a stopwatch how long laptop, phone, TV and router take — or
whether they do it at all. The number belongs in [BENCHMARKS.md](BENCHMARKS.md); it is the
justification for everything further. If the measurement yields "the router falls back to
the provider DNS", then **that** is the problem that belongs solved, and not the state
sync.

```
1. Set up the second host, the same config by hand → verify: dig against both delivers the
   same answer for a blocked and an allowed name
2. Hand out both addresses via DHCP → verify: resolv.conf/ipconfig on three devices shows
   both
3. Switch off instance A, measure the switchover time per device type → verify: numbers in
   BENCHMARKS.md, including the case "router falls back to provider DNS"
4. Section "Two instances" in OPERATIONS.md: order during upgrade, halve per_client_qps, NRD
   file on both hosts → verify: someone sets up a second system from it without asking
```

**Gain:** reboot, upgrade, hardware failure — the three cases from 10.1. **Effort:** one
evening, not a line of code. For most homelabs the item ends here, and that is a good
result.

**Stage 1 — notice that the instances drift apart.**

The silent failure of stage 0 is a second instance that has been running with an outdated
configuration for four months without anyone noticing. A **checksum** is enough against
that: `/api/status` and the UI name a hash over the loaded configuration (config file plus
list summary), and the operator compares it between the two instances — in the simplest
case by eye.

```
1. Hash over the loaded config in /api/status → verify: same file → same hash; a changed
   key → a different hash
2. Hash small in the header of the interface → verify: ui.rs rule tests green
```

**Gain:** the quiet configuration drift becomes visible. **Effort:** S, one hour. The best
yield per line in the whole of item 10.

**Stage 2 — grants on both instances, without a server change.**

The concrete annoyance from 10.3 is: the user grants a page, it works on the laptop and
not on the phone — because the phone happens to be asking instance B. The cheapest honest
solution moves consistency into the interface instead of the server: **the web UI knows
the address of the second instance and sends every write to both.** One field in the
configuration, two `fetch` calls instead of one, and a response when only one of them
worked.

That costs no new server interface, no protocol, no conflict resolution, and it has an
honest failure mode: if one of the two fails, **the interface says so** instead of
diverging silently. Drawback: it only works when someone uses the UI; an instance that
restarts in between catches up on nothing.

```
1. Optional second API address in the UI configuration → verify: without the value
   everything behaves as before
2. Writes (allow/deny, setting and revoking) to both → verify: test with two loopback
   instances; after the click the entry stands on both
3. Partial failure is displayed → verify: second instance unreachable → the interface says
   on which one the entry is missing instead of reporting success
```

**Gain:** the concrete user annoyance is gone. **Effort:** S–M, one evening, almost all of
it in `web/app.js`.

**Stage 3 — real peer sync of the time-limited entries.**

Only when stage 2 demonstrably does not suffice (an instance restarts and loses the state;
entries are also set outside the UI) does the built-in sync pay off. The design in brief:

* **No leader.** With two nodes there is no quorum, and a fixed leader role would lose
  write capability exactly when the leader fails — that is, in the only case for which the
  whole thing is built.
* **One endpoint**, `POST /api/peer/state`, behind the **existing** bearer auth
  ([api/mod.rs:145-186](../crates/alpendns/src/api/mod.rs#L145-L186)) and the same token
  (both instances get the same token file; it is on the backup list in OPERATIONS.md §3
  anyway). No second secret — a second secret is what you forget when setting things up
  again.
* **Push and pull are the same round:** the caller sends its complete state, the callee
  merges and answers with its state after the merge. A pure pull is a push with an empty
  list. What is transferred is the full state, not a delta — it is small (deadline at most
  `MAX_GRANT`), and a full picture knows no lost delta entry.
* **The `Instant` problem is bypassed, not solved:** what is transferred are **remaining
  lifetimes**, never points in time. The receiver computes `clock.now() + rest` — exactly
  what `grant()` already does today. An `Instant` never leaves the process, no wall clock
  is compared, and a moved clock on the peer has no consequences. The error is the
  transfer latency against a deadline of hours.
* **Revocation is a tombstone**, not a deletion, with the same remaining lifetime as the
  revoked entry. Without that, the next merge brings the revoked grant back. The merge is
  thereby commutative, idempotent and associative and needs neither a journal nor a
  catch-up protocol.
* **The channel carries domain names** — that is, exactly the data type that B.1 rule 3
  protects. Therefore: the peer address must be loopback, the way to the other host leads
  through a tunnel that the operator lays (WireGuard, SSH), and `alpendns check` rejects a
  non-loopback address. An API listening in cleartext in the LAN would be the wrong answer
  — via `/api/recent` it hands out names anyway.
* **B.1 rule 4 comes under pressure.** "The process contacts exactly three kinds of
  targets: configured upstream resolvers, configured blocklist URLs, and nothing else." A
  configured peer is a fourth kind. It is not telemetry — it goes to a target the operator
  has named himself and carries nothing outward. But the rule says "nothing else", and
  amending it is a **B.8 decision of the author, not an agent's resolution.** Without that
  acceptance, stage 3 is not built.
* **The endpoint processes network data**, so B.1 rule 1 applies in full: ceilings on body
  size and entry count, the same `parse_grant` check and the same `MAX_GRANT` capping as
  with the UI endpoint, saturating arithmetic, no slice indexing. The maximum damage of a
  peer that has been taken over: a name granted or blocked for at most `MAX_GRANT`. It
  cannot touch a config, a list or a detector — that is the reason the protocol
  deliberately has **no** fields for configuration.

Files touched: `policy/temporary.rs` (the entry goes from `Instant` to a small record with
a deadline, a tombstone flag and a counter; plus `export()` and `merge()`), `policy/mod.rs`
(passing through), `api/mod.rs` (route, DTOs, ceilings), a new `peer.rs` (task: `Notify` on
local change with debouncing, plus a ticker), `config.rs` (`[peer] url`), `main.rs`
(wiring, `check` conditions), `api/ui.rs` (reachability dot), `metrics.rs` (two counters,
no names). Not touched: `cache.rs`, `filter/*`, `ratelimit.rs`, `history.rs`, `logging/*`,
`detect/*` — they hold state that deliberately diverges.

**Effort:** L, three to four evenings, plus ADR-0021. **Priority: low** — the synchronized
state comprises perhaps a dozen entries per week in a household.

### 10.5 What stays weak about this undertaking

For completeness, because it belongs to the decision:

* The benefit is narrow, and the most common failure is untouched by it.
* The peer channel depends on a tunnel that AlpenDNS neither builds nor monitors. If it
  breaks, the state diverges silently; the dot in the UI mitigates that, it does not remove
  it.
* The shared token makes each instance a full reader of the other. Whoever takes over one
  box has both interfaces, including the names in the ring buffer.
* The merge writes into the same mutex that the request path reads via `check()`. With a
  few dozen entries that is measurably nothing, but it is exactly the kind of write load in
  the request path that B.3 rule 5 has in view.
* The heuristics become duller (10.3). There is no solution for that, only the mention.

### 10.6 Open decisions (B.8)

1. Will B.1 rule 4 be amended for the configured peer? Without that acceptance, stage 3
   falls away.
2. Will the reload from ARCHITECTURE.md §7 be built, or the paragraph withdrawn (finding
   B1)?
3. Is a non-loopback peer address a startup error or a warning? Recommendation: error.

---

## Order and bundles

**First, because cheap and with real effect:**

1. **Item 5** (one sentence in the UI, one paragraph in the module header) — one hour.
2. **Item 6** (`schedule` trigger) — one hour, the best effect per line.
3. **Item 1** (slowloris) — **done.** One evening, the only item that closes a gap.
4. **Item 4** (`After=time-sync.target` plus a runbook paragraph) — half an evening.

**After that, as needed:**

5. **Item 10, stages 0 and 1** — measure, document, config hash. The measurement decides
   whether anything is built further at all.
6. **Item 2** (log rotation) — as soon as someone switches on `mode = "full"`, not before.
7. **Item 7** (a and b: field suggestion and the `serde(alias)` pattern).
8. **Item 3** — and (b) before (a): the refresh task is the actual item, the age display is
   only honest afterwards.

**Last or not at all:** item 9 (wizard), item 8 (block page), item 10 stage 3.

**What is done together:**

* **Item 2 + finding B3:** the `BufWriter` belongs in the same move as the rotation.
* **Item 3(b) + item 10 stage 1:** both answer "is this instance still running with the
  right state?" — list age and config hash belong in the same header of the UI.
* **Item 7 + item 9:** both concern the way configuration gets into the system. The
  validation endpoint from item 9 is the same path as the field suggestion from item 7 —
  build once, use twice.
* **Item 8 + rebinding detector:** the sinkhole exemption is due even without a block page,
  as soon as someone uses `block_mode = "sinkhole"`. That is to be checked independently of
  the rest of item 8.

**What contradicts itself:** item 9 (the wizard writes config) and item 7 (`alpendns
migrate` writes nothing) both run into the Debian conffile contract. The decision "does
AlpenDNS ever write into `/etc/alpendns/` itself?" has to be made **once** and then holds
for both.

---

## The original list

For reference, unchanged:

- TCP slowloris — tcp.rs:86: timeout on the body read + a cap on concurrent connections. In a LAN one compromised device is enough to bring the resolver to its knees.
- Configuration wizard in the web UI
- Log rotation in full mode: logging/mod.rs writes append-only into a file that the process opens itself — systemd/journald does not reach it. Over months the query log grows without bound until the disk is full. Needs rotation (size/age)
- When a website is on a blocklist -> a nice block page from the DNS server (Diese Seite wurde blockiert weil...)
- Document the clock dependency: the project's own validation (ADR-0016) presupposes a correct host clock — a wrong clock makes every signed zone "bogus". For operations a sentence belongs in the runbook: NTP/systemd-timesyncd is a prerequisite, not optional.
- For blocklists, record "age" in the UI — last updated (2 days...) ago
- Implement config versioning: today the config is the specification (deny_unknown_fields). At the first breaking change (renaming a key) an old installation breaks with no migration path. Not a blocker for v1, but a version field + an explicit error message would be the investment that becomes expensive later if you did not make it early.
- Grants/denials are only in RAM (temporary.rs). A restart loses them. That is defensible (they are time-limited), but it belongs documented as a deliberate decision — otherwise someone wonders after the reboot why the grant is gone.
- Dependency updates: cargo deny runs, but who reacts to new RUSTSEC advisories? Set up Dependabot/Renovate so that the triage process for exceptions (like the documented time exception) is continuous instead of one-off.
- Resilience stands as a phase 10 option (two instances)
