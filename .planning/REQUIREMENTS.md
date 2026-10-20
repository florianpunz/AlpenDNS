# Requirements: AlpenDNS

**Defined:** 2026-10-20
**Core Value:** Queries are filtered on the operator's own machine and forwarded
encrypted: no query name leaves the process unless the configured logging mode
explicitly allows it.

Requirements are derived only from the ADR decisions in `docs/adr/` and the
documented acceptance criteria in `docs/ROADMAP.md`. The brownfield state is
reflected in the checkboxes and traceability: phases 0 to 7 are complete, phase 8
and 9 implementation is complete with acceptance still running.

## v1 Requirements

### Setup and quality gate

- [x] **SETUP-01**: A workspace exists in which `cargo build` and `cargo test` run.
- [x] **SETUP-02**: `cargo fmt --all --check` and `cargo clippy --all-targets --all-features -- -D warnings` are green.
- [x] **SETUP-03**: `cargo deny check` runs with `deny.toml`, and CI executes the Definition of Done on every push.
- [x] **SETUP-04**: A change counts as done only when the four commands in `docs/TESTING.md` pass and it brings a new test or a line of justification for why it needs none.

### Core forwarding

- [x] **CORE-01**: A client receives correct DNS answers over UDP and TCP on a configurable port while the server forwards to one configured upstream.
- [x] **CORE-02**: Every upstream answer is checked against the question (ID, QNAME case-insensitive, QTYPE, QCLASS); a mismatched answer is discarded.
- [x] **CORE-03**: TCP uses the 2-byte length prefix, and a UDP answer larger than `udp_payload_size` sets the TC flag while TCP returns the full answer.
- [x] **CORE-04**: Configuration is loaded from TOML with `deny_unknown_fields`; an unknown key aborts startup with a clear message.
- [x] **CORE-05**: SIGTERM/SIGINT finish in-flight queries, then the process exits 0.
- [x] **CORE-06**: v1 resolves only by forwarding; recursion is not built and resolution stays behind `ResolveBackend` with exactly one implementation.
- [x] **CORE-07**: DNS messages are parsed and serialized only by `hickory-proto`; no parser of our own sits on the production path.
- [x] **DEV-01**: The TSIG panic in `hickory-proto 0.26.1` is documented as a known crash, the delivery configuration disables overflow checks, the case stays in `fuzz/known-crashes/`, and `0.27` is awaited.

### Cache

- [x] **CACHE-01**: Repeated queries are answered from memory by a key of lowercase name, QType and QClass; the upstream does not see them again.
- [x] **CACHE-02**: TTLs are clamped to `[min_ttl, max_ttl]`, negative answers are cached separately (RFC 2308), and no TTL grows beyond its value at insert.
- [x] **CACHE-03**: The cache enforces `max_entries` with LRU eviction, and memory does not grow linearly with new names.
- [x] **CACHE-04**: Simultaneous identical queries are coalesced; 100 parallel queries produce exactly one upstream request.
- [x] **CACHE-05**: Serve-stale (RFC 8767) answers from an expired entry when the upstream is down, and prefetch refreshes an entry at 85% of its TTL.
- [x] **CACHE-06**: Hits, stale hits and misses are visible as a metric in the periodic log line and on shutdown, without names.
- [x] **CACHE-07**: Failed resolutions are remembered for 5 s, doubling to 60 s, capped at 1024 entries; a stale answer wins over the failure memory (ADR-0027).

### Encrypted upstreams

- [x] **UPSTREAM-01**: Non-`forward_zone` traffic leaves only over DoT, DoH or DoQ; `udp://` in an `upstream_pool` is a startup error.
- [x] **UPSTREAM-02**: The transports come from `hickory-net`, not `hickory-resolver`; connection lifecycle, selection, failure detection, cache and privacy stay in our code.
- [x] **UPSTREAM-03**: `split_by_zone` is the only strategy and exactly one upstream is queried per request; `fastest`, `round_robin` and `fanout` are recognized but rejected with a reason.
- [x] **UPSTREAM-04**: Passive health tracking skips an upstream after three failures, lets it recover after its block, and still attempts resolution when all upstreams are down.
- [x] **UPSTREAM-05**: `forward_zone` resolves internal names in cleartext against a LAN server, while a more specific local or forward zone wins regardless of type (ADR-0029).
- [x] **UPSTREAM-06**: ECS is stripped; EDNS padding, DNS cookies and 0x20 are applied per pool, and 0x20 is switched off automatically when an upstream rejects it.
- [x] **UPSTREAM-07**: Every transport returns the client's query ID and question unchanged.

### Blocklists and filtering

- [x] **FILTER-01**: The `hosts` parser accepts comments, CRLF, IPv6 lines and garbage lines without failing the list.
- [x] **FILTER-02**: The `domains` and `wildcard` parsers normalize leading dots and `*.` correctly.
- [x] **FILTER-03**: Adblock and RPZ formats are removed; their old names are recognized through `Deserialize` with a reason pointing to `wildcard` and `hosts`.
- [x] **FILTER-04**: Matching is a `HashMap` suffix lookup that returns a `RuleRef`; wildcard semantics hold, and `notexample.com` never matches because of `example.com`.
- [x] **FILTER-05**: An allowlist takes precedence over blocklists, so a domain on both is let through.
- [x] **FILTER-06**: Block answers are synthesized as `nxdomain` (default), `zero_ip` or `sinkhole`; `refused` is removed and no mode produces REFUSED.
- [x] **FILTER-07**: Lists download with ETag/If-Modified-Since and an on-disk cache; a 304 is not reprocessed, and updates swap atomically without error answers or a latency spike.
- [x] **FILTER-08**: A first start with an unreachable list aborts, a later outage keeps the last cached version, and 2 million entries load with p99 cache-hit latency under 1 ms and documented RSS.

### Clients and policies

- [x] **POLICY-01**: Clients are identified by IP/subnet, and two source IPs can receive different verdicts for the same name.
- [x] **POLICY-02**: A policy model covers lists, allowlists, actions and evaluation order, with schedule windows evaluated on an injectable clock.
- [x] **POLICY-03**: The decision trace is complete for every evaluation; a match returns a `RuleRef` naming the list and line, not a bare boolean.
- [x] **POLICY-04**: Temporary exemptions with a TTL can be created through the API; the block returns when the TTL expires.
- [x] **POLICY-05**: Per-policy regex rules enforce length and runtime limits and cannot be driven into catastrophic backtracking.
- [x] **POLICY-06**: `alpendns policy test <domain> --client <name>` and `/api/explain` share one evaluator and print the verdict plus the full chain without reading the log.

### Visibility

- [x] **OBS-01**: The token-protected HTTP API serves status, statistics, lists, policies, temporary exemptions, recent queries, top names, history, explain and flagged queries; requests without a token get 401.
- [x] **OBS-02**: A tokenless Prometheus endpoint exposes counters for queries, blocks, cache hit rate, upstream RTT and errors; label keys come from a closed allowlist and no domain name is ever a label.
- [x] **OBS-03**: Logging has four modes (`none`, `aggregate` default, `ring`, `full`); in `none` and `aggregate` no query name survives, and `aggregate` uses exact salted-hash counters with `aggregate_k` (default 5).
- [x] **OBS-04**: The ring buffer drops entries older than `ring_seconds` and keeps memory bounded.
- [x] **OBS-05**: The SSE live stream sends at most 25 messages per second and reports the omitted ones as `skipped`.
- [x] **OBS-06**: The web UI is served by the server itself from `include_str!` assets and works with the network disconnected, with no third-party requests.
- [x] **OBS-07**: The start page answers "Is it running?", "What was blocked?" and "Why?" without a click.
- [x] **OBS-08**: A blocked name's view shows list, rule, policy and timestamp from the decision trace.
- [x] **OBS-09**: A 24-hour counter-only history feeds `/api/history` and `/api/top`; names appear only above the k threshold, with below-threshold sums reported beside them.
- [x] **OBS-10**: Two listeners (API and metrics) are separate and the same port is refused; the token is generated with mode 0600 and allowed as an SSE query parameter; a domain name is never a Prometheus label.
- [x] **OBS-11**: The UI follows the glass surface design: four semantic colours plus `--brand`, a greyscale ramp for areas, one 1400 px container, 8-based spacing, one glass container, light/dark via `data-theme`, and every text colour computed against every field at 4.5:1 or better.

### Privacy build-out

- [x] **PRIVACY-01**: `split_by_zone` derives the registrable domain from the compiled-in PSL, rotates its seed every 24 h by default, and keeps each upstream within 5% over a 10,000-domain test.
- [x] **PRIVACY-02**: The per-upstream query share, transport mix and privacy counters (ECS removed, padding, 0x20, cookies) are visible through the API and UI.
- [x] **PRIVACY-03**: DNSSEC is validated locally from root keys, on by default; valid chains pass, invalid or missing signatures yield SERVFAIL; signatures go only to DO clients; the CD bit does not disable validation; `Bogus` is terminal and not counted as an upstream failure.
- [x] **PRIVACY-04**: Oblivious DoH (RFC 9230) works as a client, off by default; the target key is fetched directly once, every pool resolver must be `doh://`, an `http://` proxy is a startup error, DNSSEC applies, and query labels are not visible to the proxy.
- [x] **PRIVACY-05**: Domains below the k-anonymity threshold appear in no aggregate API response, while the sum of below-threshold traffic is still reported.
- [x] **PRIVACY-06**: An automated test drives a query through the process and greps all output for the query name; it is green in `none` and `aggregate` and runs permanently in CI.
- [x] **DEV-02**: The false `Bogus` for a CNAME crossing into a NODATA answer is accepted and named as a known false-alarm source in `docs/OPERATIONS.md`; DNSSEC stays on and the state ends with hickory PR #3595 or an equivalent change.

### Detection heuristics

- [x] **DETECT-01**: A detector framework provides `NameDetector` and `AnswerDetector`, a `Finding` with score 0.0 to 1.0 and a human-readable reason, and modes `off`/`log`/`flag`/`block`; a detector's result lands in the trace.
- [x] **DETECT-02**: Rebinding protection flags only addresses where a local device can sit (loopback, RFC 1918, link-local, broadcast, CGNAT, 198.18/15); `forward_zone` names are exempt.
- [x] **DETECT-03**: Tunneling detection scores zones over a time window and detects the iodine/dnscat corpus while staying under 0.1% false positives on the top-100k corpus.
- [x] **DETECT-04**: DGA detection uses a 3-gram model compiled into the binary and stays under 0.1% false positives on the top-100k corpus, with detection rates documented per family.
- [x] **DETECT-05**: Typosquat detection combines Damerau-Levenshtein, Unicode confusables and Punycode resolution; constructed variants are detected and the originals are never flagged.
- [x] **DETECT-06**: NRD awareness reads a local file and scores domains by age; a missing file is not a startup error.
- [x] **DETECT-07**: All five detectors default to `flag` and block nothing; thresholds live in code and are measured.
- [x] **DETECT-08**: The "Flagged" UI panel shows detector, score and reason, with one click to allow and one to block.
- [ ] **DETECT-09**: Acceptance — one week of real-network operation with all detectors on `flag`, with the false-positive list reviewed; only after that review may a detector default change to `block`.

### Operation and packaging

- [x] **OPS-01**: The systemd unit runs as an unprivileged user with `CAP_NET_BIND_SERVICE` and listens on port 53.
- [x] **OPS-02**: Hardening directives are in place and `systemd-analyze security alpendns` reports under 3.0.
- [x] **OPS-03**: `alpendns check` runs as `ExecStartPre`, checks config, blueprint and writable directories, needs no network, and prevents startup on a broken config while the old instance keeps running.
- [x] **OPS-04**: The `.deb` builds with `cargo deb -p alpendns`, creates the system user, ships `/etc/alpendns/alpendns.toml` as a conffile, uses systemd `StateDirectory=`, and starts after installation on Debian/Ubuntu.
- [x] **OPS-05**: Per-client rate limiting uses 16 sharded token buckets, aggregates IPv6 to /64, is on by default, and drops over-limit queries without affecting other clients.
- [x] **OPS-06**: The shipped configuration listens on loopback only, `0.0.0.0` counts as public, and `alpendns check` reports a listener that reaches beyond the local network.
- [x] **OPS-07**: `docs/BENCHMARKS.md` records queries/s, p99 latency and RSS for the release build, plus the cost of throttling.
- [x] **OPS-08**: `docs/OPERATIONS.md` covers installation, release to the LAN, upgrade, backup, troubleshooting, hardening directives, observation week and uninstall, and ships in the package.
- [ ] **OPS-09**: The practical test runs for a week with the server as the only LAN resolver without complaints, and anything notable is documented as a false-alarm list or a configuration change.
- [ ] **OPS-10**: Acceptance — fresh Debian VM, `apt install ./alpendns.deb`, a working hardened resolver with no manual follow-up work.

## v2 Requirements

None defined. Phase 10 items are optional and discardable, not deferred
commitments; they are tracked in `docs/ROADMAP.md` and `docs/TODOS.md`.

## Out of Scope

| Feature | Reason |
|---------|--------|
| Recursion from the root servers in v1 | ADR-0003: forwarder first; the `ResolveBackend` trait keeps the door open without pre-building it |
| Cleartext DNS outbound (outside LAN `forward_zone`) | B.1 rule 7; `udp://` in an upstream pool is a startup error |
| Telemetry, update checks, crash reporting | B.1 rule 4; the process contacts only configured destinations |
| Adblock and RPZ blocklist parsers | ADR-0014: removed; `hosts`, `domains` and `wildcard` cover the need |
| Block mode `refused` | ADR-0013: removed; no block mode produces REFUSED |
| Upstream strategies `fastest`, `round_robin`, `fanout` | ADR-0011 and ADR-0012: `split_by_zone` only, exactly one upstream |
| Bloom filter, label reversal, inverted trie | ADR-0008: measurements reject them; memory is the reversal criterion |
| Web UI build step, external fonts, CDN resources, client-side analytics | ADR-0010 and B.6: the UI is embedded, self-served and offline |
| Automatic config migration, config version field | ADR-0024: `serde(alias)` plus deprecation warning instead |
| Detector default change to `block` without observation and review | ADR-0019, B.8; ADR-0025 and ADR-0026 remain proposed |

## Traceability

| Requirement | Phase | Status |
|-------------|-------|--------|
| SETUP-01 | Phase 0 | Complete |
| SETUP-02 | Phase 0 | Complete |
| SETUP-03 | Phase 0 | Complete |
| SETUP-04 | Phase 0 | Complete |
| CORE-01 | Phase 1 | Complete |
| CORE-02 | Phase 1 | Complete |
| CORE-03 | Phase 1 | Complete |
| CORE-04 | Phase 1 | Complete |
| CORE-05 | Phase 1 | Complete |
| CORE-06 | Phase 1 | Complete |
| CORE-07 | Phase 1 | Complete |
| DEV-01 | Phase 1 | Complete |
| CACHE-01 | Phase 2 | Complete |
| CACHE-02 | Phase 2 | Complete |
| CACHE-03 | Phase 2 | Complete |
| CACHE-04 | Phase 2 | Complete |
| CACHE-05 | Phase 2 | Complete |
| CACHE-06 | Phase 2 | Complete |
| CACHE-07 | Phase 2 | Complete |
| UPSTREAM-01 | Phase 3 | Complete |
| UPSTREAM-02 | Phase 3 | Complete |
| UPSTREAM-03 | Phase 3 | Complete |
| UPSTREAM-04 | Phase 3 | Complete |
| UPSTREAM-05 | Phase 3 | Complete |
| UPSTREAM-06 | Phase 3 | Complete |
| UPSTREAM-07 | Phase 3 | Complete |
| FILTER-01 | Phase 4 | Complete |
| FILTER-02 | Phase 4 | Complete |
| FILTER-03 | Phase 4 | Complete |
| FILTER-04 | Phase 4 | Complete |
| FILTER-05 | Phase 4 | Complete |
| FILTER-06 | Phase 4 | Complete |
| FILTER-07 | Phase 4 | Complete |
| FILTER-08 | Phase 4 | Complete |
| POLICY-01 | Phase 5 | Complete |
| POLICY-02 | Phase 5 | Complete |
| POLICY-03 | Phase 5 | Complete |
| POLICY-04 | Phase 5 | Complete |
| POLICY-05 | Phase 5 | Complete |
| POLICY-06 | Phase 5 | Complete |
| OBS-01 | Phase 6 | Complete |
| OBS-02 | Phase 6 | Complete |
| OBS-03 | Phase 6 | Complete |
| OBS-04 | Phase 6 | Complete |
| OBS-05 | Phase 6 | Complete |
| OBS-06 | Phase 6 | Complete |
| OBS-07 | Phase 6 | Complete |
| OBS-08 | Phase 6 | Complete |
| OBS-09 | Phase 6 | Complete |
| OBS-10 | Phase 6 | Complete |
| OBS-11 | Phase 6 | Complete |
| PRIVACY-01 | Phase 7 | Complete |
| PRIVACY-02 | Phase 7 | Complete |
| PRIVACY-03 | Phase 7 | Complete |
| PRIVACY-04 | Phase 7 | Complete |
| PRIVACY-05 | Phase 7 | Complete |
| PRIVACY-06 | Phase 7 | Complete |
| DEV-02 | Phase 7 | Complete |
| DETECT-01 | Phase 8 | Complete |
| DETECT-02 | Phase 8 | Complete |
| DETECT-03 | Phase 8 | Complete |
| DETECT-04 | Phase 8 | Complete |
| DETECT-05 | Phase 8 | Complete |
| DETECT-06 | Phase 8 | Complete |
| DETECT-07 | Phase 8 | Complete |
| DETECT-08 | Phase 8 | Complete |
| DETECT-09 | Phase 8 | In Progress |
| OPS-01 | Phase 9 | Complete |
| OPS-02 | Phase 9 | Complete |
| OPS-03 | Phase 9 | Complete |
| OPS-04 | Phase 9 | Complete |
| OPS-05 | Phase 9 | Complete |
| OPS-06 | Phase 9 | Complete |
| OPS-07 | Phase 9 | Complete |
| OPS-08 | Phase 9 | Complete |
| OPS-09 | Phase 9 | In Progress |
| OPS-10 | Phase 9 | Pending |

**Coverage:**
- v1 requirements: 77 total
- Mapped to phases: 77
- Unmapped: 0
- Phase 10 has no requirements by design: it is an optional, discardable
  stockpile, not a commitment.

---
*Requirements defined: 2026-10-20*
*Last updated: 2026-10-20 after brownfield ingest and roadmap creation*
