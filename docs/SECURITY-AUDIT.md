# Security audit

A manual audit of the entire codebase (`crates/alpendns/src/`), focused on
Rust-specific risks in network and parsing code: DNS packet parsing, upstream
handling, config parsing. Performed on 2026-08-31 on `main` (at `ad3d776`),
read-only — **no code changes** were made.

> **Reading the status lines.** This is a dated snapshot, so the file and line
> references below point at the tree as it was on 2026-08-31, not as it is now.
> Each finding carries a **Status** line recording what has happened to it
> since; that line is the part to trust for the current state.

## Result

**No critical vulnerability.** The hard rules from CLAUDE.md B.1 are actually
enforced by the workspace lints, not merely asserted. What was found were two
medium DoS vectors (a missing timeout and a missing bound at the two places
where external flow does **not** go through the DNS wire format) and three
low/hardening points.

The promise "every byte from the network is hostile" is structurally kept. The
most rewarding place for future audits is therefore not DNS parsing but the two
HTTP/TCP bodies the process reads itself.

## What was checked and found clean

These points were specifically looked for and **not** found — recorded so that a
later audit does not start from scratch:

- **`unsafe`:** appears nowhere (`unsafe_code = "forbid"`).
- **`panic!` / `unreachable!` / `todo!`:** nowhere in production code.
- **`unwrap()` / `expect()` on external input:** the only non-test `unwrap()`
  is at [ratelimit.rs:210](../crates/alpendns/src/ratelimit.rs#L210), marked
  with `#[expect(clippy::unwrap_used)]` and provably safe (`SHARDS` is a
  constant > 0). All other hits are in `#[cfg(test)]` blocks. The workspace
  lints (`unwrap_used = "deny"`, `panic = "deny"`, `indexing_slicing = "deny"`)
  and `-D warnings` in CI make this state enforced, not accidental.
- **Hand-written DNS parsing:** none. The wire format goes through
  `hickory-proto` ([ADR-0002](adr/0002-hickory-proto-statt-eigenem-parser.md)).
  Our own layer validates ID, message type, QNAME (case-insensitive), QTYPE and
  QCLASS against the question that was asked
  ([dns.rs](../crates/alpendns/src/dns.rs)); `format_error` and `encode_for_udp`
  manage without slice indexing.
- **Integer overflow in length/offset arithmetic:** `saturating_*`, `checked_*`
  or `try_from(...).unwrap_or(MAX)` throughout. No silent wrap found.
- **Config parsing fail-closed:** `serde(deny_unknown_fields)`, plaintext ban in
  the pool, mandatory `tls_name`, mandatory IP literal for upstreams (no
  DNS-dependent resolution). A typo leads to a startup error, not to unfiltered
  operation (B.1 rule 5).
- **XSS in the web UI:** ruled out. Domain names flow into the UI, but the
  script uses only `textContent`/`setAttribute`, never `innerHTML`/`outerHTML`/
  `insertAdjacentHTML` — pinned as a test in
  [ui.rs:551-557](../crates/alpendns/src/api/ui.rs#L551-L557). In addition the
  token is mandatory, compared in constant time
  ([api/mod.rs:180](../crates/alpendns/src/api/mod.rs#L180)).
- **Metric label injection:** ruled out. All label values come from config or
  closed enums; domain/client values as labels are forbidden by a test
  ([metrics.rs:711-788](../crates/alpendns/src/metrics.rs#L711-L788)).
- **Cache/policy races:** the cache is sharded (`Mutex<LruCache>` per shard),
  rarely changing state sits behind `ArcSwap`. The `Inflight` dedup releases the
  key and the `broadcast` signal in the correct order.
- **Rate limiting runs before parsing:**
  [server/mod.rs](../crates/alpendns/src/server/mod.rs) throttles on the basis
  of the peer IP before a single byte of the request is parsed.

## Findings

### Medium — TCP slowloris: missing timeout when reading the body

- **File:line:** [tcp.rs:86](../crates/alpendns/src/server/tcp.rs#L86)
- **Description:** reading the 2-byte length prefix has `IDLE_TIMEOUT` (10 s,
  [tcp.rs:74](../crates/alpendns/src/server/tcp.rs#L74)). The subsequent
  `stream.read_exact(&mut packet)` — up to 65,535 bytes — has **no** timeout. A
  client sends two bytes (`0xFF 0xFF`) and then holds the connection silent: the
  task hangs indefinitely and holds a `vec![0; 65535]` allocation.
- **Risk:** classic slowloris. There is neither a per-connection limit nor an
  overall limit for concurrent TCP connections (`tracker.spawn` per `accept`,
  without a semaphore). A handful of silent connections ties up tasks and
  memory — for a publicly listening resolver a single client suffices. For the
  pure LAN resolver it takes a compromised host on your own network, hence
  Medium rather than High.
- **Pattern:** the prefix timeout gives the impression that the connection is
  fully covered — which is exactly why the missing body timeout does not stand
  out at first glance.
- **Status: fixed** in `81708ae` (2026-09-16). The connection now has a body
  timeout and there is a per-source-IP cap (`MAX_PER_CLIENT`), an overall cap,
  and counters (`body_timeouts`, `at_capacity`) that make a rejected connection
  visible at all — see `crates/alpendns/src/server/tcp.rs` and
  [OPERATIONS.md](OPERATIONS.md) §4.

### Medium — blocklist download buffers without limit despite the 64 MB cap

- **File:line:** [source.rs:183](../crates/alpendns/src/filter/source.rs#L183)
  (the check only at
  [source.rs:187](../crates/alpendns/src/filter/source.rs#L187))
- **Description:** the size limit `MAX_LIST_BYTES` (64 MB) was checked **before**
  reading, and only via `content_length()`
  ([source.rs:171](../crates/alpendns/src/filter/source.rs#L171)). A response
  without `Content-Length` (chunked encoding) bypasses that check.
  `response.text().await` then buffers the **complete** body, and the real limit
  only takes effect afterwards.
- **Risk:** the limit is meant as protection against unbounded allocation and
  fails on exactly the response shape that does not announce itself. Exploitable
  only if the blocklist source is compromised or answers maliciously (the URLs
  are admin-configured, hence trusted) — therefore Medium. The fix would be a
  streaming `read_limited` for the body instead of `text()`.
- **Status: fixed** in `cb8f86a` (2026-09-19), refined in `05018cb`. The body is
  now read chunk by chunk against a running counter, and — beyond what this
  audit asked for — the limit is also applied after `from_utf8_lossy`, which can
  inflate a body up to threefold and would otherwise have written a cache file
  that `read_limited` could never read back.

### Low — TOCTOU in `read_limited`

- **File:line:** [source.rs:294-302](../crates/alpendns/src/filter/source.rs#L294-L302)
- **Description:** the `metadata.len()` check and `read_to_string` are two
  separate steps. Between them the file can grow or be swapped (symlink swap, or
  a write by a local user with rights on the cache directory); the cached
  fallback then reads more than 64 MB.
- **Risk:** requires local write access to the service user's cache directory;
  not reachable over the network. Defense in depth, hence Low.
- **Status: open.** `read_limited` still checks and reads in two steps
  (`crates/alpendns/src/filter/source.rs`). The fix in `cb8f86a` addressed the
  download path, not this cached-file path.

### Low — API token file briefly with default permissions

- **File:line:** [main.rs:800-807](../crates/alpendns/src/main.rs#L800-L807)
- **Description:** `std::fs::write(path, …)` creates the file with umask
  permissions (typically `0644`), and only afterwards is `0o600` set. During
  that window the freshly generated password is world-readable. Also: if the
  file already exists with loose permissions, `read_or_create_token` only reads
  it and does not tighten the permissions
  ([main.rs:782-787](../crates/alpendns/src/main.rs#L782-L787)).
- **Risk:** local attacker, tiny time window, but it is the API credential. The
  clean version would be `OpenOptions` with `mode(0o600)` (Unix) at creation
  time, so that no open window ever exists.
- **Status: open.** `crates/alpendns/src/main.rs` still writes and then chmods,
  and does not tighten pre-existing loose permissions.

### Context (not a code problem) — amplification via spoofed source IP

- **Observation:** the rate limiter keys on the source IP. With UDP the source
  IP is forgeable; the throttling therefore does **not** prevent an
  amplification reflection against a spoofed victim, it only caps the
  per-client volume. The response size is bounded by `udp_payload_size`.
- **Assessment:** an inherent property of a resolver, not a bug. The project
  knows it: `alpendns check` warns explicitly about public listeners, and B.5
  makes throttling mandatory. Recorded for completeness only, not a prioritized
  finding.

## Assessment

The two notable gaps do not lie in the feared class "panic/unsafe on input" but
in **missing timeouts/bounds at the two places where the process itself reads a
body that is not the DNS wire format**: the TCP body in
[tcp.rs](../crates/alpendns/src/server/tcp.rs) and the HTTP body of the
blocklists in [source.rs](../crates/alpendns/src/filter/source.rs). Both were
closable with limited effort, neither was critical; both have since been closed.
What remains open are the two Low findings, both of which require local access
to the host.
