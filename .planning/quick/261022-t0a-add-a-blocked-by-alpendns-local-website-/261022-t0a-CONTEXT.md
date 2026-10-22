# Quick Task 261022-t0a: A "Blocked by AlpenDNS" page for sinkholed names - Context

**Gathered:** 2026-10-22
**Status:** Ready for planning

<domain>
## Task Boundary

A blocked name can already be answered with an address instead of NXDOMAIN:
`block.mode = "sinkhole"` returns `block.sinkhole_ipv4` / `sinkhole_ipv6`
(`crates/alpendns/src/filter/block.rs:95-96`). Point that at the AlpenDNS host
and the client's browser lands on this machine. What is missing is something
listening there.

The task: serve a "Blocked by AlpenDNS" page from a dedicated HTTP listener, in
the visual language of the Web UI, with light and dark themes following the
system preference.

It is a page, not a redirect: DNS decides which address is returned, HTTP
decides what the client sees afterwards. With `block.mode = "nxdomain"` the
page is never reachable, and that stays the default.

</domain>

<decisions>
## Implementation Decisions

### Delivery
- **Own listener on port 80**, unauthenticated, serving only the block page.
  Not a route on the existing API listener: the API is token-protected and must
  not share a routing surface with an open endpoint.
- **HTTP only, no port 443.** No certificate generator, no key storage, no TLS
  code in this task. A browser that tries `https://` first gets a connection
  error rather than this page; that limitation is documented, not worked
  around.

### Content
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

</decisions>

<specifics>
## Specific Ideas

- The page is the only AlpenDNS surface an outside device will ever fetch.
  It should say plainly what happened and imply nothing about the rest of the
  installation.
- Language: English, like the Web UI and the documentation.
- One kind of container, glass edge, greyscale except where colour is semantic
  — the rules in CLAUDE.md B.6 apply to this page too, not only to the UI.
- No JavaScript is needed to show a static page; the theme switch can be pure
  CSS (`prefers-color-scheme`). Any script would have to justify itself.

</specifics>

<canonical_refs>
## Canonical References

- `docs/adr/0022-glas-als-flaeche.md` — the glass material and its reasoning
- `docs/adr/0031-bind-all-interfaces.md` — why the shipped listener is reachable
  on the LAN, and the open-resolver warning that does **not** apply to this page
- CLAUDE.md B.6 — the Web UI rules that bind this page as well
- `web/app.css` — the variable set to reuse (spacing scale, band ramp, semantic
  colours, light/dark as equals over `data-theme`)
- `crates/alpendns/src/api/ui.rs` — how the assets are embedded and served
- `crates/alpendns/src/filter/block.rs` — `BlockMode::Sinkhole` and the
  addresses it returns
- `crates/alpendns/src/config.rs` — `block.mode`, `sinkhole_ipv4`,
  `sinkhole_ipv6`, and the strict `deny_unknown_fields` rule (B.1 rule 5)

</canonical_refs>
