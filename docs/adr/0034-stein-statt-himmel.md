# ADR-0034: Stone instead of sky — glass only on floating surfaces

**Status:** accepted · **Date:** 2026-11-02 · **Affects:**
`web/` (index.html, app.css, app.js), `crates/alpendns/src/api/ui.rs`, the web
UI rules in CLAUDE.md B.6

## Context

[ADR-0022](0022-glas-als-flaeche.md) put the whole content on translucent panes
over four colour fields. That material suited a page of a few large panels. The
overview now carries a dense table that redraws continuously, and two properties
of the old design work against it:

* The blur is the most expensive thing on the page. Over a table that receives a
  row several times a second, the browser re-blurs the moving content under
  every pane, and the result is a slightly softer version of what the pane
  already shows.
* The four fields were colour without meaning, which the design allowed them
  as the single exception. But they were everywhere, and the eye had to read
  past them to find the numbers.

The reference design replaces the sky with a flat, warm stone ground and
reserves the translucent material for the few elements that genuinely float:
the header, the inspector and the command bar. It also reduces the palette to
one accent for interactive elements, with red for blocked and amber for
thresholds. Everything else is neutral.

## Decision

**The ground is flat and quiet; the material is reserved for what floats.**

* The ground is a warm limestone in light mode and granite in dark mode,
  with inline contour lines at 4–6 % opacity. They are a drawing, not data:
  no elevation source is loaded, and no text sits on them.
* Glass — translucent surface, backdrop blur, light edge, soft shadow — is
  used on exactly three surfaces: the header, the inspector sheet and the
  command bar. Everything else sits on a near-opaque surface with a hairline
  and a very soft shadow.
* The sticky table head gets the glass *look* (a tinted, near-opaque strip)
  but no `backdrop-filter`. This is the one deliberate deviation from "glass
  for floating elements": it floats, but it floats over the log, and that is
  the one place where the blur costs without being seen.
* One accent colour (gentian blue) marks interactive elements, the logo and
  focus. Red is reserved for "blocked", amber for latency ≥ 250 ms and
  SERVFAIL. Green marks a healthy status and a cache answer, nothing else.
* The rest of B.6 continues to apply unchanged: the 8-px spacing, the
  typographic scale, one centred container, tabular numbers, monospace names,
  computed contrast, `prefers-reduced-motion` and
  `prefers-reduced-transparency`.

## Consequences

* `backdrop-filter` appears only on `.topbar`, `.sheet` and `.commandbar`.
  The test `glass_is_reserved_for_floating_surfaces` fails when a fourth
  surface gets it.
* The contrast test computes every text colour against the ground, the
  surface and the glass, in both modes — the ground now carries text, so it
  is a text background and not decoration.
* A log that scrolls under an opaque head still reads as a table; the blur
  adds nothing there and costs a repaint per row.
* B.6 describes the old material ("the fixed sky of four colour fields"). It
  needs new wording; this ADR is the reason the wording changes.

## Alternatives

* **Keep glass everywhere.** Consistent with ADR-0022, but the overview puts
  the blur over the busiest part of the page, and the hierarchy it once
  created is now carried by the shadow and the hairline at lower cost.
* **Remove glass entirely.** Cheaper still, but the header and the sheet do
  float over the content; a flat page would make them look pasted on rather
  than above.
* **Keep the four colour fields under opaque surfaces.** Nothing of them
  would be visible; a ground with a purpose is better than a hidden one.
