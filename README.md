# AlpenDNS

A privacy-focused DNS server for Linux, in Rust. A **forwarding resolver**:
it accepts queries from the LAN, filters them against blocklists and policies,
and forwards them encrypted (DoT/DoH/DoQ) to upstreams. Recursion from the
root servers is deliberately not a goal.

**[Live demo →](https://florianpunz.github.io/alpendns/)** — the web UI with
recorded data, no install. Built from [`web/demo/`](web/demo/), which produces
a single self-contained HTML file.

![The AlpenDNS web UI](docs/images/ui.png)

> **Status.** Phases 1–9 are implemented; 1–7 are accepted, and the acceptance
> run for 8 and 9 is in progress in a real home network. Where that stands, with
> numbers and open gaps: [docs/ROADMAP.md](docs/ROADMAP.md).

## Quick start

```bash
cargo run -- -c config/alpendns.minimal.toml
dig @127.0.0.1 -p 5353 example.com

# Why was something blocked? Without a running server:
cargo run -- -c config/alpendns.minimal.toml policy test doubleclick.net

# Web UI: http://127.0.0.1:8053 — the token is in api.token_file
```

### As a service on a server

```bash
cargo install cargo-deb --locked
cargo deb -p alpendns
sudo apt install ./target/debian/alpendns_<version>_amd64.deb

dig @127.0.0.1 example.com
```

Afterwards the resolver runs as an unprivileged service — for now only on
loopback. To serve your own network, add the LAN address to
`/etc/alpendns/alpendns.toml` and run
`sudo alpendns -c /etc/alpendns/alpendns.toml check`. Everything else in
[docs/OPERATIONS.md](docs/OPERATIONS.md).

## Why another DNS server?

Pi-hole and AdGuard Home solve "blocking ads on the LAN" well. What they don't solve:

* **Your upstream still sees everything.** A single encrypted upstream replaces the nosy
  provider with a nosy vendor. AlpenDNS distributes queries deterministically across
  multiple upstreams (`split_by_zone`), so no single one sees your complete profile.
* **Blocking is a black box.** "Why doesn't this site work?" is, in most setups, a search
  through the log. AlpenDNS emits a decision chain for every answer: which list, which
  rule, which policy.
* **Filtering is purely list-based.** Lists only know what was already known yesterday.
  AlpenDNS additionally evaluates locally and without the cloud: DGA patterns, DNS
  tunneling, rebinding, typosquatting against domains that matter to you.
* **Logging is on or off.** AlpenDNS has an aggregating default with a k-anonymity
  threshold: you see patterns, but the box doesn't store who called what when.

The stated goal is not to replace Pi-hole, but to answer the questions Pi-hole leaves
open.

## What v1 can do

| | |
|---|---|
| Role | Forwarding resolver (no own recursion) |
| Client transports | UDP/53, TCP/53 |
| Upstream transports | DoT, DoH, DoQ; cleartext only for explicit internal zones |
| Filtering | Blocklists (hosts, domains, wildcard), allowlists, regex rules |
| Policies | per client (IP/subnet), time windows, temporary grants |
| Privacy | ECS stripping, padding, DNS cookies, 0x20, upstream splitting, DNSSEC validation, ODoH, aggregated logging |
| Heuristics | DGA, tunneling, rebinding, typosquatting, newly registered domains — all on `flag`, block nothing |
| Operations | systemd unit, .deb package, Prometheus metrics, web UI, per-client rate limiting, SIGHUP reload |

Encrypted listeners for clients (DoT/DoH/DoQ) and client identification via DoH path
tokens or mTLS are planned but not yet built — the policy currently distinguishes clients
by IP.

## What v1 is explicitly not

* **Not a recursive resolver.** AlpenDNS queries upstreams, not the root servers. The
  architecture keeps the slot open where recursion could later be hooked in
  ([ADR-0003](docs/adr/0003-forwarder-first.md)), but nothing is built for it in advance.
* **Not an authoritative name server.** If you want to host a zone, use Knot or NSD.
* **Not a DHCP server.** Pi-hole does that too; that's a different job.
* **Not a VPN replacement.** DNS privacy protects name resolution. The IP connection
  afterwards is still visible to your provider. See
  [docs/THREAT-MODEL.md](docs/THREAT-MODEL.md).

## Repository

```
CLAUDE.md                    Rules for coding agents in this repo
CONTRIBUTING.md              How to contribute
SECURITY.md                  How to report a vulnerability
LICENSE                      AGPL-3.0-or-later
config/                      Example, minimal and shipped configurations
crates/                      Rust workspace
crates/alpendns/fuzz/        Fuzz targets, corpus, known crashes
docs/ROADMAP.md              Phase plan with acceptance criteria
docs/ARCHITECTURE.md         Structure, request pipeline, data model
docs/FEATURES.md             Feature catalog with cost/benefit assessment
docs/TESTING.md              Test strategy, and the definition of done
docs/OPERATIONS.md           Installation, upgrade, backup, troubleshooting
docs/THREAT-MODEL.md         What this protects against — and what it doesn't
docs/adr/                    Architecture decisions with rationale
docs/images/                 Screenshots used by this file
packaging/                   systemd unit, Debian scripts, shipping configuration
scripts/                     Version bump used before every commit
web/                         Web UI; web/demo/ builds the self-contained demo page
```

## Development

Prerequisite: Rust stable (`rust-toolchain.toml` pins the appropriate version).

```bash
cargo build
cargo test --all-features
```

Before a change counts as finished, the **definition of done** has to be green —
four commands, listed in [docs/TESTING.md](docs/TESTING.md) along with the
manual smoke test, the fuzzing targets and the measurement runs.

## Contributing

[CONTRIBUTING.md](CONTRIBUTING.md) covers getting set up, the commit
conventions, and what this project expects of a change. Found a security
problem? [SECURITY.md](SECURITY.md) — please don't open a public issue for it.

## License

[AGPL-3.0-or-later](LICENSE). The license is a deliberate choice: even someone who only
offers AlpenDNS as a network service must disclose the source of their changes (AGPL
§13) — that fits the project's privacy motivation.
