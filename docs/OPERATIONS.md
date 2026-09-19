# Operations

Installation, upgrade, backup, troubleshooting. This file is written so that
someone else can install from it without asking — that is the acceptance
criterion of phase 9, step 8.

Everything here applies to Debian 12/13 and Ubuntu 24.04 and up. AlpenDNS is a
Linux program; other systems are not a target.

> **A note on languages.** The prose is English, but the program's own output is
> not: log messages and `alpendns check` output are German. Wherever this
> document quotes such output or greps for it, the string is reproduced
> verbatim and must stay German to match.

---

## 1. Installation

### Building the package

On a machine with a Rust toolchain, once:

```bash
cargo install cargo-deb --locked
```

Then in the repository:

```bash
cargo deb -p alpendns
# → target/debian/alpendns_<version>_amd64.deb
```

`cargo deb` builds with `--release` itself. The package contains a binary
statically linked against `ring`/`rustls`; an OpenSSL version on the target
system does not matter.

### Releases and versions

The `push` to `main` builds the package in GitHub Actions and publishes it as a
GitHub release — for `amd64` and `arm64`. The version number is bumped **before
the commit**, not by CI: `scripts/bump-version.sh major|minor` before committing
(`major` for a breaking change, otherwise `minor`; there is no patch — every
commit is a new version). The coding agent does this automatically on every
commit; anyone committing by hand calls the script themselves. If the rule is
broken and the version already exists as a tag in the repo, CI aborts with a
clear message rather than publishing the same number a second time.

The package can still be built locally, as described above.

### Installing the package

```bash
sudo apt install ./alpendns_<version>_amd64.deb
```

The package creates:

| Path | What | Owned by |
|---|---|---|
| `/usr/bin/alpendns` | The program | root |
| `/etc/alpendns/alpendns.toml` | Configuration (conffile) | root |
| `/lib/systemd/system/alpendns.service` | The unit | root |
| `/var/lib/alpendns/` | API token, NRD file | `alpendns` |
| `/var/cache/alpendns/` | Downloaded blocklists | `alpendns` |
| `/var/log/alpendns/` | Query log, only in `full` mode | `alpendns` |
| `/usr/share/doc/alpendns/` | This manual, example configuration | root |

The three directories under `/var` are created by **systemd** at start
(`StateDirectory=`, `CacheDirectory=`, `LogsDirectory=`), not by the package.
That way there is exactly one place that sets permissions, and an upgrade cannot
break them.

The service user `alpendns` is a system user without a login shell and without a
home directory.

### After installation

Freshly installed, the server listens **on loopback only** — it is not
reachable from outside. That is deliberate: a resolver listening to the internet
unasked is an amplification reflector.

Checking that it answers:

```bash
dig @127.0.0.1 example.com          # should return an address
dig @127.0.0.1 doubleclick.net      # should return NXDOMAIN
systemctl status alpendns
```

### Opening it up to your own network

One change, in the first block of `/etc/alpendns/alpendns.toml`:

```toml
[server]
listen_udp = ["127.0.0.1:53", "192.168.1.10:53"]
listen_tcp = ["127.0.0.1:53", "192.168.1.10:53"]
```

Enter this machine's actual LAN address, **not** `0.0.0.0`: the wildcard also
binds to an interface that may be facing the internet tomorrow.

Then:

```bash
sudo alpendns -c /etc/alpendns/alpendns.toml check
sudo systemctl restart alpendns
```

`check` validates the configuration and the directories without starting the
server, and says at the end whether a listener reaches beyond your own network.
The same check runs as `ExecStartPre` before every start: if the file is broken,
the new process does not start at all, and on a `restart` the service stays down
and says why — instead of ending up in a restart loop.

Finally, switch the clients over: point the DHCP DNS server in your router at
this machine's address.

### The interface

The web UI shows names and therefore listens on loopback only. From another
machine, via an SSH tunnel:

```bash
ssh -L 8053:127.0.0.1:8053 <server>
# then http://127.0.0.1:8053 in the browser
```

The page asks for the token on first load:

```bash
sudo cat /var/lib/alpendns/api.token
```

Anyone who makes the UI reachable without a tunnel publishes their query log as
soon as the token becomes known. The intended route is the tunnel, or a reverse
proxy with authentication of its own.

The interface follows the system colour scheme. The button at the top right of
the header (sun in light mode, moon in dark) overrides it; the choice is stored
in the browser. Without JavaScript it stays in light mode. A
`prefers-reduced-transparency` in the system turns the blur off — the page stays
readable, just without the material.

---

## 2. Upgrade

```bash
cargo deb -p alpendns                            # build the new package
sudo apt install ./alpendns_<version>_amd64.deb  # install it
```

What happens:

* `/etc/alpendns/alpendns.toml` is a **conffile**. An edited file is not
  overwritten; `dpkg` asks if both sides have changed.
* The service is restarted after the upgrade (`restart-after-upgrade`).
  `alpendns check` runs beforehand — a configuration the new version does not
  understand prevents the start, and the old instance keeps running until it is
  shut down as planned.
* Blocklist cache and API token are left in place.

**Before an upgrade, a look into `docs/ROADMAP.md` pays off:** if a phase
removed configuration keys, the start says so with a reason — but it says it
only at start. `alpendns -c /etc/alpendns/alpendns.toml check` with the **new**
binary before the restart is the faster answer.

### Rolling back

```bash
sudo apt install ./alpendns_<old-version>_amd64.deb --allow-downgrades
```

The state under `/var` is compatible across versions: blocklist cache and token
are files without a schema.

---

## 3. Backup

Exactly one file needs backing up:

```
/etc/alpendns/alpendns.toml
```

Everything else is recoverable: blocklists are reloaded, the API token is
regenerated, the cache is ephemeral anyway. If you want to keep the token (so
the UI in the browser does not ask again), include
`/var/lib/alpendns/api.token`.

```bash
sudo tar czf alpendns-backup-$(date +%F).tar.gz \
    /etc/alpendns/alpendns.toml \
    /var/lib/alpendns/api.token
```

Restoring: copy the file back, `alpendns check`, `systemctl restart`.

There is deliberately **no database**: in `aggregate` mode the query log exists
only as counters, in `ring` mode only in RAM. Backing that up would contradict
the project's purpose.

---

## 4. Troubleshooting

Always start with:

```bash
systemctl status alpendns
journalctl -u alpendns -n 100 --no-pager
```

### The service does not start

`alpendns check` says why in almost every case:

```bash
sudo -u alpendns alpendns -c /etc/alpendns/alpendns.toml check
```

The `sudo -u alpendns` matters: as root you see directories as writable that are
not writable for the service.

| Message | Meaning |
|---|---|
| `unknown field ...` | Typo in a key. Unknown keys are a startup error, not a warning — otherwise someone would sit unfiltered on the internet and not notice. |
| `Blocklisten konnten beim Start nicht geladen werden` | No network on the very first start, and nothing in the cache yet. Later, an outage is not critical: the cached version stays in force. |
| `Listener konnten nicht geöffnet werden` | Port 53 is taken — see below. |
| `... kann nicht geschrieben werden` | Permissions under `/var` are off. `systemctl restart alpendns` resets them, because systemd manages the directories. |

### Port 53 is taken

Usually `systemd-resolved` (Ubuntu, some Debian installations). To see who is
listening:

```bash
sudo ss -lunp sport = :53
```

`systemd-resolved` normally takes `127.0.0.53:53` and does not get in the way.
If it takes `0.0.0.0:53`, its stub listener has to go:

```bash
sudo mkdir -p /etc/systemd/resolved.conf.d
printf '[Resolve]\nDNSStubListener=no\n' | \
    sudo tee /etc/systemd/resolved.conf.d/alpendns.conf
sudo systemctl restart systemd-resolved
sudo systemctl restart alpendns
```

If the machine itself should resolve through AlpenDNS, afterwards point
`/etc/resolv.conf` or the `DNS=` entry in `resolved.conf` at `127.0.0.1`.

### Names do not resolve although the service is running

The order to look in:

```bash
# 1. Does the server answer at all?
dig @127.0.0.1 example.com

# 2. Is the name blocked — and by which rule?
sudo alpendns -c /etc/alpendns/alpendns.toml policy test example.com

# 3. Are the upstreams getting through?
journalctl -u alpendns | grep -i upstream
```

`policy test` answers "why was this blocked?" without looking into the log and
without the server having to run. It shows the same reasoning the UI shows under
"Why?".

A name that is blocked wrongly belongs on an allowlist, or as a temporary grant
in the UI (the "Allow" button).

### A device stops getting answers

Possibly the rate limiter. The counter is in the log and in the metrics:

```bash
# The message is at debug level — see "More in the log" below.
journalctl -u alpendns | grep -i "Überschreitung des Limits"
curl -s localhost:9153/metrics | grep rate_limit   # only if [metrics] is on
```

If `alpendns_rate_limited_total` rises while a device complains: the limit is
100 queries per second per client with a burst of 200. That is a great deal for
a single device — anyone who reaches it anyway either has a loop in the network
or a device standing in for several hosts (another resolver, a NAT in front). In
the second case the values belong higher:

```toml
[server.rate_limit]
per_client_qps = 500
burst = 1000
```

Turning it off (`enabled = false`) is only correct as long as the server listens
on loopback exclusively.

### A device stops getting connections over TCP

Over TCP there are two additional ceilings: **8 concurrent connections per
source IP** and **64 in total** (63 served concurrently, one reserved for the
next accept). Both are constants in the code
(`crates/alpendns/src/server/tcp.rs`) and not configurable.

They are visible only in the counters — a rejected connection gets no answer,
and nothing about it is written to the log:

```bash
curl -s localhost:9153/metrics | grep alpendns_tcp   # only if [metrics] is on
```

| Metric | Meaning | What to do |
|---|---|---|
| `alpendns_tcp_connections_rejected_total` | A source IP wanted more than 8 concurrent connections. | The server is not the problem, the device behind it is: either a client with a connection leak or a NAT with several devices behind it. Find the device, do not raise the ceiling — no healthy device needs 8 concurrent TCP connections for DNS. |
| `alpendns_tcp_connections_at_capacity_total` | All 64 slots were taken, the accept had to wait. | A sign that the ceiling holds, not an error. If the counter rises permanently, the LAN is larger than a household — then the number in the code belongs higher. |
| `alpendns_tcp_body_timeouts_total` | A connection sent its length prefix and then went silent. | It is closed after five seconds. Individual hits are harmless (an aborted client); if the counter grows, a device is deliberately holding connections open. |

A client that no longer gets an answer over TCP but still resolves over UDP is
exactly this case: `dig +notcp` works, `dig +tcp` does not.

### Seeing everything at once

```bash
sudo systemctl show alpendns -p MainPID -p User   # is it running unprivileged?
systemd-analyze security alpendns                 # are the barriers active?
curl -s -H "Authorization: Bearer $(sudo cat /var/lib/alpendns/api.token)" \
     localhost:8053/api/status | head -40
```

### More in the log

The log level comes from `RUST_LOG`:

```bash
sudo systemctl edit alpendns
# [Service]
# Environment=RUST_LOG=debug
sudo systemctl restart alpendns
```

`debug` shows, among other things, which client address was throttled. **Query
names are not there either** — those go exclusively through the logging layer,
and that follows `privacy.logging.mode`. Remove it again after troubleshooting:
`sudo systemctl revert alpendns`.

---

## 5. What the hardening directives mean

`systemd-analyze security alpendns` computes the unit; the score is **1.5**
(lower is better, under 3.0 was required). What remains is what a resolver
inherently needs: network access and port 53.

| Directive | Against what |
|---|---|
| `User=alpendns` + `AmbientCapabilities=CAP_NET_BIND_SERVICE` | The process was never root. Port 53 comes from a single capability, not from omnipotence. |
| `NoNewPrivileges=yes` | No way back up, not even through an SUID program. |
| `ProtectSystem=strict` + `ProtectHome=yes` | The entire filesystem is read-only, except the three directories under `/var` that systemd itself releases. |
| `PrivateTmp` / `PrivateDevices` | No shared `/tmp`, no devices beyond the harmless ones. |
| `MemoryDenyWriteExecute=yes` | No memory that is writable *and* executable — the usual runway for shellcode. |
| `SystemCallFilter=@system-service` + `~@privileged @resources` | System calls a service does not need do not exist for it. |
| `RestrictAddressFamilies=AF_INET AF_INET6 AF_UNIX AF_NETLINK` | No raw sockets, no packet sniffing. `AF_NETLINK` is in there because glibc needs it when resolving a hostname — without the line the blocklist download fails, and it fails silently. |
| `ProtectKernelTunables/Modules/Logs`, `ProtectClock`, `LockPersonality` | The service cannot change anything about the system. |

The unit lives in the repository at `packaging/systemd/alpendns.service` and is
commented.

---

## 6. Observation week

The acceptance of phase 8 and phase 9 is a multi-day run in the real network. It
cannot be replaced by a test, and it has an order.

**Preparation.** Observation needs names, and those only survive the week if
they go to disk. The `ring` mode is **not** enough for that: it holds the last
`ring_seconds` in RAM, nothing more. The "Auffällig" panel in the UI reads
exactly that buffer — a false positive nobody notes down the same day is gone
afterwards. For a run lasting a week, that is the wrong basis.

```toml
[privacy.logging]
# Necessary so the week can be evaluated at the end. The price is in ADR-0004:
# every resolved name lies on disk for the duration of the observation.
mode = "full"
```

Set it back after the evaluation.

Also enter your own domains into the typosquat guard (bank, government agency,
employer), otherwise it does nothing:

```toml
[detection]
typosquat = { action = "flag", threshold = 0.85, protect = ["meine-bank.at"] }
```

Whether a detector can find anything *at all* is answered beforehand by a look
into `alpendns check`: behind an enabled detector that lacks its basis, the
reason appears in parentheses — `typosquat flag (ohne protect: findet nichts)`,
`nrd flag (/var/lib/alpendns/nrd.txt fehlt: läuft leer)`.

All five detectors are on `flag`. **They stay on `flag` the whole week.** They
report, they do not block.

**During the week.** Look into the UI once a day, "Auffällig" panel:

* What is in there that is obviously harmless? That is a false positive. Note it
  down — domain, detector, score, what the device was doing at the time.
* Reputation services and antivirus products look like a DNS tunnel by
  construction. Their zones belong in `detection.tunneling.allow_zones`.
* Split-horizon DNS and device web interfaces under a real name trigger the
  rebinding protection. Their zones belong in `detection.rebinding.allow_zones`.
* If someone in the household complains that something does not work:
  `alpendns policy test <domain>` says whether AlpenDNS was the cause. As a rule
  it was the blocklist and not a detector — the detectors do not block.

Let run alongside, whatever comes for free:

```bash
# Does it stay up? A restart shows up as a new start time.
systemctl show alpendns -p ActiveEnterTimestamp -p NRestarts

# Counters over the week, if [metrics] is on
curl -s localhost:9153/metrics | grep -E 'queries_total|cache_hit_ratio|rate_limited|detections'
```

**At the end of the week.** Numbers first, then the decision. `packaging/abnahme.py`
reads the query log and counts per detector:

```bash
# Second argument optional — this also evaluates a saved copy
python3 abnahme.py 2026-09-16T16:24 | tee abnahme-periode.txt
```

It prints queries and **distinct names**, then per detector the findings and the
most frequent names with score. Both belong to the assessment: 7 findings across
4,025 names is something different from 7 across 40,000.

Then go through the false positive list and decide, per detector individually:

* No false positives over a week of real traffic → this detector may go to
  `action = "block"`. One at a time, not all together.
* **A detector that never fired is not thereby assessed.** "No false positives"
  means only that nothing happened — for `block` there is no evidence that it
  fires correctly at all. Either run a controlled test or leave it on `flag`.
* **A detector that ran empty is likewise not assessed.** An empty `protect`
  list or a missing `nrd.txt` produces the same zero as a clean run — just
  without a basis.
* False positives that can be settled via `allow_zones` → enter them, observe
  another week.
* False positives that cannot be settled → the detector stays on `flag`. That is
  a valid outcome and not a failure; the known limits of the methods are in
  `docs/FEATURES.md` and `docs/BENCHMARKS.md`.

The result belongs as a note in `docs/ROADMAP.md` under the phase's acceptance —
with numbers. "Worked for me" is not an acceptance criterion.

---

## 7. Uninstalling

```bash
sudo apt remove alpendns    # service gone, /etc and /var remain
sudo apt purge alpendns     # additionally: configuration, user, /var/lib, /var/cache
```

After `purge` nothing is left behind. Before that, do not forget to point the
DHCP server in your router back at another resolver — otherwise the network
stands still.
