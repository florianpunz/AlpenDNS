# Quick Task 261022-t0a: "Blocked by AlpenDNS" page — Research

**Researched:** 2026-10-22
**Domain:** A second, open HTTP listener in an existing Rust/tokio/axum DNS service; read-only policy lookup; static page in the established design language
**Confidence:** HIGH for every integration point (read in the pinned source this session), MEDIUM for the two presentation/refactor choices flagged below

<user_constraints>
## User Constraints (from CONTEXT.md)

### Locked Decisions

#### Delivery
- **Own listener on port 80**, unauthenticated, serving only the block page.
  Not a route on the existing API listener: the API is token-protected and must
  not share a routing surface with an open endpoint.
- **HTTP only, no port 443.** No certificate generator, no key storage, no TLS
  code in this task. A browser that tries `https://` first gets a connection
  error rather than this page; that limitation is documented, not worked
  around.

#### Content
- The page shows the **blocked name, the fact that it was blocked locally on
  this network, and the name of the list that matched**.
- **No** client IP, policy name, rule text, allow/block controls, or link to the
  token-protected UI. Nothing that turns the page into a reconnaissance surface.
- The matched list name comes from the decision trace the server already
  produces; the page must not invent a second source of truth for "why".

### Claude's Discretion
- **The shipped default stays `block.mode = "nxdomain"`.** Switching it to
  `sinkhole` would change behaviour for every installation, and the sinkhole
  address is the server's own LAN address, which the configuration cannot know
  by itself. The block page is an opt-in: the operator sets the mode and the
  sinkhole address. `docs/OPERATIONS.md` gets the recipe.
- **The listener is off by default** (`enabled = false`). A fresh install opens
  no new port; enabling it is a deliberate act, like configuring the sinkhole.
- The `[block_page]` section carries `enabled`, `listen`, and nothing else
  unless the implementation genuinely needs it.
- How the page asset is embedded (a `web/blocked.html` + a shared stylesheet
  via `include_str!`, as `api/ui.rs` does today) and whether the theme follows
  `prefers-color-scheme` only or also offers a toggle.

### Specific Ideas
- The page is the only AlpenDNS surface an outside device will ever fetch.
  It should say plainly what happened and imply nothing about the rest of the
  installation.
- Language: English, like the Web UI and the documentation.
- One kind of container, glass edge, greyscale except where colour is semantic
  — the rules in CLAUDE.md B.6 apply to this page too, not only to the UI.
- No JavaScript is needed to show a static page; the theme switch can be pure
  CSS (`prefers-color-scheme`). Any script would have to justify itself.

### Deferred Ideas
CONTEXT.md contains no `## Deferred Ideas` section. The explicit exclusions are
the two locked Delivery decisions above: no TLS/443, and no second routing
surface on the API listener.
</user_constraints>

## Summary

The DNS half already exists: `block.mode = "sinkhole"` synthesizes the
configured address for a blocked name, and the browser does the rest
(`crates/alpendns/src/filter/block.rs:26-38`, `:79-120`). What is missing is an
HTTP answer on port 80. The HTTP half is smaller than it looks: axum 0.8.9 is
already in the tree and `main.rs` already starts two independent axum listeners
with the exact bind/spawn/graceful-shutdown pattern this task needs
(`crates/alpendns/src/main.rs:542-579`).

The real design question is not the listener but the *reason*. The page gets a
name from the `Host` header, not from a DNS query, and it needs the matched
list name. The obvious reuse — `alpendns::policy::explain` — is **not safe
here**: despite its doc comment saying it counts nothing, the evaluator it calls
increments the policy counters (`crates/alpendns/src/policy/mod.rs:347-355`) and
feeds the tunneling detector's per-zone window
(`crates/alpendns/src/detect/tunneling.rs:306-326`). An HTTP page view must not
distort DNS statistics or heuristic windows. The recommendation is therefore a
small, read-only variant of the same evaluation path (`evaluate_with(…,
Effects::ReadOnly)`) plus a `policy::block_cause` helper that reads the existing
typed `Step`/`deciding_step` trace instead of parsing the human-readable steps.

**Primary recommendation:** add a separate `api::block_page` router behind a
`BlockPageSource` trait; serve it via `axum::serve` on its own
`TcpListener` with `ConnectInfo<SocketAddr>` so the policy lookup uses the
requesting client's address; look the name up through a read-only policy path
that never runs detectors and never touches counters; render a server-side
`web/blocked.html` that links the existing `/app.css` (no duplicated palette),
sets `data-theme` from `matchMedia` in a three-line head script, and answers
with `Cache-Control: no-store`.

## Answers to the Eight Questions

### Q1 — HTTP stack, listener pattern, test pattern

**Stack.** `axum = "0.8"` in the workspace (`Cargo.toml`, workspace
dependencies under the comment "HTTP-API und Web-UI. Die Roadmap nennt axum
ausdrücklich."); the lock resolves **0.8.9**:

```
name = "axum"
version = "0.8.9"
```
`[VERIFIED: Cargo.lock:214-217]`

The runtime is tokio 1.53 with `rt-multi-thread`, `net`, `signal`, `time`
(`Cargo.toml`, workspace dependencies). No HTTP/TLS dependency needs to be
added.

**Listener pattern to copy verbatim.** `main.rs` builds the API listener as:

```rust
let listener = tokio::net::TcpListener::bind(api_config.listen)
    .await
    .with_context(|| format!("API listener on {}", api_config.listen))?;
tracing::info!(listen = %api_config.listen, token_file = %api_config.token_file.display(), "API and web UI");
let router = alpendns::api::router(state);
let signal = shutdown.clone();
tokio::spawn(async move {
    let _ = axum::serve(listener, router)
        .with_graceful_shutdown(async move { signal.cancelled().await })
        .await;
});
```
`[VERIFIED: crates/alpendns/src/main.rs:549-559]`

The metrics listener is the same shape at `main.rs:568-577`. A third listener is
a third copy of this block — no new abstraction is justified for three
instances.

**Peer address.** The block page needs the requesting client's IP to evaluate
*that client's* policy. axum provides `ConnectInfo` only when the service is
built with connect info:

```rust
fn into_make_service_with_connect_info<C>(self) -> IntoMakeServiceWithConnectInfo<Self, C>;
```
`[VERIFIED: axum-0.8.9/src/service_ext.rs:33]`; extractor defined in
`axum-0.8.9/src/extract/connect_info.rs:24-30`. So the block-page spawn must use
`axum::serve(listener, router.into_make_service_with_connect_info::<SocketAddr>())`
— not the bare `router` the API uses. Missing this yields a 500 from the
extractor, not a compile error; pin it with a test.

**`Host` extraction.** The pinned axum 0.8.9 has **no** `Host` extractor:
`src/extract/` contains no `host` module and `grep -rn "pub struct Host"` over
the crate source returns nothing. (`Host` lives in `axum-extra`.) Do not add a
dependency for one header; read it from `axum::http::HeaderMap`:

```rust
headers.get(axum::http::header::HOST).and_then(|v| v.to_str().ok())
```
`[VERIFIED: axum-0.8.9 source tree, src/extract/]`

**Test pattern.** The project's HTTP tests start a real listener on an
ephemeral port and use `reqwest`:

```rust
let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.expect("bind");
let addr: SocketAddr = listener.local_addr().expect("local_addr");
tokio::spawn(async move {
    let _ = axum::serve(listener, alpendns::api::router(state)).await;
});
```
`[VERIFIED: crates/alpendns/tests/observability.rs:441-447]`

`reqwest` is already a dev-dependency with the `json` feature
(`crates/alpendns/Cargo.toml`, `[dev-dependencies]`). Copy `start_api`'s helper
shape (`tests/observability.rs:418-476`) and replace the bearer-token helper
with one that sets `Host`. Do **not** bind port 80 in tests: CI and a normal
developer are unprivileged.

### Q2 — Getting the blocked name and the reason

**Name.** On plain HTTP there is no SNI; the name is the `Host` header
(HTTP/1.1 requires it). Normalize it exactly as the policy engine does:

```rust
let name = Name::from_str_relaxed(domain)
    .map_err(|error| format!("'{domain}' is not a valid domain name: {error}"))?;
```
`[VERIFIED: crates/alpendns/src/policy/mod.rs:97-98]`

The policy normalizer then lowercases and trims the root dot:
`name.to_ascii()`, `trim_end_matches('.')`, `to_ascii_lowercase()`
`[VERIFIED: crates/alpendns/src/policy/mod.rs:372-376]`. Strip an optional
numeric port (`example.com:8080`) before parsing; a bracketed IPv6 literal is
not a domain name and should yield the neutral/400 path.

**Reason — the function and its trap.** The existing function is:

```rust
pub fn explain<C: Clock + Clone, W: WallClock>(
    engine: &Engine<C, W>,
    domain: &str,
    peer: IpAddr,
) -> Result<Explanation, String>
```
`[VERIFIED: crates/alpendns/src/policy/mod.rs:92-96]`

It performs the same evaluation as the DNS path: `engine.evaluate(&name, peer,
&mut ctx)` (`policy/mod.rs:100`), and `Explanation` exposes only rendered
strings (`domain`, `client`, `blocked`, `steps: Vec<String>`,
`policy/mod.rs:78-85`). Using it would mean either parsing
`"Blocklist 'x' line 42: '…'"` back out of `steps` or changing `Explanation`.

**The trap.** Its doc says "sie zählt in keiner Statistik mit"
(`policy/mod.rs:89-91`), but the path it calls increments counters:
`record_block` does `self.counters.blocked.fetch_add(1, Ordering::Relaxed)`
`[VERIFIED: crates/alpendns/src/policy/mod.rs:351-355]`; allowlist/pass paths
increment `allowed`/`passed` (`policy/mod.rs:238`, `:261`, `:305`); and
`record_findings` calls `crate::detect::record` (`policy/mod.rs:701-704`),
which increments the process-wide detector counters
`[VERIFIED: crates/alpendns/src/detect/mod.rs:383-391]`. Worse for correctness:
the tunneling detector **mutates** its zone window on every `inspect`:

```rust
let mut zones = self
    .zones
    .lock()
    .unwrap_or_else(std::sync::PoisonError::into_inner);
...
let window = zones
    .entry(zone.to_owned())
    .or_insert_with(|| Window::new(now));
```
`[VERIFIED: crates/alpendns/src/detect/tunneling.rs:306-326]`

Every page view would therefore look like one more DNS query to the tunneling
heuristic. That is a behaviour bug, not a rounding error. **Do not call
`policy::explain` (and therefore do not call `Engine::evaluate`) from the HTTP
handler.**

**Smallest honest fix — one path, two effects.** Add a private
`evaluate_with(…, effects)` used by both callers:

- `evaluate` / `evaluate_typed` pass `Effects::Count` — current behaviour,
  byte for byte;
- the new `policy::block_cause` passes `Effects::ReadOnly` — skip the five
  counter updates and skip the detector block at `policy/mod.rs:297-303`.

Then read the typed trace, not the rendered strings. The helper already exists:

```rust
pub fn deciding_step(steps: &[Step]) -> Option<&Step>
```
`[VERIFIED: crates/alpendns/src/policy/mod.rs:695-708]`

`block_cause` is then ~20 lines:

```rust
pub enum BlockCause {
    NotBlocked,
    /// A blocklist matched; this is the list name the page may show.
    List(Arc<str>),
    /// Regex, schedule or temporary deny — blocked, but naming no list.
    Rule,
}

pub fn block_cause<C: Clock + Clone, W: WallClock>(
    engine: &Engine<C, W>,
    domain: &str,
    peer: IpAddr,
) -> Result<BlockCause, String> {
    let name = Name::from_str_relaxed(domain)
        .map_err(|error| format!("'{domain}' is not a valid domain name: {error}"))?;
    let mut ctx = Ctx::new(std::net::SocketAddr::new(peer, 0));
    if engine.evaluate_with(&name, RecordType::A, peer, &mut ctx, Effects::ReadOnly)
        != Decision::Block
    {
        return Ok(BlockCause::NotBlocked);
    }
    Ok(match deciding_step(ctx.steps()) {
        Some(Step::BlocklistHit { list, .. }) => BlockCause::List(Arc::clone(list)),
        _ => BlockCause::Rule,
    })
}
```
*Design sketch; the types and the `deciding_step`/`Step` shapes above are
verified, the `Effects` enum is to be written.*

The list name itself is already the right value in the trace:
`Step::BlocklistHit { list, line, matched }`
`[VERIFIED: crates/alpendns/src/trace.rs:66-70]`, built from
`matcher.list_name(hit.rule.list)`
`[VERIFIED: crates/alpendns/src/policy/mod.rs:270-274]`, and `list_name` indexes
the matcher's name table `[VERIFIED: crates/alpendns/src/filter/matcher.rs:122-124]`.

**If the planner refuses the hot-path change:** the fallback is a read-only
mirror that reproduces the precedence from `evaluate_typed` (temporary allow →
temporary deny → allowlist → blocklist → regex → schedule) without counters or
detectors. It is smaller in diff but is a second copy of the precedence and can
drift. If chosen, add the drift test described in Q7. Do **not** use
`Explanation.steps` string parsing in either case.

### Q3 — Echo-host abuse

The page renders the `Host` header. Two separate problems:

1. **Reflected markup.** Treat the `Host` value as hostile (B.1 rule 1). Parse
   it with hickory and render the ASCII form. `Name::to_ascii()` escapes every
   byte that is not `[A-Za-z0-9-_.*]` as `\x` / `\DDD`
   (`hickory-proto-0.26.1/src/rr/domain/label.rs:179-209`, `is_safe_ascii`
   `:225-235`), so its output cannot contain `<`, `>`, `&`, `"` or `'`.
   *Still* run the final string through a five-line HTML escaper and pin it with
   a hostile-input test: defense in depth, and it keeps the property true if the
   name is ever sourced differently. `to_ascii()`'s escape routine is a hickory
   implementation detail, not part of our contract.
2. **False accusation / oracle.** Anyone can request port 80 with
   `Host: some-unrelated.example`. The page must not answer "this name is
   blocked" without checking. Verify at request time with `block_cause`; the
   lookup is an `ArcSwap::load()` plus a `HashMap` suffix probe of at most the
   label count (`matcher.rs:126-154`), i.e. the same work as one DNS query, no
   lock. If the result is `NotBlocked`, render a neutral sentence ("this name is
   not on a blocklist on this network") and name nothing. The page reveals only
   name + list for a name the *requester's own address* is actually blocked
   from — no client IP, no policy name, no rule text, no counts.

**Client identity/policy.** Use the HTTP peer address, not loopback. The
default-policy fallback (`127.0.0.1`) would show the wrong list on any
installation with per-client policies — a core feature (`PolicySet::policy`,
`policy/mod.rs:57-63`; `Clients::identify`, `policy/clients.rs:57-72`).
`ConnectInfo<SocketAddr>` supplies it. The address is used for evaluation only;
it is never rendered or logged.

**Rate limiting.** None needed. The DNS throttling exists because UDP DNS is a
spoofable amplification reflector. This listener is TCP: the handshake cannot be
spoofed, so there is no reflection vector; the response is a small fixed page.
Document this rather than wiring `crate::ratelimit` into HTTP (which is
per-client and DNS-shaped).

### Q4 — Configuration

The existing pattern is a defaulted, strict section:

```rust
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ApiConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default = "default_api_listen")]
    pub listen: SocketAddr,
    #[serde(default = "default_token_file")]
    pub token_file: std::path::PathBuf,
}
```
`[VERIFIED: crates/alpendns/src/config.rs:299-312]`

Follow it exactly:

```rust
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BlockPageConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default = "default_block_page_listen")]
    pub listen: SocketAddr,
}

fn default_block_page_listen() -> SocketAddr {
    SocketAddr::from(([0, 0, 0, 0], 80))
}

impl Default for BlockPageConfig {
    fn default() -> Self {
        Self { enabled: false, listen: default_block_page_listen() }
    }
}
```

and in `Config`, `#[serde(default)] pub block_page: BlockPageConfig,` next to
`api`/`metrics` (`config.rs:83-85`). Because every field is defaulted and the
section is optional, this is additive: existing configs parse unchanged
(B.1 rule 5 still holds — unknown keys inside the section are errors).

**Defaults.** `enabled = false` (locked). `listen = 0.0.0.0:80` is the
recommendation: the sinkhole address must be the server's LAN address, so the
page must be reachable on the LAN; `0.0.0.0` is the same decision as ADR-0031's
DNS wildcard, for the same reason. Port 80 is the browser default for
`http://<sinkhole>/`.

**`Config::validate` additions** (the natural place is next to the
api/metrics collision check at `config.rs:1316-1323`):

1. `block_page.enabled && blocking.mode != BlockMode::Sinkhole` → `Invalid`.
   The page can never be reached in `nxdomain`/`zero_ip`, and the project treats
   a configuration that promises nothing as a startup error (B.1 rule 5). Message
   should name both keys.
2. Address collision with an enabled API or metrics listener — same error shape
   as the existing api/metrics check.
3. Optional but cheap: `block_page.enabled` and either sinkhole address
   `is_unspecified()` (`0.0.0.0` / `::`) → `Invalid`; that answer never reaches a
   page. The shipped defaults are loopback (`config.rs:573-580`), so the mode
   check is the one that catches the common mistake.

**Do not** add `block_page.listen` to `public_listeners()`
(`config.rs:1415-1425`). That list feeds `alpendns check`, whose text is
DNS-specific: "… is not restricted to a private address. If this port is
reachable from the internet, the server is an open resolver."
`[VERIFIED: crates/alpendns/src/main.rs:1195-1204]` Printing that for an HTTP
page is wrong. Add a separate `check` line instead ("Block page: http://… —
open, no token; it shows the blocked name and the matched list"). The existing
test `the_packaged_configuration_listens_on_every_interface` asserts exactly two
public listeners (`config.rs:2331-2345`); keeping the page out preserves it.

**Shipped files.**

- `config/alpendns.example.toml`: add a `[block_page]` section after `[metrics]`
  with `enabled = false`, `listen = "0.0.0.0:80"`, and the recipe in comments.
  The file's own rule is "Sections that are not implemented yet are marked
  [PHASE n]" (`config/alpendns.example.toml:5`); this section is implemented,
  so it gets no marker.
- `packaging/alpendns.toml`: add the same disabled section. It is the file the
  Debian package installs; the operator should see the option without reading
  the example. `the_packaged_configuration_is_valid` (`config.rs:2276-2282`)
  will exercise it.
- `config/alpendns.minimal.toml`: leave alone. It is deliberately minimal and is
  parsed by `the_shipped_minimal_config_still_parses` (`config.rs:2196-2213`);
  an absent section is valid.

### Q5 — systemd and packaging

Port 80 needs **no unit change**. `CAP_NET_BIND_SERVICE` is not port-specific:

```
CAP_NET_BIND_SERVICE
      Bind a socket to Internet domain privileged ports (port numbers
      less than 1024).
```
`[VERIFIED: man 7 capabilities]`

The unit already grants exactly that capability:

```
AmbientCapabilities=CAP_NET_BIND_SERVICE
CapabilityBoundingSet=CAP_NET_BIND_SERVICE
```
`[VERIFIED: packaging/systemd/alpendns.service:57-58]`

`NoNewPrivileges=yes` (`:59`), `RestrictAddressFamilies=AF_INET AF_INET6 AF_UNIX
AF_NETLINK` (`:102`) and `SystemCallFilter=@system-service` (`:96`) are all
compatible with `bind`/`listen`/`accept` on an INET socket. `ProtectSystem=strict`
(`:79`) is irrelevant: the page is embedded in the binary, like the UI
(`crates/alpendns/src/api/ui.rs:15-17`). `packaging/debian/*` contains no
firewall/port logic (only user creation, start/stop messaging); no change there.

### Q6 — Theming and stylesheet reuse

**The rule.** B.6 says light and dark are "switched over `data-theme` on the
root element — not over `prefers-color-scheme`, so that the toggle can override
the system's wish." ADR-0022 point 2 repeats why: the attribute is set by a
short head script before the first paint. The variable set is declared once in
`:root` and once in `:root[data-theme="dark"]`
(`web/app.css:57-137`, `:139-171`).

**What to do for a page with no toggle.** Keep `data-theme`. A three-line head
script, the same mechanism as `web/index.html:8-21`, minus the toggle and
`localStorage`:

```html
<script>
  document.documentElement.dataset.theme =
    matchMedia("(prefers-color-scheme: dark)").matches ? "dark" : "light";
</script>
```

This is **smaller and safer** than the pure-CSS alternative. A
`@media (prefers-color-scheme: dark) { :root { … } }` block would duplicate the
entire dark variable set (~30 declarations), creating a second place where every
colour lives; `light-dark()` would rewrite the token declarations and break the
existing tests that count each token exactly twice
(`crates/alpendns/src/api/ui.rs:92-110`). The script is a *justified* exception
to CONTEXT's "no JavaScript is needed": it is three lines, it uses the mandated
mechanism, and it degrades to the light theme (still fully readable) when
JavaScript is off. Document that one sentence in the page header comment.

**Reuse `app.css` as-is.** Serve the existing `/app.css` from the block
listener as well. It already supplies everything the page needs:

| Need | Provided by |
|---|---|
| Sky background | `.sky` + `--base`, `--field-1..4` (`app.css:196-206`) |
| Centred container | `.wrap` + `--page` (`app.css:217-222`) |
| The one glass container | `.panel, .card` + `--glass`, `--hairline`, `--edge`, `--shadow`, `--blur`, `--radius` (`app.css:347-354`) |
| Text colours | `--text`, `--faint`, `--muted`, `--danger` (`app.css:89-99`) |
| Brand wordmark | `.mark`, `.wordmark b`, `.logo` (`app.css:251-268`) |
| Monospace name | `code { font-family: var(--mono) }` (`app.css:954`) |
| Type/spacing scale | `--t-*`, `--s-*` (`app.css:115-136`) |
| Reduced transparency/motion | `app.css:962-976` |

Cost: ~995 lines / ~36 KB uncompressed for a page that loads once on a LAN.
Benefit: **zero duplication** of the palette, so the contrast test
`the_text_stays_readable_on_every_field` (`api/ui.rs:460-502`) keeps covering
this page's colours too. A dedicated `blocked.css` that re-declares the tokens
is the wrong trade in this repo: it creates a second source of truth for the
most sensitive values in the file. If a separate file is preferred anyway, it
must link `app.css` with a second `<link>`, not `@import url(…)` (the UI test
treats `@import url(` as a "foreign reference" needle, `api/ui.rs:65-74`).

Add only a few page-specific rules — centring a single panel on one screen:

```css
.block-page {
  min-height: 100dvh;
  display: grid;
  place-items: center;
  padding-block: var(--s-32);
}
.block-page .panel { max-width: 38rem; padding: var(--s-32); }
```

Put them in a small `<style>` block in `blocked.html` (single document, no extra
route) or in a `web/blocked.css` served next to it. Either way: no ad-hoc pixel
values, no new colour token, no second container inside the panel.

**Visual content.** Wordmark with the existing inline SVG paths
(`web/index.html:37-41`, B.6 "graphics embedded in the page"), a heading, the
name in `<code>`, one sentence of reason, one footer sentence. `--danger` is the
one semantic colour and it means "blocked" — allowed by B.6; everything else is
greyscale. No emoji, no animation, no link to the UI, no client/policy/rule
data.

### Q7 — Tests that pin the behaviour

The project already tests HTTP surfaces two ways, and this task should do both:

- **Router/real-port tests** in the style of `tests/observability.rs:418-476`
  (bind `127.0.0.1:0`, `axum::serve`, `reqwest`).
- **Static-asset assertions** in the style of `api/ui.rs:51-853` (assert on the
  embedded HTML/CSS strings).

Test map:

| # | Behaviour | Where | Command |
|---|---|---|---|
| 1 | Listener answers `GET /` with 200, `text/html; charset=utf-8`, `Cache-Control: no-store` | router test | `cargo test -p alpendns block_page` |
| 2 | The `Host` name appears in the body; the matched list appears when `Cause::List` | router test | same |
| 3 | `Cause::NotBlocked` never claims "blocked" and names no list | router test | same |
| 4 | `Cause::Rule` says "blocked on this network" and names no list/rule | router test | same |
| 5 | A missing/empty/invalid `Host` answers 400 and echoes nothing | router test | same |
| 6 | A numeric port is stripped (`ads.example.com:8080` → `ads.example.com`) | unit test on `host_name` | same |
| 7 | A hostile `Host` cannot become markup (`"><script>…`, `&lt;`) | unit test on `render`/`escape` | same |
| 8 | The page references nothing external (mirror `ui.rs:58-77`) and links only `/app.css` | static test | same |
| 9 | The theme follows the system: head script contains `prefers-color-scheme: dark` and `dataset.theme`; no theme toggle | static test | same |
| 10 | The page uses `.sky` + exactly one `.panel`, no nested container, no `innerHTML` | static test | same |
| 11 | `block_cause` returns `List("…")` for a blocklisted name, `NotBlocked` for an allowlisted one, `Rule` for a regex/schedule/deny | `policy` test with the existing `Engine` builders (`tests/detection.rs`/`tests/policies.rs` show the pattern) | same |
| 12 | `block_cause` leaves `engine.stats()` unchanged and does not advance the tunneling window | `policy` test | same |
| 13 | `block_page.enabled` with `blocking.mode != sinkhole` is rejected | `config.rs` unit test (use the `MINIMAL` helper, `config.rs:1530-1552`) | same |
| 14 | `block_page.listen` colliding with an enabled API/metrics listener is rejected | `config.rs` unit test | same |
| 15 | The shipped config parses and keeps `block_page.enabled == false` | extend `the_packaged_configuration_is_valid` (`config.rs:2276-2282`) | same |

`reqwest` can set an explicit `Host` header with `.header("Host", "…")`; the
existing test client helper (`observability.rs:459-476`) is the base. Prefer
`http://127.0.0.1:<port>/` plus the header override over DNS tricks.

Only test 12 needs the read-only path; tests 1-10 are pure router/string tests
and do not need a real `Engine`.

### Q8 — What would make this larger than a quick task

Call these out and stop rather than absorb them:

- **TLS/443 and certificates.** Locked out. HTTPS users get a certificate
  warning or an HSTS hard abort; this is documented, not solved.
- **A template engine** (minijinja/askama) or any new dependency. Not needed:
  `include_str!` + two placeholder replacements + a five-line escaper.
- **Refactoring `Engine::evaluate`'s counter/detector split.** The
  `Effects::{Count, ReadOnly}` variant is small and contained. Rewriting
  statistics ownership across the pipeline is not.
- **A second config format decision** (per-client pages, theme toggle, TLS
  listener list, multiple `listen` addresses). The section stays `enabled` +
  `listen`.
- **An ADR.** `docs/TODOS.md` §8 says a *full* block page is "M–L plus an ADR"
  (`docs/TODOS.md:628`); CONTEXT does not ask for one. A short ADR-0033 is
  defensible if the maintainer wants the HTTP-only choice recorded, but it is
  scope growth for a quick task.
- **IPv6 dual-stack.** One `listen` address only. On Linux `[::]:80` usually
  covers IPv4, but v4-mapped peers defeat `IpNet` client matching (the same
  argument ADR-0031 used to reject `[::]` alone). Keep `0.0.0.0:80` as the
  default and document the IPv6 limitation.
- **Rebinding-detector interaction.** `docs/TODOS.md:592-596` predicted that
  every blocked domain would trip the rebinding detector. In the current
  pipeline it cannot: `PolicyBackend` returns `block_response` **before**
  `inner.resolve` and therefore before `inspect_answer`
  (`crates/alpendns/src/policy/mod.rs:645-655`). Do not add an exemption as part
  of this task; record the check.

## Architectural Responsibility Map

| Capability | Primary Tier | Secondary Tier | Rationale |
|---|---|---|---|
| HTTP listener on port 80 | API/backend, in-process axum | — | Reuse the stack already serving API/metrics; no separate service or dependency |
| Name validation and normalization | Policy layer (`Name::from_str_relaxed` + `normalize`) | — | One parser and one normalization rule for DNS and HTTP |
| "Why was it blocked?" | Policy layer, read-only (`block_cause`) | — | The trace is the single source of truth (B.3 rule 3); no second evaluation |
| Page rendering | Server (embedded static asset) | — | No client data fetch; works offline and without JS except the theme |
| Theme | Browser/client | Server (single token set) | `matchMedia` → `data-theme` before first paint; values live once in `app.css` |
| Configuration | Config layer | — | Additive defaulted section, strict unknown-key rule |

## Standard Stack

### Core

| Library | Version | Purpose | Why Standard |
|---|---|---|---|
| axum | 0.8.9 | HTTP listener/router | Already in the tree for API and UI; same bind/serve pattern |
| tokio | 1.53 | Async runtime, `TcpListener` | The project's only runtime (B.2) |
| hickory-proto | 0.26.1 | `Name` parsing/normalization | The project parses DNS names only through hickory (ADR-0002) |

### Supporting

| Library | Version | Purpose | When to Use |
|---|---|---|---|
| reqwest (dev) | workspace | Router tests | Only in `tests/`, never in the production path |
| tracing | workspace | Startup/shutdown lines | Same as the API/metrics listeners; never log the `Host` |

### Alternatives Considered

| Instead of | Could Use | Tradeoff |
|---|---|---|
| Separate axum listener | Route on the API router | Forbidden by CONTEXT: the API is token-protected and must not share a routing surface with an open endpoint |
| `include_str!` + placeholder | Template crate | New dependency for two substitutions; rejected |
| Reuse `app.css` | Dedicated `blocked.css` re-declaring tokens | Duplicates the most sensitive values in the project; drift risk |
| `Host` extractor | `axum-extra` | New dependency for one header; read `HeaderMap` instead |

**Installation:** none. No dependency is added.

**Version verification:** `axum 0.8.9`, `hickory-proto 0.26.1` from
`Cargo.lock` (workspace pins `axum = "0.8"`, `hickory-proto = "0.26"`).

## Package Legitimacy Audit

No packages are installed by this task. Nothing to audit; no `[ASSUMED]` package
names appear in this research.

## Architecture Patterns

### System Architecture Diagram

```
DNS path (already built)
  DNS query for "ads.example.com"
     → PolicyBackend::resolve
        → Engine::evaluate_typed → Decision::Block
           → block::synthesize(Sinkhole) → A = sinkhole_ipv4
     → client's browser connects to sinkhole_ipv4:80

HTTP path (this task)
  GET /  (Host: ads.example.com)          [path ignored; fallback serves the page]
     │
     ▼
  TcpListener 0.0.0.0:80 → axum::serve(…, into_make_service_with_connect_info)
     │  HeaderMap → Host (strip numeric port)
     │  ConnectInfo → peer IpAddr
     ▼
  BlockPageSource::cause(name, peer)
     │
     ▼
  policy::block_cause(engine, name, peer)          [READ-ONLY]
     │  Name::from_str_relaxed → normalize
     │  ArcSwap::load → Clients::identify(peer) → Policy
     │  temporary allow → temporary deny → allowlist → blocklist → regex → schedule
     │  (detectors NOT run: they mutate and count)
     ▼
  BlockCause::{NotBlocked, List(list_name), Rule}
     │
     ▼
  render(blocked.html, escaped name, reason)
     │
     ▼
  200 text/html; charset=utf-8 · Cache-Control: no-store · (nosniff)
```

### Recommended Project Structure

```
crates/alpendns/src/
├── api/
│   ├── mod.rs           # existing API + UI router
│   ├── ui.rs            # existing embedded assets (STYLE made pub(super))
│   └── block_page.rs    # NEW: router, Host handling, render, escape, tests
├── policy/
│   └── mod.rs           # NEW: BlockCause, block_cause, Effects::ReadOnly
├── config.rs            # NEW: BlockPageConfig, defaults, validate, tests
└── main.rs              # NEW: bind/spawn the third listener; impl BlockPageSource
web/
├── app.css              # reused, unchanged
└── blocked.html         # NEW: template with {{NAME}} / {{REASON}}
```

### Pattern 1: Independent listener, shared state, no shared router

**What:** Build a standalone `Router` in `api::block_page`, give it its own
small state (one trait object), and spawn it with `axum::serve` exactly as the
API and metrics listeners are spawned.

**When to use:** Whenever a surface has a different audience/auth model. Here:
open page vs token-protected API.

**Sketch:**

```rust
pub fn router(source: Arc<dyn BlockPageSource>) -> Router {
    Router::new()
        .route("/app.css", get(stylesheet))
        .fallback(page)          // any path shows the page; no 404 for deep URLs
        .with_state(BlockPageState { source })
}
```

`fallback` matters: a browser that was heading for
`http://blocked.example/some/deep/path` must still get the message. Serve
`/app.css` from the same listener by reusing the embedded `STYLE` const
(`ui.rs:16`); make it `pub(super)` or add a shared accessor.

### Pattern 2: Read-only evaluation via the same trace

**What:** One evaluation path, an explicit `Effects` switch. `Count` runs
counters + detectors (DNS, CLI, `/api/explain` today); `ReadOnly` records the
same `Step`s and stops before both.

**When to use:** Any surface that asks "why" but is not a DNS query.

**Why:** Keeps `deciding_step` and the step vocabulary as the single source of
truth; avoids a second precedence ladder.

### Pattern 3: Server-side render with an escaping boundary

**What:** `include_str!("../../../../web/blocked.html")`, replace two
placeholders, escape every dynamic value. The list name comes from operator
config and the name from the network: both are escaped.

**When to use:** Static page + tiny dynamic content. No template engine.

### Anti-Patterns to Avoid

- **Calling `Engine::evaluate`/`policy::explain` from HTTP.** Corrupts
  `blocked`/`allowed`/`passed` counters and feeds the tunneling window.
- **Parsing `Explanation.steps` strings.** Formats change; `Step` is typed.
- **Using loopback as the policy peer.** Shows the wrong list for clients with
  their own policy.
- **Rendering `Host` raw.** XSS and false accusations.
- **Adding the block page to `public_listeners()`.** `check` then calls an HTTP
  page an "open resolver".
- **`no-cache` instead of `no-store`.** TODOS §8 is explicit: the browser must
  not keep the page past the end of the block (`docs/TODOS.md:600-603`).

## Don't Hand-Roll

| Problem | Don't Build | Use Instead | Why |
|---|---|---|---|
| HTTP parsing | A TCP/HTTP parser | axum/hyper (already a dependency) | Request smuggling, header limits, keep-alive |
| DNS name validation | A custom hostname regex | `Name::from_str_relaxed` | Same parser as the policy engine; handles escapes and IDN consistently |
| Policy evaluation | A second precedence ladder in the HTTP module | `evaluate_with(…, ReadOnly)` + `deciding_step` | Drift between DNS and page answers |
| Theme tokens | A second palette in `blocked.css` | `web/app.css` (`:root` / `[data-theme="dark"]`) | Contrast tests and one source for every colour |
| HTML escaping | (justified small exception) | Five-line `&<>"'` escaper + tests | A crate for five characters is not warranted; the test is the guard |

**Key insight:** the only thing that should be new is the *transport*. Every
semantic value — what is blocked, why, and what the page looks like — already
exists and must be reused.

## Common Pitfalls

### Pitfall 1: The "explain" path counts and mutates
**What goes wrong:** the page view increments block/pass/detector counters and
adds a synthetic observation to the tunneling window.
**Why it happens:** `explain`'s doc comment is stale; `evaluate` counts and
`Tunneling::inspect` mutates (`policy/mod.rs:351-355`,
`detect/tunneling.rs:306-326`).
**How to avoid:** `Effects::ReadOnly`, skip detectors, test
`engine.stats()` before/after.
**Warning signs:** the UI's block rate rises while DNS traffic is flat; a
tunneling finding appears for a domain nobody queried twice.

### Pitfall 2: `ConnectInfo` extractor without the right service
**What goes wrong:** every request returns 500; the extractor cannot find peer
info.
**Why it happens:** `axum::serve(listener, router)` does not attach connect
info; the API's existing spawn uses exactly that form.
**How to avoid:** `router.into_make_service_with_connect_info::<SocketAddr>()`;
a router test that asserts 200 (not 500).

### Pitfall 3: Binding port 80 in a test
**What goes wrong:** tests fail for unprivileged users and in CI.
**Why it happens:** the production default is 80.
**How to avoid:** tests bind `127.0.0.1:0`; only the real process uses the
configured `listen`.

### Pitfall 4: The page is unstyled on port 80
**What goes wrong:** `GET /` works but `/app.css` is 404 on the block listener.
**Why it happens:** the UI routes live in `ui::router` on the API listener.
**How to avoid:** add the stylesheet route to the block-page router; a test
fetches both.

### Pitfall 5: `Host` with a port or IPv6 literal
**What goes wrong:** `example.com:8080` fails name parsing; `[::1]:80` is shown
as a bad name.
**Why it happens:** the port is not part of the DNS name.
**How to avoid:** strip a trailing `:<digits>` only; treat bracketed literals as
not-a-domain.

### Pitfall 6: Browser caching past the unblock
**What goes wrong:** after the operator removes the name from the list, the
browser keeps showing the block page.
**Why it happens:** default HTTP caching or a proxy.
**How to avoid:** `Cache-Control: no-store` (and consider `Vary: Host` if a proxy
is ever introduced).

### Pitfall 7: Logging the name
**What goes wrong:** a `tracing::info!(host = …)` violates B.1 rule 3 and the
central promise.
**Why it happens:** HTTP request logging is an easy habit.
**How to avoid:** do not add a trace layer; never log the header. The name
leaves only in the response body to the requester.

### Pitfall 8: HTTPS expectations
**What goes wrong:** the operator enables the page and still sees certificate
errors on most sites.
**Why it happens:** HTTPS requires a valid certificate for the blocked name;
HSTS sites abort hard.
**How to avoid:** document it in OPERATIONS next to the recipe; do not attempt
TLS in this task.

## Code Examples

### Host extraction and validation

```rust
use axum::http::{HeaderMap, header};
use hickory_proto::rr::Name;

/// The Host header without an optional numeric port; `None` when what remains
/// is not a domain name. A bracketed IPv6 literal is not a domain name.
fn host_name(headers: &HeaderMap) -> Option<String> {
    let raw = headers.get(header::HOST)?.to_str().ok()?;
    let without_port = match raw.rsplit_once(':') {
        Some((name, port))
            if !name.ends_with(']') && !port.is_empty()
                && port.bytes().all(|b| b.is_ascii_digit()) =>
        {
            name
        }
        _ => raw,
    };
    let name = Name::from_str_relaxed(without_port).ok()?;
    let text = name.to_ascii();
    let trimmed = text.trim_end_matches('.').to_ascii_lowercase();
    (!trimmed.is_empty()).then_some(trimmed)
}
```
*Design sketch. `Name::from_str_relaxed` and `to_ascii` are verified
(`hickory-proto-0.26.1/src/rr/domain/name.rs:581-584`, `:769-775`).*

### Handler and response headers

```rust
async fn page(
    State(state): State<BlockPageState>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
) -> Response {
    let Some(name) = host_name(&headers) else {
        return (StatusCode::BAD_REQUEST, "AlpenDNS: no valid domain in the Host header")
            .into_response();
    };
    let cause = state.source.cause(&name, peer.ip());
    let body = render(&name, &cause);

    (
        [
            (header::CONTENT_TYPE, "text/html; charset=utf-8"),
            (header::CACHE_CONTROL, "no-store"),
            (header::X_CONTENT_TYPE_OPTIONS, "nosniff"),
        ],
        body,
    )
        .into_response()
}
```

### Escaping boundary

```rust
fn escape(text: &str) -> String {
    text.chars().fold(String::with_capacity(text.len()), |mut out, c| {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            _ => out.push(c),
        }
        out
    })
}
```

### Page template shape (no external references)

```html
<!DOCTYPE html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>Blocked — AlpenDNS</title>
<link rel="stylesheet" href="/app.css">
<script>
  document.documentElement.dataset.theme =
    matchMedia("(prefers-color-scheme: dark)").matches ? "dark" : "light";
</script>
<style>
  .block-page { min-height: 100dvh; display: grid; place-items: center;
                padding-block: var(--s-32); }
  .block-page .panel { max-width: 38rem; padding: var(--s-32); }
</style>
</head>
<body>
  <div class="sky" aria-hidden="true"></div>
  <main class="wrap block-page">
    <section class="panel">
      <h1 class="wordmark"><b>Alpen</b>DNS</h1>
      <p class="subject">{{NAME}}</p>
      <p>{{REASON}}</p>
      <p class="note">Blocked on this network. Nothing else about this
      installation is shown here.</p>
    </section>
  </main>
</body>
</html>
```
*Shape only; the actual copy and classes are a design task. Do not add
`<img>`, external fonts, a CDN, or a second panel.*

## State of the Art

| Old Approach | Current Approach | When Changed | Impact |
|---|---|---|---|
| Blocked name → no page (`nxdomain`) | `sinkhole` mode exists, returns configured A/AAAA | Phase 4 (`block.rs:26-44`) | The DNS half is done; only the HTTP answer is missing |
| UI assets served from disk | `include_str!` into the binary | Phase 6 (`api/ui.rs:15-17`) | The page must use the same embedding, no files at runtime |
| API listener with token | Separate metrics listener without token | Phase 6 (`api/mod.rs:1-12`) | Establishes the precedent for "different audience, own listener" |

**Deprecated/outdated:** `docs/TODOS.md:592-596`'s warning that every blocked
name would trip the rebinding detector no longer applies: blocked answers never
reach `inspect_answer` (`policy/mod.rs:645-655`).

## Assumptions Log

| # | Claim | Section | Risk if Wrong |
|---|---|---|---|
| A1 | Default `block_page.listen` should be `0.0.0.0:80` | Q4 | Page reachable only on loopback if the operator forgets; contradicts "must be reachable from the LAN" |
| A2 | `Cause::Rule` (regex/schedule/temporary deny) shows a generic "blocked by a rule" sentence, no list name | Q2/Q4 | The page would show a list name where none exists, or claim nothing was blocked |
| A3 | The HTTP peer IP, not loopback, is the right identity for the lookup | Q2/Q3 | Wrong list on per-client policies |
| A4 | No ADR is required for this quick task | Q8 | The project's ADR habit / TODOS §8 expects one; would add one file and decisions to review |
| A5 | A three-line inline `data-theme` script is an acceptable exception to "no JS needed" | Q6 | If refused, the dark variable set must be duplicated in a media query |
| A6 | Detectors may be skipped entirely in the page's verdict | Q2 | A detector switched from `flag` to `block` would produce the neutral page (`NotBlocked`, never `Rule`) with no list name; the neutral copy says only that no blocklist entry applies, so it stays true. B.8 makes that switch a stop-and-ask; default is `flag` |

## Open Questions

1. **Should the page name the list for non-list blocks?**
   - What we know: CONTEXT requires "the name of the list that matched" and forbids rule text.
   - What's unclear: whether a regex/schedule block should be visually distinguished from a list block.
   - Recommendation: `Cause::Rule` → "blocked by a rule on this network"; no details.
   - **RESOLVED:** `BlockCause::Rule` renders "blocked by a rule on this network" with no list name. A detector block is not a `Rule` cause at all: `Effects::ReadOnly` skips `inspect`, so it reaches `BlockCause::NotBlocked`, whose neutral copy says only "No blocklist entry applies to this name." and never claims the name was allowed or unblocked. Pinned by `block_cause_does_not_run_detectors` and the neutral-copy assertion.

2. **ADR or not?**
   - What we know: `docs/TODOS.md:628` expected an ADR for the full block page; CONTEXT does not.
   - What's unclear: whether the maintainer wants the HTTP-only decision recorded formally.
   - Recommendation: skip for a quick task; mention it in the plan's summary if the maintainer wants it added later.
   - **RESOLVED:** no ADR in this quick task. The HTTP-only choice is documented in the plan, the config comments and OPERATIONS; a follow-up ADR can record it if the maintainer asks.

3. **IPv6 clients.**
   - What we know: one `listen` address; `[::]:80` may cover IPv4 on Linux but breaks `IpNet` matching for v4-mapped peers (ADR-0031 reasoning).
   - Recommendation: keep `0.0.0.0:80`, document the limitation; a second listen address is a later additive change.
   - **RESOLVED:** keep `0.0.0.0:80`; the IPv6-only limitation is carried into `docs/OPERATIONS.md` and the `[block_page]` comment in the example and packaged configs.

4. **`check` output for the page.**
   - What we know: `public_listeners()` flows into an "open resolver" warning.
   - Recommendation: a separate line, not included in that list.
   - **RESOLVED:** a separate `check` line; `public_listeners()` is untouched and its packaged-config test still expects exactly two public DNS listeners.

## Environment Availability

| Dependency | Required By | Available | Version | Fallback |
|---|---|---|---|---|
| Rust stable / edition 2024 | Build | ✓ | `rust-toolchain.toml` | — |
| axum | HTTP listener | ✓ | 0.8.9 (Cargo.lock) | — |
| tokio (net, rt-multi-thread) | Listener/runtime | ✓ | 1.53 | — |
| hickory-proto | Name parsing | ✓ | 0.26.1 | — |
| reqwest (dev) | Router tests | ✓ | workspace | raw `TcpStream` text request |
| Bind port 80 as non-root | Production | ✗ in dev/test | — | Systemd `CAP_NET_BIND_SERVICE`; tests use ephemeral ports |

**Missing dependencies with no fallback:** none.

**Missing dependencies with fallback:** port-80 binding outside systemd — use
`systemd-run`/root for a manual smoke test, or point `[block_page].listen` at a
high port for testing.

## Validation Architecture

### Test Framework

| Property | Value |
|---|---|
| Framework | Rust built-in test harness; `#[tokio::test]`; `reqwest` dev-dependency |
| Config file | none — `cargo test` conventions |
| Quick run command | `cargo test -p alpendns block_page` |
| Full suite command | `cargo test --all-features` (the definition of done, `docs/TESTING.md:157-165`) |

### Phase Requirements → Test Map

No requirement IDs were supplied for this quick task. Map the task's capabilities:

| Behaviour | Test Type | Automated Command | File Exists? |
|---|---|---|---|
| Listener + response headers | integration (real port) | `cargo test -p alpendns block_page` | ❌ Wave 0 |
| Name shown from `Host` | integration | same | ❌ Wave 0 |
| List name shown / neutral when unmatched | integration | same | ❌ Wave 0 |
| Hostile `Host` cannot inject markup | unit | `cargo test -p alpendns escape` | ❌ Wave 0 |
| Theme follows system, no toggle | static string test | `cargo test -p alpendns block_page` | ❌ Wave 0 |
| No external references in the page | static string test | same | ❌ Wave 0 |
| `block_cause` read-only and agrees with policy | unit/integration | `cargo test -p alpendns block_cause` | ❌ Wave 0 |
| Config validation (mode, collisions) | unit | `cargo test -p alpendns block_page` | ❌ Wave 0 |
| Packaged config remains valid and off | unit | `cargo test -p alpendns the_packaged_configuration` | ✅ extend existing |

### Sampling Rate

- **Per task commit:** `cargo test -p alpendns block_page`
- **Per wave merge:** `cargo fmt --all --check && cargo clippy --all-targets --all-features -- -D warnings && cargo test --all-features && cargo deny check` (the four DoD commands)
- **Phase gate:** full definition of done green before commit (B.4, B.7).

### Wave 0 Gaps

- [ ] `web/blocked.html` — the page and its placeholders
- [ ] `crates/alpendns/src/api/block_page.rs` — router, handler, `host_name`, `render`, `escape`, static + real-port tests
- [ ] `crates/alpendns/src/policy/mod.rs` — `BlockCause`, `block_cause`, `Effects` switch, read-only/agreement tests
- [ ] `crates/alpendns/src/config.rs` — `BlockPageConfig`, defaults, validation, packaged-config assertion
- [ ] `crates/alpendns/src/main.rs` — third listener + `BlockPageSource` impl (typed `Arc<Runtime>` so it can coerce to both trait objects)
- [ ] `docs/OPERATIONS.md` — the operator recipe
- [ ] `config/alpendns.example.toml`, `packaging/alpendns.toml` — disabled section

## Security Domain

### Applicable ASVS Categories

| ASVS Category | Applies | Standard Control |
|---|---|---|
| V2 Authentication | no | Open by design; the page exposes no privileged action |
| V3 Session Management | no | No session, no cookie, no `Set-Cookie` |
| V4 Access Control | yes | Nothing beyond name + list; no client IP, policy, rule text, token, or UI link |
| V5 Input Validation | yes | `HeaderMap` → `Name::from_str_relaxed` → normalized ASCII → HTML escape; 400 on invalid |
| V6 Cryptography | no | HTTP only; no TLS in this task (documented) |
| V7 Errors/Logging | yes | No request logging; no name in `tracing` (B.1 rule 3) |

### Known Threat Patterns for this stack

| Pattern | STRIDE | Standard Mitigation |
|---|---|---|
| Reflected XSS via `Host` | Tampering / Elevation | Validate as DNS name; render ASCII form; escape at the boundary; malicious-input test |
| Echo-host false accusation / policy oracle | Spoofing / Info disclosure | Request-time `block_cause`; neutral page when not blocked |
| Cache poisoning of a per-host page | Tampering | `Cache-Control: no-store`; no shared cache |
| Reflection/amplification | DoS | TCP handshake required; small fixed page; no DNS rate limiter needed |
| Information disclosure | Info disclosure | Only name + list; never client IP/policy/rule/token/counts |
| Header/URI abuse (long Host, deep path) | DoS | hyper header limits; name length rejected by hickory; `fallback` returns the same fixed page |
| Log injection / name leakage | Repudiation / Info disclosure | No trace layer; no `tracing` call with the header |

## Project Constraints (from CLAUDE.md)

- **B.1 rule 1:** no `unwrap()`, `expect()`, `panic!()`, or slice indexing on the
  request path. The handler must use `?`/`ok()?`/`get()`; tests may panic.
- **B.1 rule 3:** the query name may only leave through `logging::QueryLog`.
  The block page processes a name but must not log it anywhere.
- **B.1 rule 5:** broken configuration is a startup error; `deny_unknown_fields`
  applies to the new section; a typo must not silently disable the page.
- **B.2:** no new dependency without one line of justification; no second async
  runtime; `thiserror` in the library, `anyhow` only in the binary;
  `tracing`, never `println!`.
- **B.4:** every parser gets broken/truncated/malicious input tests; the four
  definition-of-done commands are the finish line.
- **B.5:** the service runs unprivileged; port 80 comes from
  `CAP_NET_BIND_SERVICE`, not root.
- **B.6:** one glass container, no nesting, no text on the bare sky, semantic
  colour only, 8-based spacing, system fonts, monospace names, no external
  resources, contrast as a test.
- **B.7:** conventional commit, one logical change, no attribution lines.
- **B.8:** stop before adding a network-opening dependency, changing the config
  format incompatibly, or softening a hard rule. This design does none of
  those.

## Sources

### Primary (HIGH confidence — read this session)

- `crates/alpendns/src/filter/block.rs:26-44`, `:79-120` — `BlockMode`
  (`Nxdomain` default, `ZeroIp`, `Sinkhole`) and `synthesize`.
- `crates/alpendns/src/config.rs:299-312`, `:425-436`, `:1316-1323`,
  `:1415-1425`, `:1530-1552`, `:2196-2213`, `:2276-2282`, `:2331-2345` —
  config patterns, validation, shipped-config tests.
- `crates/alpendns/src/api/mod.rs:32-55`, `:106-150`, `:162-195` —
  `StatusSource`, `ApiState`, token middleware.
- `crates/alpendns/src/api/ui.rs:15-49`, `:51-853` — `include_str!`, asset
  serving, static-asset test style, contrast computation.
- `crates/alpendns/src/main.rs:213-215`, `:519-579`, `:728-862`, `:1195-1204` —
  runtime state, listener spawns, `StatusSource::explain`, `check` output.
- `crates/alpendns/src/policy/mod.rs:78-116`, `:203-307`, `:347-355`,
  `:645-655`, `:695-708` — `Explanation`, `explain`, `evaluate_typed`,
  counters, block-before-upstream order, `deciding_step`.
- `crates/alpendns/src/trace.rs:42-123`, `:125-196` — typed `Step` variants
  and rendering.
- `crates/alpendns/src/filter/matcher.rs:122-154` — `list_name`, suffix lookup.
- `crates/alpendns/src/detect/mod.rs:347-391`,
  `crates/alpendns/src/detect/tunneling.rs:292-326` — detector mutation and
  counting.
- `crates/alpendns/tests/observability.rs:289-476` — `StatusSource` fake,
  real-port axum test pattern, token helpers.
- `web/app.css:57-171`, `:196-222`, `:251-268`, `:347-354`, `:954`,
  `:962-976` — token set, sky, container, wordmark, monospace, reduced modes.
- `web/index.html:1-41` — head script, sky, inline logo, stylesheet link.
- `packaging/systemd/alpendns.service:57-59`, `:79`, `:96-102` — capabilities
  and hardening.
- `Cargo.lock:214-217` — axum 0.8.9; `crates/alpendns/Cargo.toml`,
  `Cargo.toml` — workspace dependencies.
- `axum-0.8.9/src/extract/connect_info.rs`, `axum-0.8.9/src/service_ext.rs:33`
  — `ConnectInfo` and `into_make_service_with_connect_info`.
- `hickory-proto-0.26.1/src/rr/domain/name.rs:581-584`, `:769-775`;
  `src/rr/domain/label.rs:179-235` — `from_str_relaxed`, `to_ascii`, escaping.
- `man 7 capabilities` — `CAP_NET_BIND_SERVICE` covers ports < 1024.

### Secondary (MEDIUM confidence)

- `docs/TODOS.md:568-630` — block-page analysis: HTTPS certificate warning,
  non-browser clients, rebinding concern, `no-store`, effort/ADR note.
- `docs/adr/0022-glas-als-flaeche.md:24-60` — glass material, `data-theme`
  before first paint, contrast as a test.
- `docs/adr/0031-auslieferung-auf-allen-schnittstellen.md` — wildcard-listener
  reasoning and the v4-mapped-address problem.
- `docs/OPERATIONS.md` §1 — where the operator recipe belongs.
- `docs/TESTING.md:157-165`, `:1-70` — definition of done, test levels.

### Tertiary (LOW confidence)

- None. No web search was used; every claim above is from the pinned source or
  the project's documentation.

## Metadata

**Confidence breakdown:**
- Standard stack: HIGH — versions read from `Cargo.lock` and the pinned crate
  sources.
- Architecture: HIGH for the listener, config, and policy integration points;
  MEDIUM for the exact shape of the `Effects::ReadOnly` refactor (the call graph
  is verified, the diff is a design sketch).
- Pitfalls: HIGH — the counter mutation, detector mutation, and
  block-before-upstream order were read in the source.
- Presentation/theming: MEDIUM — the reuse of `app.css` and the three-line
  script satisfy B.6/ADR-0022 as written, but the visual result needs a human
  look.

**Research date:** 2026-10-22
**Valid until:** ~2026-11-21 (stable stack, pinned versions; revisit if axum or
hickory is upgraded).

## RESEARCH COMPLETE
