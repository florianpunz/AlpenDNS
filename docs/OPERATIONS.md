# Operations

Installation, upgrade, backup, troubleshooting. This file is written so that
someone else can install from it without asking — that is the acceptance
criterion of phase 9, step 8.

Everything here applies to Debian 12/13 and Ubuntu 24.04 and up. AlpenDNS is a
Linux program; other systems are not a target.

> **A note on languages.** Prose and program output are English; only the code
> comments and the commit messages are still German. Wherever this document
> quotes output or greps for it, the string is reproduced verbatim — if you
> change it in the source, change it here too. The test
> `crates/alpendns/tests/language.rs` holds the output to English.

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
GitHub release — for `amd64` and `arm64`. The version number is raised **before
the push**, not by CI: `scripts/bump-version.sh major|minor|patch`. Which of the
three it is follows from what has come together since the last tag — the
heaviest kind wins:

| Call | What it means | Example |
|---|---|---|
| `major` | something new has arrived, or something existing breaks | `1.4.2` → `2.0.0` |
| `minor` | something existing is repaired or changed: a bugfix, a change in behaviour, documentation that adds or changes content | `1.4.2` → `1.5.0` |
| `patch` | typos, spelling, a few corrected sentences in the docs | `1.4.2` → `1.4.3` |

Raised once per release, not once per commit — that is what makes the number a
statement about the release: what has changed since the previous one, rather
than what the last commit happened to be. `git describe --tags --abbrev=0` names
the last tag, `git log <tag>..HEAD --oneline` what has come together since.

That a new feature already moves the first position is a deliberate departure
from SemVer, where it is a `minor` and only a break is a `major`. Here the kind
of change decides, and "something is new" is the heaviest kind there is.

The coding agent raises the version itself before the push; anyone pushing by
hand calls the script themselves. If the rule is broken and the version already
exists as a tag in the repo, CI aborts with a clear message rather than
publishing the same number a second time.

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
| `/etc/logrotate.d/alpendns` | Rotation of the query log (conffile), see §6 | root |
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

Freshly installed, the server listens on **every IPv4 interface** (`0.0.0.0`).
A client in the LAN reaches it by this machine's address; pointing the router's
DHCP at that address is the whole setup. A resolver that nobody can reach was
the more common installation mistake.

> **Warning — open resolver.** The configuration cannot know whether this
> machine also has a public IPv4 address. If it does, the wildcard listener is
> an open resolver, and an open resolver is an amplification reflector. Rate
> limiting is on by default (100 queries per second per client, IPv6 keyed by
> /64), but it is the brake for the packet that gets through, not the
> protection: keep port 53 closed on the WAN side of the router, or restrict
> the listener as described below.

`alpendns check` prints that warning again on every start, once per wildcard
listener. It is not an error — on a machine behind a router it is the intended
state.

Checking that it answers:

```bash
dig @127.0.0.1 example.com          # should return an address
dig @127.0.0.1 doubleclick.net      # should return NXDOMAIN
dig @192.168.1.10 example.com       # same, from another machine in the LAN
systemctl status alpendns
```

### Restricting the listener

One change, in the first block of `/etc/alpendns/alpendns.toml`:

```toml
[server]
listen_udp = ["127.0.0.1:53", "192.168.1.10:53"]
listen_tcp = ["127.0.0.1:53", "192.168.1.10:53"]
```

Enter this machine's actual LAN address. Do **not** add `[::]:53` next to
`0.0.0.0:53`: on Linux the v6 wildcard already carries IPv4, and the second
bind fails with "address already in use". For IPv6 enter the machine's concrete
IPv6 address instead.

If the machine itself resolves through AlpenDNS, keep a loopback listener
(`127.0.0.1:53`, and `[::1]:53` where the host uses IPv6 loopback) in both
lists: the blocklist refresh after startup resolves its own list URL through
the system resolver. The strict load before the listeners means that a `url`
list without a cached copy aborts the start on such a host; point
`/etc/resolv.conf` at the loopback only after a start in which every
configured `url` list reached the cache (`sudo ls -l
/var/cache/alpendns/lists/`, or the age in the interface).

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

Add a list after that start with a reload rather than a restart:

```bash
sudo systemctl kill -s HUP --kill-whom=main alpendns
```

A restart instead hits the abort above, because the new list has no cached
copy yet. If the reload's fetch fails, the failed list keeps the cached
version if there is one. A missing list that a policy references — the
implicit default policy references every enabled list — makes the rebuild fail
with `rule set cannot be built, the old one stays in effect`, and then no list
is updated. A repeated HUP retries immediately; the next regular attempt is
one refresh interval away (24 h by default). `journalctl -u alpendns` shows
`list loaded` with `origin=Network` or `origin=NotModified` on success.
Restart only once the new list is in the cache (`sudo ls -l
/var/cache/alpendns/lists/`); a restart before that ends after five attempts
with the unit failed.

Finally, switch the clients over: point the DHCP DNS server in your router at
this machine's address.

### A page for blocked names

With `blocking.mode = "sinkhole"` a blocked name resolves to an address you
choose. Point it at this machine and the browser lands here; without something
listening, it gets a connection error. AlpenDNS can serve a page that shows the
blocked name and the blocklist that matched it.

Three changes in `/etc/alpendns/alpendns.toml`:

```toml
[blocking]
mode = "sinkhole"
sinkhole_ipv4 = "192.168.1.10"
sinkhole_ipv6 = "fd00::10"

[block_page]
enabled = true
listen = "0.0.0.0:80"
```

Enter this machine's LAN address in both sinkhole keys. Then:

```bash
sudo alpendns -c /etc/alpendns/alpendns.toml check
sudo systemctl restart alpendns
```

`check` prints the block-page listener on its own line — not in the
open-resolver note, which is about DNS.

The page shows the name and the matched list, nothing else: no client address,
no policy name, no rule text, no counters, no link to the interface. A name
with no list entry gets a neutral page that says only that. Page views do not
appear in the DNS statistics and do not feed the heuristics.

Two limitations:

- **HTTP only, no port 443.** There is no certificate for a blocked name, so an
  `https://` site shows a certificate error or aborts through HSTS before the
  page is reached. Plain `http://` works.
- **IPv4 only.** The listener binds one IPv4 address (`0.0.0.0:80` by default),
  so an IPv6-only client cannot reach the page. `[::]:80` would also carry
  IPv4, but its v4-mapped peers defeat the per-client policy matching
  (ADR-0031).

The listener is an open, unauthenticated port. Like port 53, it belongs behind
the router's WAN firewall. The shipped configuration leaves `[block_page]
enabled = false` and `blocking.mode = "nxdomain"`: a fresh install opens no new
port.

### Names of your own

Two things look similar and are not. Both keep a name in the house; a zone of
one kind may lie inside the other, and the more specific one wins:

| | `[[forward_zone]]` | `[[local_zone]]` |
|---|---|---|
| Who answers | another nameserver, e.g. the router | AlpenDNS itself |
| What it is for | a zone that already exists in the LAN (`home.arpa`, `10.in-addr.arpa`) | a name that exists nowhere: `nas.meinedomain.at` |
| Transport | cleartext UDP, into the own network | none for its entries — no upstream is asked |
| Changes | on the other server | in this file, restart needed |

A `[[local_zone]]` is a table, not a zone file: no SOA, no transfer, and
AlpenDNS does not become authoritative for it
([ADR-0023](adr/0023-lokale-zonen.md)).

A `[[forward_zone]]` may name several servers — `upstream = ["udp://…",
"udp://…"]`, or a single string as before. They are asked in the order written;
the next one only when the one before did not answer. A dead domain controller
then costs its timeout, not the whole zone.

A name that should be answered here and *only* here:

```toml
[[local_zone]]
zone = "meinedomain.at"
records = [
  { name = "@",   type = "A", value = "192.168.1.5" },
  { name = "nas", type = "A", value = "192.168.1.5" },
]
```

Everything else under `meinedomain.at` is still resolved over the upstream — or
by a more specific `[[forward_zone]]`; the default is `fallback = "upstream"`,
split horizon. To keep the whole domain inside the house,
`fallback = "nxdomain"`; without `records` that is the entire zone:

```toml
[[local_zone]]
zone = "meinedomain.at"
fallback = "nxdomain"      # no upstream sees the rest of this domain
```

`alpendns check` prints the state of every local zone, including which of the two
fallbacks is active — the default is the quieter of the two, and a forgotten
`fallback` would weaken exactly the promise the section is about.

Zones may lie inside one another. A more specific `[[forward_zone]]` wins over
the local zone that contains it: `1.168.192.in-addr.arpa` inside a closed
`168.192.in-addr.arpa` sends that `/24` to the LAN server and keeps the rest of
the `/16` at home. A more specific local zone answers its entries; its
`fallback` decides the rest. `alpendns check` prints the direction for a nested
local/forward pair. A local record in the less specific zone no longer applies
under a more specific forward zone
([ADR-0029](adr/0029-nested-forward-zone.md)).

Three things to know before the first entry:

* **A name with a dot in it, but no dot at the end, is read relative** — as in a
  zone file. `nas.example.com` becomes `nas.example.com.meinedomain.at`. That is
  right for `_https._tcp` in an SRV zone and a silent mistake everywhere else.
  Absolute means `nas.example.com.`
* **Reverse lookups need a reverse zone of their own.** `dig -x 192.168.1.5`
  needs `zone = "1.168.192.in-addr.arpa"` with `{ name = "5", type = "PTR",
  value = "nas.meinedomain.at." }`. It is not derived from the A records.
* **A publicly signed domain that is overridden here** can produce answers that
  a validating client rejects — the well-known split-horizon problem. It only
  concerns clients that validate DNSSEC themselves.

### Private reverse zones are answered locally

A PTR query for `192.168.1.5` must not leave the house. The question "which
device is 192.168.1.5" is nobody else's business, and upstreams treat these
ranges differently — some answer NXDOMAIN, some SERVFAIL, which is where the
unstable answers in the observation period came from.

The reverse zones of
[RFC 6303](https://www.rfc-editor.org/rfc/rfc6303#section-4) §4 are compiled
into the binary: the IPv4 `in-addr.arpa` entries and the IPv6 `ip6.arpa`
entries, 33 zones in all. A name inside one is answered like a closed local
zone — NXDOMAIN without a synthetic SOA, never asked outward
([ADR-0032](adr/0032-rfc-6303-eingebaut.md)). Without an SOA the answer is also
not negatively cached (RFC 2308 §5), exactly as for a closed `[[local_zone]]`;
that is intentional, because a synthetic SOA would make AlpenDNS look
authoritative, which it is not.

**A configured zone always wins.** If a `[[local_zone]]` or a `[[forward_zone]]`
contains the name, the built-in default steps aside, whatever that zone's
`fallback` is. `config/alpendns.example.toml` does exactly this: its explicit
`168.192.in-addr.arpa` and `10.in-addr.arpa` forward zones remain legitimate
overrides, because on those networks a LAN server answers part of the range.
That override rule is also the only way to send one of these ranges outward
again — a `[[local_zone]]` with `fallback = "upstream"` for the zone. There is
no global switch; "do not ask strangers about my LAN" is not a preference.

**The zone name itself answers NXDOMAIN too**, where RFC 6303 §3 would return
an SOA or "no data". Deliberate: AlpenDNS hosts no zone and invents no record
([ADR-0032](adr/0032-rfc-6303-eingebaut.md)). `dig 10.in-addr.arpa SOA` therefore
says "does not exist"; that is not a sign of a broken installation.

**Migration:** a configuration from before this change starts unchanged.
Explicit local and forward zones keep winning over the built-in default, and the
old copy-paste block of `[[local_zone]] … fallback = "nxdomain"` entries may
simply be deleted; leaving it in place is harmless.

**A `forward_zone` inside a closed zone is allowed, and the more specific zone
wins.** The chain is `Cache → Local → ZoneRouter → Pool`, so `LocalBackend`
hands a name to the `ZoneRouter` when a forward zone lies deeper than the
local zone that would otherwise answer ([ADR-0029](adr/0029-nested-forward-zone.md)).
A `/24` of the `192.168/16` range can therefore go to the domain controllers
while the rest stays closed — and with the built-in default the `[[local_zone]]`
half is no longer needed:

```toml
[[forward_zone]]
zone = "1.168.192.in-addr.arpa"
upstream = "udp://192.168.1.10:53"
```

`alpendns check` lists the forward zones, warns about duplicates, and names
which zone answers for every nested local/forward pair outside `resolver.arpa`.
A `local_zone` and a `forward_zone` for the same name are still a startup error;
a local record in the less specific zone does not apply under a deeper forward
zone.

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

The built-in RFC 6303 reverse zones need no configuration change: an existing
configuration starts unchanged, explicit local and forward zones keep winning,
and the old copy-paste block may be deleted (see
[Private reverse zones are answered locally](#private-reverse-zones-are-answered-locally)).

What happens:

* `/etc/alpendns/alpendns.toml` is a **conffile**. An edited file is not
  overwritten; `dpkg` asks if both sides have changed. An **unmodified** one is
  replaced — after the upgrade that carries
  [ADR-0031](adr/0031-auslieferung-auf-allen-schnittstellen.md), an installation
  that never left the old loopback default listens on `0.0.0.0`. Set the
  listener explicitly before such an upgrade, or run `alpendns check` right
  after it; the check lists every listener beyond your own network.
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
are files without a schema. Keep a copy of the pre-ADR-0029 configuration or run
`alpendns check` from the 2.0.0 package before downgrading: a configuration with
nested `[[local_zone]]`/`[[forward_zone]]` entries is refused by the 2.0.0
release (ADR-0029).

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

Temporary grants and blocks — the `Allow` and `Deny` buttons in the interface —
are not in the backup either, because they never reach the disk. A restart clears
them. That is deliberate: they are time-limited, and persisting them would tie
their deadline to the wall clock, which the policy layer otherwise avoids (a
grant that survives a reboot is an allowlist with an expiry date — put the name
in a real allowlist instead).

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
| `blocklists could not be loaded at startup` | No network on the very first start, and nothing in the cache yet — or a single list that cannot be read and has no cache yet, a `path` typo included. Seen in `journalctl -u alpendns` after a failed start, not in `check` (it neither downloads nor stats list files). On a host that resolves through itself, that is the case until every `url` list has a cached copy — see "Restricting the listener". Later, an outage is not critical: the cached version stays in force. |
| `listeners could not be opened` | Port 53 is taken — see below. |
| `... cannot write in ...` | Permissions under `/var` are off. `systemctl restart alpendns` resets them, because systemd manages the directories. |

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
`/etc/resolv.conf` or the `DNS=` entry in `resolved.conf` at `127.0.0.1`; see
the listener and cache caveat under "Restricting the listener".

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

**A name from `[[local_zone]]` does not resolve.** Four questions, in this
order:

1. **Is the name in the table at all — and is it shadowed?** A local zone never
   answers a name that was not written into it — `grep` in the configuration is
   faster than any tool here. And watch the relative-name trap from §1. A more
   specific `[[forward_zone]]` can shadow an entry; `alpendns check` prints the
   pair. A deeper `[[local_zone]]` does not — entries are looked up across all
   local zones.
2. **Was the right type asked?** A name with an `A` entry and no `AAAA` entry
   answers NODATA rather than the upstream's answer. That is intended: a name
   must not have two horizons.
3. **Is the zone closed?** With `fallback = "nxdomain"` everything the table
   does not contain is NXDOMAIN — unless a more specific zone claims it (a
   nested `forward_zone` or `local_zone`). That can lock a whole network out if
   the zone was meant to be split-horizon. `alpendns check` prints the fallback
   of every zone for exactly this reason.
4. **Is the old answer still in the cache?** Positive answers are cached for
   their TTL, and a change to the table only takes effect after a restart. A
   restart empties the cache.

```bash
alpendns -c /etc/alpendns/alpendns.toml check   # zones, entries, fallback
dig @127.0.0.1 nas.meinedomain.at A             # what comes back, and from whom
```

### Everything is SERVFAIL

Every name, from every client, and the upstreams look healthy in the UI. If
`privacy.dnssec` is on, suspect the system clock before anything else:

```bash
timedatectl                                    # "System clock synchronized: yes"?
journalctl -u alpendns | grep -i "resolution failed"
# → DNSSEC validation failed, the answer was dropped
alpendns -c /etc/alpendns/alpendns.toml check  # prints a note about the clock
```

A signature carries a validity window, and the validator compares it against
the system clock (`hickory-net`, `ExpiredRrsig`). A box without a working RTC
comes up on the epoch date; `current_time >= sig_inception` is then false for
*every* signed zone, so every answer is bogus and every query is SERVFAIL. The
upstreams stay green while this happens, and that is not a bug: a bogus zone is
not an upstream outage — the upstream answered, and it would answer the same at
every other provider, so counting it as a failure would let one broken zone
mark the whole pool dead (`crates/alpendns/src/upstream/pool.rs`).

The usual case is milder than the epoch date: at boot the resolver can be up
before `systemd-timesyncd` has set the clock, and until it has, DNS is SERVFAIL.
Then it heals by itself — a bogus answer is never cached. /run is a tmpfs, so
the marker file disappears with every reboot; the note from `check` therefore
appears again on each start until the clock has been set, and not only on the
very first one.

**NTP is a prerequisite here, not a comfort.** With the clock correct, the
signature check is the thing that stops a forged answer; with it wrong, the
check itself is what breaks the network. Do not "fix" this by turning
`privacy.dnssec` off — anything that can set the clock can then also forge an
answer.

The unit is ordered after `time-sync.target`, which is what
`systemd-time-wait-sync.service` provides. That ordering alone changes nothing
on a stock Debian/Ubuntu system, because the service is not enabled there.
Enabling it is a decision rather than a default, and the price is real: it
waits with `TimeoutStartSec=infinity`, so on a machine that never synchronizes
it would hold the resolver back for good. On a box with an RTC and a working NTP
server, the wait is over in the first seconds of boot, and the ordering buys
exactly those seconds:

```bash
sudo systemctl enable systemd-time-wait-sync.service   # the wait becomes real
systemctl status systemd-time-wait-sync                # what it is waiting for
```

### A device stops getting answers

Possibly the rate limiter. The counter is in the log and in the metrics:

```bash
# The message is at debug level — see "More in the log" below.
journalctl -u alpendns | grep -i "exceeding the rate limit"
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
`ring_seconds` in RAM, nothing more. The "Flagged" panel in the UI reads
exactly that buffer — a false positive nobody notes down the same day is gone
afterwards. For a run lasting a week, that is the wrong basis.

```toml
[privacy.logging]
# Necessary so the week can be evaluated at the end. The price is in ADR-0004:
# every resolved name lies on disk for the duration of the observation.
mode = "full"
```

Set it back after the evaluation.

**How long the names stay.** `full` writes to `/var/log/alpendns/queries.jsonl`
and nothing in the server deletes that file. The package ships a logrotate rule
to `/etc/logrotate.d/alpendns`: daily, compressed, and `rotate 7` — **seven files
means seven days of names**. That number *is* the retention period of the
observation, and it is a decision rather than a default: change it in that file.
The rule is a conffile, so an upgrade will not overwrite an edited version. It
works on a running service — `copytruncate` keeps the process writing into the
same file, so no restart and no signal to the server are needed, at the price of
a window of milliseconds in which a line can be lost. `alpendns check` prints a
note when the mode is `full` and the rule is missing.

Also enter your own domains into the typosquat guard (bank, government agency,
employer), otherwise it does nothing:

```toml
[detection]
typosquat = { action = "flag", threshold = 0.85, protect = ["meine-bank.at"] }
```

Whether a detector can find anything *at all* is answered beforehand by a look
into `alpendns check`: behind an enabled detector that lacks its basis, the
reason appears in parentheses — `typosquat flag (no protect: finds nothing)`,
`nrd flag (/var/lib/alpendns/nrd.txt is missing: runs empty)`.

All five detectors are on `flag`. **They stay on `flag` the whole week.** They
report, they do not block.

**During the week.** Look into the UI once a day, "Flagged" panel:

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
