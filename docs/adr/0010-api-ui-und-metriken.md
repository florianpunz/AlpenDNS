# ADR-0010: Two listeners, one token, and a UI without a build step

**Status:** accepted · **Date:** 2026-08-29

## Context

Phase 6 exposes three things that did not exist before: an HTTP API that shows
query names and reasons; a Prometheus endpoint; and a web UI. Each of them is a
new attack surface on a service whose purpose is restraint.

## Decisions

### Two listeners instead of one

The API runs on `api.listen` with a token, the metrics on `metrics.listen`
without. The reason is not convenience but that the two serve different things:

* The **API** shows names, clients and chains of reasoning. Without a token it
  would be a query log with an HTTP interface.
* The **metrics** contain no names by construction — labels exist only for
  resolver names and RCODEs, both from the configuration. A Prometheus scraper
  sends no bearer header; forcing it to would mean breaking the usual setup.

The configuration check refuses to put both on the same port. Otherwise the
tokenless endpoint would be the way around authentication.

**A domain name must never become a Prometheus label.** Prometheus keeps every
time series it has ever seen; a label with a domain would be a query log under a
different filename and with no expiry. A test checks this.

### The token is allowed in the URL for Server-Sent Events

`EventSource` in the browser cannot set headers. For `/api/events` the token is
therefore also permitted as a query parameter. That is a concession: it can end up
in a proxy log that way. The alternatives were worse — offering the live stream
without authentication (it shows names), or introducing a session-cookie mechanism
the server needs nowhere else.

The comparison runs in constant time. Bailing out at the first wrong character
reveals through the response time how far a guess got.

### The token is generated at startup if it is missing

Otherwise someone would have to create a file of random characters by hand before
the first start, just to see the UI. The file gets permissions 0600 — it is the
password.

### The UI lives in the binary, without a build step

Three files (`web/index.html`, `web/app.css`, `web/app.js`), compiled in via
`include_str!`. No npm, no bundler, no directory that has to exist at runtime —
and no way for the UI to belong to a different version than the server serving it.

A test checks that none of the three files refers to a foreign origin: no CDN
scripts, no external fonts, no `@import`. The mistake would otherwise be exactly
the one nobody notices as long as they are online themselves.

No framework. The page shows four numbers, two tables and one list; `textContent`
and an `EventSource` are enough for that. Foreign data is never inserted as markup
— a domain name from the network is text, not markup.

### The Prometheus format is produced by hand

A metrics library would introduce a registry in which every value is kept a second
time. The counters already live in the structures that increment them — cache,
pool, policy, query log. The text format is a handful of lines, and this way we
control exactly what goes out.

## Consequences

* Anyone who makes the API publicly reachable publishes their query log as soon as
  the token becomes known. The guidance stands: both listeners on localhost,
  access via SSH tunnel or reverse proxy. The defaults are set accordingly, and
  both endpoints are **off** by default.
* The UI is tied to the server's language (German) and to its layout.
  A translation would be a project of its own.
* `include_str!` means: a change to the UI requires a rebuild. For a project that
  ships a single binary, that is the right trade.
* The live stream costs nothing as long as nobody is listening: the query log
  checks the number of receivers before it builds an event.
