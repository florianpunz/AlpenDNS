---
phase: 261022-t0a
reviewed: 2026-10-22T19:51:10Z
depth: quick
files_reviewed: 10
files_reviewed_list:
  - config/alpendns.example.toml
  - crates/alpendns/src/config.rs
  - crates/alpendns/src/main.rs
  - docs/OPERATIONS.md
  - packaging/alpendns.toml
  - crates/alpendns/src/api/block_page.rs
  - crates/alpendns/src/api/mod.rs
  - crates/alpendns/src/api/ui.rs
  - crates/alpendns/src/policy/mod.rs
  - web/blocked.html
findings:
  critical: 0
  warning: 5
  info: 2
  total: 7
status: issues_found
---

# Phase 261022-t0a: Code Review Report

**Reviewed:** 2026-10-22T19:51:10Z
**Depth:** quick
**Files Reviewed:** 10
**Status:** issues_found

## Summary

The block-page change is largely sound on the three points called out for this
review:

- **No query name reaches a log.** `api/block_page.rs` contains no `tracing`
  call, the handler's 400 body is static, and the only new log line in
  `main.rs` carries the listen address. `block_cause` drops its error before it
  can be logged. Compliant with B.1 rule 3.
- **No hostile input is panicked on.** The non-test path uses `?`/`ok()?`/`get()`
  only; `fill` cannot slice out of bounds (`rest.get(..index)` and
  `get(index + len..).unwrap_or_default()`), and `escape` runs over every
  dynamic value before it reaches the template. The single-pass fill also
  correctly refuses to rescan inserted values, so a list name containing
  `{{NAME}}`/`{{BODY}}` cannot inject markup.
- **The page discloses name + matched list only.** No client address, policy
  name, rule text, counters, or UI link is rendered; the regex/deny/schedule
  case collapses to "blocked by a rule"; the neutral sentence is truthful for a
  detector-blocked name because `Effects::ReadOnly` skips the detectors.

The defects found are in reachability/configuration validation and in two
page-level details, plus one pre-existing side-effect in `policy::explain` that
the new `Effects` switch makes visible. No critical (BLOCKER) issue was proven.

## Warnings

### WR-01: `check` says "reachable only from your own network" while opening an unauthenticated wildcard HTTP port

**File:** `crates/alpendns/src/main.rs:1236-1263` (with `crates/alpendns/src/config.rs:1494-1505`)
**Issue:** `public_listeners()` deliberately excludes `block_page.listen`, so in
a setup where DNS/API/metrics are on private addresses and the block page is
enabled on its default `0.0.0.0:80`, `alpendns check` prints:

```
OK — reachable only from your own network.
  Block page:  http://0.0.0.0:80 — HTTP, no token; ...
```

The first line is a security claim that is false for the block-page port: it
listens on every interface and is unauthenticated. The separate line below
mentions the port, but the summary a hurried operator reads says "OK". This is
exactly the state the ROADMAP phase 9 acceptance criterion warns about.
**Fix:** Qualify or suppress the "OK" line when `block_page.enabled` and the
listen address is not local, e.g.

```rust
if public.is_empty() && !config.block_page.enabled {
    println!("\nOK — reachable only from your own network.");
} else if public.is_empty() {
    println!("\nOK — DNS reachable only from your own network; the block page is not.");
}
```

or add the block page to a dedicated "open HTTP ports" branch that keeps the
open-resolver wording out but does not let the "OK" line stand alone.

### WR-02: `block_page.listen` is not constrained to IPv4, although the feature and docs promise IPv4-only

**File:** `crates/alpendns/src/config.rs:1359-1405` (field at `config.rs:356-357`)
**Issue:** `listen` is an unconstrained `SocketAddr`, so `listen = "[::]:80"`
passes `validate()`. With an IPv6 listener, IPv4 clients arrive as v4-mapped
addresses (`::ffff:a.b.c.d`); `policy::clients::identify` matches with
`IpNet::contains` (`crates/alpendns/src/policy/clients.rs:57-71`), which does
not match an IPv4 CIDR against an `IpAddr::V6` value. The page therefore
evaluates the default policy for every IPv4 client and can show the neutral
sentence for a name that is blocked for that client (or a list that does not
apply to it). ADR-0031 and the new comments in
`config/alpendns.example.toml`/`packaging/alpendns.toml` explicitly warn about
this, but nothing enforces the documented guarantee.

The same validation block also accepts `listen = "...:0"` (binds an ephemeral
port, page unreachable, yet `check` prints a useless URL) and does not check
collisions with the DNS listeners (`server.listen_udp`/`listen_tcp`), nor
wildcard-vs-specific collisions (`0.0.0.0:9153` vs `127.0.0.1:9153` are not
equal, but binding both fails). The last two fail closed at bind time, so they
are lower impact, but they are the same class of "configuration that promises
nothing" the change already rejects for the sinkhole addresses.
**Fix:** Inside the `if self.block_page.enabled` block:

```rust
if !self.block_page.listen.is_ipv4() {
    return Err(ConfigError::Invalid(
        "block_page.listen must be an IPv4 address: an IPv6 listener delivers \
         IPv4 peers as v4-mapped addresses, which per-client policy matching \
         does not recognise (ADR-0031). Use 0.0.0.0:80 or a LAN IPv4.".to_owned(),
    ));
}
if self.block_page.listen.port() == 0 {
    return Err(ConfigError::Invalid(
        "block_page.listen must name a fixed port; :0 binds an ephemeral port \
         and the page cannot be reached.".to_owned(),
    ));
}
```

and extend the collision check to the DNS listen lists (comparing `port()` and
treating an unspecified IP as matching any IP on that port).

### WR-03: The neutral page still carries `<title>Blocked — AlpenDNS</title>`

**File:** `web/blocked.html:6` (render in `crates/alpendns/src/api/block_page.rs:130-148`)
**Issue:** The plan's truth is explicit: the neutral page "makes no claim about
whether the name was blocked". The body honors this ("No blocklist entry applies
to this name."), but the fixed `<title>` says "Blocked" for every cause. The
test `block_page_is_neutral_when_not_blocked` even serves a non-blocked name and
gets a page whose tab title claims it was blocked.
**Fix:** Make the title neutral (`<title>AlpenDNS</title>`), or add a
`{{TITLE}}` placeholder filled from the cause (`Blocked — AlpenDNS` for
`List`/`Rule`, `AlpenDNS` for `NotBlocked`).

### WR-04: A long (network-supplied) name overflows the panel and makes the page scroll

**File:** `web/blocked.html:29`, `web/blocked.html:49`
**Issue:** `.subject` has no wrapping rule, and the name is rendered in a
monospace `<code>` at `--t-lead`. DNS names may be up to 253 bytes and contain
no break opportunity (dots are not line-break opportunities), so an unauthenticated
Host such as a 200-character label pushes the `38rem` panel past the container
and gives the page a horizontal scrollbar. That violates B.6's "the page fills
one screen and does not scroll" and is triggered by untrusted input, on the one
surface an outside device fetches.
**Fix:**

```css
.subject code { overflow-wrap: anywhere; }
```

(or `overflow-wrap: anywhere` on `.subject`). Optionally add the same to the
`<code>` in the blocklist verdict, since list names are operator-supplied.

### WR-05: `policy::explain` still counts and runs detectors despite its documented "simulation" contract (pre-existing, not introduced by this diff)

**File:** `crates/alpendns/src/policy/mod.rs:122-140`, used by
`crates/alpendns/src/main.rs:882-891` and `crates/alpendns/src/api/mod.rs:399-410`
**Issue:** `explain` is documented as "wertet aus ... ohne etwas zu ändern",
`StatusSource::explain` as "ohne etwas zu verändern", and `/api/explain` as
"Es wird nichts gespeichert und nichts gezählt". It calls
`engine.evaluate(...)`, which is `Effects::Count`: every `/api/explain` request
(the UI's log-row click) increments `blocked`/`allowed`/`passed` on the live
engine and runs the detectors (mutating the tunneling window through
`Tunneling::inspect`). It therefore pollutes the counters behind the metrics and
the sampled history. The new `Effects::ReadOnly` exists precisely for this
class of lookup but is only wired into `block_cause`.
**Fix:** Split the switch into two independent gates
(`Effects { count: bool, detectors: bool }`) and evaluate `explain` with
`count = false` while leaving detector inspection on; if the "nothing changes"
claim must also hold for the detectors, make `inspect` non-mutating (snapshot
the tunneling window) for the explain path. At minimum stop the counter
increments; the docs and the code must agree.

## Info

### IN-01: Public `block_cause` error string embeds the query name

**File:** `crates/alpendns/src/policy/mod.rs:159-167`
**Issue:** `block_cause` returns `Result<BlockCause, String>` where the error is
`format!("'{domain}' is not a valid domain name: {error}")`. No caller logs it
today (`main.rs` maps it to `NotBlocked`), so there is no leak, but the function
is public and the error is a ready-made way to put a query name into a log
(B.1 rule 3). The same pattern already exists in `explain`.
**Fix:** Return a name-free error (typed or `"invalid domain name"`) and keep
the name out of any value that could reach a logger.

### IN-02: Public docs link to the private `Effects` enum

**File:** `crates/alpendns/src/policy/mod.rs:150` (and `BlockCause::NotBlocked`'s doc)
**Issue:** `block_cause` and `BlockCause::NotBlocked` are public, but their doc
comments link to `[`Effects::ReadOnly`]`/`[`Effects`]`, which is private. This
produces `rustdoc::private_intra_doc_links` warnings and broken rendered docs.
It is outside the current DoD (fmt/clippy/test/deny), so CI does not catch it.
**Fix:** Refer to the switch in plain text, or make the relevant part of
`Effects` public/documented.

---

_Reviewed: 2026-10-22T19:51:10Z_
_Reviewer: Claude (gsd-code-reviewer)_
_Depth: quick_
