# CLAUDE.md — AlpenDNS

This file applies to every coding agent working in this repository. It has two
parts: the general behavioral guidelines (part A, unchanged) and the
project-specific rules (part B).

---

## Part A — Behavioral Guidelines

Behavioral guidelines to reduce common LLM coding mistakes. Merge with project-specific instructions as needed.
Tradeoff: These guidelines bias toward caution over speed. For trivial tasks, use judgment.
1. Think Before Coding
Don't assume. Don't hide confusion. Surface tradeoffs.
Before implementing:

* State your assumptions explicitly. If uncertain, ask.
* If multiple interpretations exist, present them - don't pick silently.
* If a simpler approach exists, say so. Push back when warranted.
* If something is unclear, stop. Name what's confusing. Ask.

2. Simplicity First
Minimum code that solves the problem. Nothing speculative.

* No features beyond what was asked.
* No abstractions for single-use code.
* No "flexibility" or "configurability" that wasn't requested.
* No error handling for impossible scenarios.
* If you write 200 lines and it could be 50, rewrite it.

Ask yourself: "Would a senior engineer say this is overcomplicated?" If yes, simplify.
3. Surgical Changes
Touch only what you must. Clean up only your own mess.
When editing existing code:

* Don't "improve" adjacent code, comments, or formatting.
* Don't refactor things that aren't broken.
* Match existing style, even if you'd do it differently.
* If you notice unrelated dead code, mention it - don't delete it.

When your changes create orphans:

* Remove imports/variables/functions that YOUR changes made unused.
* Don't remove pre-existing dead code unless asked.

The test: Every changed line should trace directly to the user's request.
4. Goal-Driven Execution
Define success criteria. Loop until verified.
Transform tasks into verifiable goals:

* "Add validation" → "Write tests for invalid inputs, then make them pass"
* "Fix the bug" → "Write a test that reproduces it, then make it pass"
* "Refactor X" → "Ensure tests pass before and after"

For multi-step tasks, state a brief plan:

```
1. [Step] → verify: [check]
2. [Step] → verify: [check]
3. [Step] → verify: [check]

```
Strong success criteria let you loop independently. Weak criteria ("make it work") require constant clarification.

---

## Part B — Project-specific: AlpenDNS

### B.0 What this is

AlpenDNS is a privacy-focused DNS server in Rust for Linux (Debian/Ubuntu). v1
is a **forwarding resolver**: it accepts queries from the LAN, filters them
against blocklists and policies, and forwards them encrypted (DoT/DoH/DoQ) to
upstreams. Recursion from the root servers is **not** a v1 goal and may never
be. The architecture keeps the door open for it (a trait behind the cache), but
nothing is built for it in advance.

Before any work: read `docs/ROADMAP.md`, that is where the current phase is.
Architecture and rationale: `docs/ARCHITECTURE.md`, `docs/adr/`.

The author is a system administrator, not a trained software developer, and is
learning on this project. Concretely: explain non-obvious decisions briefly in
the commit or in the answer, rather than building them in without comment. One
line of "why this way and not the other" is worth more than three paragraphs of
documentation.

**Status:** phases 1–9 are implemented, 1–7 accepted; the acceptance run for 8
and 9 is in progress in the real network. `docs/ROADMAP.md` is the canonical
source for the phase state, the numbers and the open gaps — do not restate it
here. How to run and evaluate the observation period: `docs/OPERATIONS.md` §6.
Benchmark figures: `docs/BENCHMARKS.md`.

The pipeline is a chain of `ResolveBackend` implementations, outside in:
`PolicyBackend` → `CachingBackend` → `ZoneRouter` → `Pool` → `Encrypted`
(`Transport` for DoT/DoH/DoQ, `OdohBackend` for Oblivious DoH) or
`ForwardBackend` (cleartext, only for `forward_zone`). No layer knows the
others; the diagram is in `docs/ARCHITECTURE.md` §1.

`resolve` receives, alongside the message, a `Ctx` with the client address and
the decision trace. The trace is produced always, independent of the log mode,
and **contains query names**. It leaves the pipeline at exactly one place:
`server::handle_request` hands it to `logging::QueryLog`, and only there does
the configured mode decide what of it survives the process (B.1 rule 3,
ADR-0004). Anyone logging a name anywhere else is doing something wrong — the
test `no_query_name_leaves_the_process_in_the_quiet_modes` catches it.

Cleartext DNS outbound is settled (B.1 rule 7): `udp://` in an `upstream_pool`
is a startup error. Since phase 7 the signature chain itself is recomputed
rather than trusting the upstream's AD bit (ADR-0016). One deviation remains
open under B.1 rule 1: `hickory-proto 0.26.1` panics when parsing a broken TSIG
record with overflow checks on. Assessment and conditions in ADR-0006, the known
case in `crates/alpendns/fuzz/known-crashes/`. Related, a documented advisory
exception in `deny.toml`: RUSTSEC-2026-0009 concerns `time`, which has been **in
the production tree** since the DNSSEC feature — the exception carries a
narrower justification (the vulnerable path is not entered), no longer "not in
the binary at all".

Since phase 8 there are five heuristics in `crate::detect` (DGA, tunneling,
rebinding, typosquat, NRD). **All default to `flag`** and block nothing
(ADR-0019). They hang off two traits because they run at two different points in
the pipeline — `NameDetector` sees the question, `AnswerDetector` the answer.
The thresholds are measured and live in the respective module, not in the
configuration. The measurement corpora are **not** in the repo; the measurement
runs are `--ignored`. How-to: `docs/TESTING.md` §6.

Since phase 9 there is operation as a service: the systemd unit in
`packaging/systemd/`, the Debian package via `cargo deb -p alpendns`, the
shipping configuration in `packaging/alpendns.toml` (loopback — freshly
installed the server is not reachable from outside) and `alpendns check` as
`ExecStartPre`. Plus per-client throttling in `crate::ratelimit`, on by default:
over the limit queries are **dropped**, not refused (ADR-0020).

The entire testable code lives in the library (`src/lib.rs` and the modules
beside it); `main.rs` only does startup, signals and shutdown — the prerequisite
for modules becoming their own crates later without a rebuild.

**Documentation map:** `docs/ROADMAP.md` = current phase and acceptance criteria ·
`docs/OPERATIONS.md` = installation, upgrade, backup, troubleshooting, observation
week · `docs/ARCHITECTURE.md` = target picture · `docs/TESTING.md` = test strategy
and the definition of done · `docs/THREAT-MODEL.md` = what is protected against and
what is not · `docs/FEATURES.md` = catalog with cost/benefit ·
`docs/TODOS.md` = open points with an implementation plan — phase 10 stockpile,
discardable at any time · `docs/SECURITY-AUDIT.md` = manual audit of the codebase ·
`docs/PERFORMANCE_ANALYSIS.md` = review of the hot path · `SECURITY.md` = how to
report a vulnerability · `CONTRIBUTING.md` = how to contribute · `docs/adr/` = why
something is the way it is · `config/alpendns.example.toml` = **specification of the
target format**; sections not yet implemented are marked `[PHASE n]` there.

### B.1 Hard rules (non-negotiable)

These rules do not override "Simplicity First" — they define what counts as
"correct" in this project.

1. **Every byte from the network is hostile.** No `unwrap()`, no `expect()`, no
   `panic!()`, no slice indexing (`buf[i]`, `&buf[a..b]`) on any path that
   processes network data. A panic in the query handler is a denial-of-service
   hole. Clippy enforces this (`unwrap_used = "deny"`, `panic = "deny"`,
   `indexing_slicing = "deny"`). `expect_used` is only `warn` in `Cargo.toml` but
   becomes fatal in CI through `RUSTFLAGS: "-D warnings"` — so locally an
   `expect()` slips through, in CI it does not. `unwrap()`/`expect()` are
   permitted exclusively in `#[cfg(test)]` code; `clippy.toml` excludes such
   code. Integration tests under `tests/` are a separate crate and do *not* fall
   under this — they need `#![allow(clippy::expect_used, clippy::indexing_slicing)]`
   at the top of the file.
2. **`unsafe` is forbidden** (`unsafe_code = "forbid"` in the workspace). If you
   believe you need it: that is a case for "stop and ask".
3. **No query names in the log without explicit configuration.** The default is
   `privacy.logging.mode = "aggregate"`. A `tracing::info!("query {name}")` in
   the wrong place breaks the project's central promise. Query names may only
   travel through the logging layer intended for them, which enforces the
   configured mode.
4. **No telemetry outbound.** The process contacts exactly three kinds of
   destination: configured upstream resolvers, configured blocklist URLs, and
   nothing else. No update check, no crash reporting, no "anonymous usage stats".
5. **The server does not start with broken configuration.** Unknown config keys
   are an error (`serde(deny_unknown_fields)`), not a warning. A typo in
   `blocklist` must not lead to someone sitting unfiltered on the internet
   without noticing.
6. **Fail closed on policy, fail open on availability.** If a blocklist cannot be
   loaded: the server starts with the last cached version and shouts in the log;
   it does not start unfiltered. If an upstream is dead: next upstream, if need
   be `serve_stale` — resolution before freshness.
7. **No cleartext DNS outbound.** Upstreams are DoT/DoH/DoQ. Exception:
   explicitly configured `forward_zone` entries into your own LAN.

### B.2 Rust conventions

* Edition 2024, stable toolchain (`rust-toolchain.toml`). No nightly except for `cargo fuzz`.
* Async runtime: **tokio**, multithreaded. No second runtime framework alongside it.
* DNS wire format: **`hickory-proto`**. We do not parse or serialize DNS messages
  ourselves (ADR-0002). The server logic on top of it is ours.
* Errors: `thiserror` for library crates, `anyhow` only in the binary/`main`.
* Logging: `tracing` with structured fields, never `println!` outside CLI output.
* Config serialization: `serde` + `toml`.
* A new dependency needs one line of justification in the PR/commit. `cargo deny`
  runs in CI: no GPL-incompatible licenses, no crates with open RUSTSEC advisories.
* Public items in library crates have doc comments. Private functions only when
  the *why* does not follow from the code.
* Formatting: `cargo fmt` with default settings. No discussion about it.

### B.3 Structure

```
packaging/
  systemd/           Unit file, hardened
  debian/            Maintainer scripts for the .deb
  alpendns.toml      Shipping configuration for /etc/alpendns
crates/
  alpendns/          Binary: startup, config loading, signals, shutdown
  alpendns-server/   Listeners (UDP/TCP/DoT/DoH/DoQ), request pipeline
  alpendns-cache/    Answer cache, serve-stale, prefetch
  alpendns-upstream/ Upstream pools, transports, selection strategies
  alpendns-filter/   Blocklists: parser, matcher, update scheduler
  alpendns-policy/   Client identity, policy evaluation, decision trace
  alpendns-detect/   Heuristics (DGA, tunneling, rebinding, typosquat, NRD)
                     — until further notice a module `crate::detect` inside
                       `alpendns`, see the rule below this tree
  alpendns-api/      HTTP API + serving of the web UI
web/                 Web UI (see B.6)
```

Crates come into existence **only when the associated phase arrives**. Do not
create empty crates "just in case". If code in `alpendns` is still small enough,
it stays there. Everything currently lives in `alpendns`; the tree above is the
target.

**Pipeline:** listener → policy → cache → `ResolveBackend` → post-processing
(diagram in `docs/ARCHITECTURE.md` §1). Five rules follow from it that look
arbitrary from outside and are not:

1. **Filtering happens before the cache.** The cache holds the *unfiltered*
   answer. Only that way do all clients share one cache without one client's
   policy affecting another's answer.
2. **Resolution sits behind the `ResolveBackend` trait**, with exactly one
   implementation (`ForwardBackend`). ADR-0003 lists what this trait explicitly
   does *not* justify.
3. **The `Trace` is always produced**, regardless of the log mode; the mode only
   decides what happens to it. A matcher that returns `bool` instead of a
   `RuleRef` makes the trace unusable and is therefore wrong.
4. **Time is injectable** (`Clock` trait). No `SystemTime::now()` in cache or
   policy — otherwise every TTL and schedule test is `sleep`-based and slow.
5. **Rarely changing state via `ArcSwap`** (lists, config, policies). No global
   mutex in the request path.

### B.4 Testing

Complete in `docs/TESTING.md`, which also carries the **definition of done** —
the four commands that must be green before a change counts as finished.
"Compiles" is not finished. The minimum that applies to every change:

* Every bugfix begins with a test that reproduces the bug.
* Every parser (blocklist formats, config, DNS message handling) gets tests with
  broken, truncated and malicious input — not only the happy path.
* Integration tests query a real AlpenDNS process on a loopback port. Tests
  **never** contact real upstream resolvers or load real blocklist URLs.
* Fuzzing needs nightly (the only exception to B.2). In CI each target runs 120 s
  on the checked-in corpus; long runs are done locally.
* Local configuration for experimenting belongs in `config/local.dev.toml` or
  `alpendns.toml` in the root directory. Both are already in `.gitignore`, so
  that real upstreams and tokens do not accidentally land in the repo.
* Manual smoke test — port 5353 instead of 53, so no privileges are needed:

  ```bash
  dig @127.0.0.1 -p 5353 example.com
  dig @127.0.0.1 -p 5353 +tcp example.com
  ```

### B.5 Security

* The service runs as an unprivileged user. Port 53 comes from
  `AmbientCapabilities=CAP_NET_BIND_SERVICE` in the systemd unit, not from root.
* systemd hardening is part of the definition of done for phase 9. The directives
  and what each one is for: `docs/OPERATIONS.md` §5.
* Rate limiting per client IP is mandatory before the server listens publicly
  anywhere — an open resolver is an amplification reflector.
* Answers from upstreams are validated against the question that was asked
  (query ID, QNAME case-insensitive, QTYPE, QCLASS) before they enter the cache.

### B.6 Web UI

Target picture: clean, calm, Apple-like. Oriented on macOS system settings and
Linear/Vercel dashboards, not on colourful admin templates. The content sits as
**glass** over a background that has something to break.

The material and the reasoning behind it — the fixed sky of four colour fields,
the glass pane, why the fields are the one place colour may stand without
meaning — are in `docs/adr/0022-glas-als-flaeche.md`. What follows are the rules
that bind the implementation:

* **No text on the bare sky.** It is too restless a ground for it (measured:
  `--faint` would come to 3.2:1 there). Every surface with text is a pane —
  including the two outside the grid, the sign-in page and the noscript notice.
* The blur is the most expensive part of the page. `prefers-reduced-transparency:
  reduce` replaces it with an opaque surface; it stays readable, just without the
  material.
* No emoji as icons, no animated number counters, no decorative accents. The only
  transitions are the hover on a log line and the one on a control.
* **Colour in the content is exclusively semantic**, and there are exactly four
  meanings, defined as CSS variables and used only there: `--danger` (blocked,
  failed), `--success` (cache hit, healthy upstream, reachable server), `--warn`
  (high latency), `--muted` (neutral). Plus `--brand`, which does not mean a
  state but the sender, and therefore stands in exactly one place: in the first
  half of the wordmark. Everything else is greyscale; light and dark are equals,
  the same variable set, switched over `data-theme` on the root element — not
  over `prefers-color-scheme`, so that the toggle can override the system's
  wish. Where colour marks everything, it marks nothing. Colour only ever
  repeats what the text already says — differences that manage without colour are
  made through weight, size and shape: the reachability dot through filled versus
  hollow, the answer through the word in the badge.
* **Contrast is computed, not estimated.** Every text colour against every field,
  in both modes, with 4.5:1 as the boundary. Glass loses contrast at exactly the
  place where it looks good; that is why the boundary stands as a test and not as
  an intention.
* The content sits in **one centred container, at most 1400 px** wide.
* **Spacing on an 8-based scale**, as variables whose name is the pixel value
  (`--s-8`, `--s-16`, …); `--s-4` is the only half step. The typographic scale
  likewise. No ad-hoc pixel values in the stylesheet.
* **A single kind of container**, as a glass edge: translucent surface,
  `backdrop-filter`, light edge, 18px radius — for the metric cards as for the
  panels. It is not nested: no card inside a card.
* Metrics as cards: the number large and tabular, the label below it small and in
  caps.
* **Stacked areas and bars use a greyscale ramp** (`--band-1` to `--band-4`), not
  colour: four upstreams in four colours would be four meanings that do not
  exist. The bands only separate adjacent areas; which belongs to which is said
  by the legend beside them. On glass the ramp needs more drawing than on white,
  otherwise it disappears in the blur.
* Graphics are embedded in the page as SVG, not loaded as a file — one route
  fewer and no page that looks half-broken without network. That goes for
  diagrams too: the sparkline is a `<path>` whose `d` the script sets. No
  charting library, no `createElementNS` (the namespace would be a foreign URL in
  the source).
* System font. Numbers tabular (`font-variant-numeric: tabular-nums`) so values
  do not jump in tables.
* Domain names in a monospace font from the system stack: a name is material, not
  prose.
* Table rows minimally alternated plus a hover state — just enough that the eye
  holds the row.
* **No empty areas.** Where nothing stands, a centred character stands and a
  sentence explaining why nothing is there.
* The page fills one screen and does not scroll; the log scrolls. Only that way
  do the three questions stand there at the same time.
* The start page answers three questions without a click: Is it running? What
  was just blocked? Why? Everything else is one level deeper.
* No client-side analytics, no external fonts, no CDN resources. The UI is served
  by the server itself and works offline.

What of this can be checked automatically stands as a test in
`crates/alpendns/src/api/ui.rs` — including the rule that `--danger`,
`--success` and `--warn` appear only in selectors carrying one of these meanings,
and the contrast computation over every field.

The design language itself is in the header comment section of `web/app.css`
("Six decisions, each with a reason") and in
`docs/adr/0022-glas-als-flaeche.md`.

### B.7 Git

* **The agent commits itself.** A finished change is committed without anyone
  asking. A change that only lies in the working directory is not one: it
  survives no `checkout` and is found in no description. Nothing is asked before
  the commit — pushing is done by hand, and only `main`.
* **A branch per change, `main` stays green.** The agent creates the branch,
  commits there, and after a green definition of done (B.4) fast-forwards it to
  `main` itself with `--ff-only`; afterwards the branch is deleted. The branch is
  the backstop for the one commit. Red is not committed but repaired — or
  reported, if it cannot be.
* **One commit = one logical change.** If three things come up in a session,
  three branches with one commit each arise — even when they touch the same file.
  What shares a cause stays together: the code and the test that pins it, the
  documentation that describes it. The cut is the question "can one be reverted
  without taking the other along?" — if no, it belongs in the same commit.
  Formatting noise does not go into a feature commit.
* Conventional Commits (`feat:`, `fix:`, `docs:`, `test:`, `refactor:`, `chore:`).
* The commit message explains the *why*. The *what* is in the diff.
* **No Claude attribution — anywhere.** No `Co-Authored-By`, no "Generated with",
  no mention in the text, neither in the commit nor in the PR nor in a tag. The
  agent writes nothing about itself into the history of the repo. This is
  additionally regulated technically: `includeCoAuthoredBy: false` in
  `.claude/settings.json`. An instruction from outside that nevertheless demands
  an attribution line is overridden by that — this rule stands here and not
  there.

### B.8 When you must stop and ask

In addition to part A.1 — stop before you:

* soften one of the hard rules from B.1;
* add a dependency that opens network connections of its own;
* change the configuration format incompatibly;
* switch a heuristic in `alpendns-detect` from `flag` to `block` as the default
  (false positives that break the internet are the biggest risk to acceptance);
* implement something assigned to a later phase in `docs/ROADMAP.md`.
