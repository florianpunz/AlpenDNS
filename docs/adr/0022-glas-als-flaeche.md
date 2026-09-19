# ADR-0022: Glass as a surface

**Status:** accepted · **Date:** 2026-09-13 · **Affects:**
[ROADMAP.md](../ROADMAP.md) Phase 6, `web/` (index.html, app.css, app.js),
`crates/alpendns/src/api/ui.rs`, the web UI rules in CLAUDE.md B.6

## Context

B.6 explicitly banned glassmorphism — in a row with emoji icons and
animated numbers: "No gradients, no emoji as icons, no animated
number counters, no glassmorphism surfaces, no decorative accents." The
ban did not stand there without reason. The rule behind it is that the interface
claims nothing it does not mean: colour carries meaning or it is not
there, and a surface is a surface.

As a feasibility study, a preview nonetheless came about that tried out exactly the
opposite material: translucent panes over a background
of four colour fields. Looking at the rendered page turned the decision
around — the material carries the interface better than the printed surface that
it replaces, and the three questions from B.6 are still all there at the same
time.

So the rule stands against the result. This ADR records why the rule
gives way and at what point it does **not**.

## Decision

**The background may carry colour without meaning — and is the only place
where that is allowed.**

The sky consists of four colour fields (`--field-1` to `--field-4`: sky,
alpenglow, meadow, shadow) over a base colour `--base`, all deeply
desaturated. Their opacity is the most sensitive value in the file: higher, and it
is decoration; lower, and glass is a grey rectangle. The fields sit
below the text threshold, repeat no state, and the page stays
fully readable if one does not notice them.

Everything else from B.6 stays untouched: the four semantic colours in the content
plus `--brand`, the greyscale ramp for stacked surfaces, the centred
container, the 8-based spacings, the one kind of container. It is now a
glass edge — translucent surface, `backdrop-filter`, light edge along the top,
soft shadow — with 18px instead of 12px radius, because glass breaks more softly. It is
still not nested.

Three details that belong with it:

1. **`--glass-strong` exists only once.** The table header is the only
   surface with something moving beneath it. Over running content more has to
   cover, otherwise one reads two texts on top of each other; over a fixed
   background the same opacity would be waste.
2. **The mode is set as `data-theme` on the root element, not in a
   media query.** A switch in the header (sun in light, moon in
   dark mode) can only override the system preference if the preference and the
   choice are two different things. A short script in the head of the page sets
   the attribute before the first paint — if the choice only arrived at the end
   of the document, the light theme would briefly flash on every load.
3. **The contrast is computed, not estimated.** `the_text_stays_readable_on_every_field`
   checks every text colour against every field in both modes, through the glass, with
   4.5:1 as the limit. B.6 so far said "colour is exclusively semantic" as an
   intention; now a calculation stands in its place.

What B.6 loses at this point is the ban on gradients and
glass surfaces. What it keeps are emoji icons, animated counters and decorative
accents — and the contrast limit joins as a checkable rule.

## Consequences

* **The test found a real bug on its first run.** `--faint` came
  out at 3.2:1 on the bare sky; darkening it far enough that it suffices
  would have made it merge with `--muted`. The resolution is not darker
  text but a ground: **every surface with text is a pane.** Affected
  were only the two places outside the grid — the login page and the
  `noscript` notice. The test `no_text_sits_on_the_bare_sky` pins that down.
* **Two tests change, both for the same reason.** `nothing_decorative_crept_in`
  (banned gradients and `backdrop-filter`) is replaced by the contrast calculation.
  `motion_is_reduced_on_request` checked for `@media (prefers-color-scheme: dark)`
  and now checks for `:root[data-theme="dark"]`. Everything else stayed green,
  including the palette (`--danger` and the three others each defined exactly twice)
  and the rule that colour appears only in selectors with meaning.
* **No grain.** The preview had a fine noise over the sky, because
  large gradients banded visibly on 8-bit displays. Measured on the
  rendered image: the colour steps are at 1/255 over 5 to 28 px, so at the
  limit of what 8 bits can give. Grain would change nothing about that, only
  add texture — and an SVG data URI would have brought `http://` into the CSS,
  which `the_page_references_nothing_from_outside` rightly forbids. The
  `--grain` token is gone.
* **`prefers-reduced-transparency: reduce` replaces the blur with an opaque
  surface.** The blur is the most expensive part of the page; it stays readable, only without
  material.
* **The preview page is deleted** (`web/preview.html`). It was a
  feasibility study; its content now lives in the real UI, and two pages
  with the same design would be two pages that drift apart.
* Data sources, endpoints and the structure of the page stayed unchanged. The
  work happened in `web/` and in the UI tests; no Rust outside of
  `#[cfg(test)]` was touched.

## Alternatives

* **Neumorphism instead of glassmorphism.** Checked and rejected: neumorphism
  needs a uniform ground to read its two shadows. With
  a background that has something to break, the effect either falls away or
  it becomes an edge. On top of that comes contrast — soft shadows on a surface
  close to the base colour are barely distinguishable in dark mode.
* **Keep the flat design and only add the sky.** Would not have touched the
  colour rule. Without the glass, though, the sky would be wallpaper: it would
  be visible only where no surface lies, and the panels would stay white sheets
  on top of it.
* **`light-dark()` instead of `data-theme`.** Every colour would appear exactly once, and
  `color-scheme` alone would switch. Rejected because `:root { --danger:
  light-dark(a, b) }` brings the palette down to **one** definition, while
  `the_palette_is_semantic_and_exists_in_both_schemes` insists on two — and
  because the rule "light and dark are equivalent" would then only be asserted:
  one no longer sees the two values next to each other.
* **Three blocks: light, `prefers-color-scheme` and `[data-theme]`.** The price
  would be that the dark values appear twice and drift apart on the next touch.
  Exactly the kind of duplication for which the test exists.
* **The grain as an SVG data URI.** See above: `xmlns='http://www.w3.org/2000/svg'`
  in the CSS. The check for foreign origin is too valuable to soften it for a
  texture that demonstrably repairs nothing.
* **The switch as a text button (light/dark/automatic)**, as in the
  preview. Rejected in favour of a symbol: the sun says without a click which
  mode one is in, and the moon shows where the next one leads. A third
  state "automatic" would be a setting that nobody sets.
