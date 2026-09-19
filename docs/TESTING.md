# Test strategy

A DNS server is unusually testable: input and output are bytes over a socket,
and there is a normative standard for what counts as correct. We take advantage
of that.

## The six levels

### 1. Unit tests

Next to the code, `#[cfg(test)]`. Responsible for: blocklist parsers, config
deserialization, cache TTL logic, upstream selection strategies, heuristic
scoring, trace construction.

Rule: every parser is tested not only with valid input but with **broken**
input. For blocklists that means at least: empty file, file without a trailing
newline, lines with CRLF, comments, inline comments, Unicode/IDN, labels over
63 characters, names over 255 characters, leading/trailing dots, duplicate
entries, a 300 MB line.

### 2. Property tests (`proptest`)

For invariants that must hold for *all* inputs:

* Blocklist matcher: if `example.com` is listed as a wildcard, every subdomain
  matches and `notexample.com` never matches.
* Cache: a cached answer never has a higher TTL than it had on insertion.
* 0x20: `unrandomize(randomize(name)) == name`, and the comparison is
  case-insensitive.
* Name normalization is idempotent.

### 3. Fuzzing (`cargo fuzz`)

Everything that reads bytes from the network or from foreign files gets a fuzz
target:

* Parsing a DNS message (even though `hickory-proto` does it — we fuzz our use
  of it, including the places where we pull fields out).
* Blocklist lines, all formats.
* Config TOML.
* DoH path parsing (token extraction).

Success criterion: no panic, no endless loop, no unbounded growth. A crash that
is found is checked in as a corpus file and becomes a regression test.

In CI each target runs briefly (120 s) on the existing corpus. Long runs are
done locally.

### 4. Integration tests

A real AlpenDNS process on a loopback port, real queries, real answers. The
upstream is **always** a fake — an in-process DNS server with hard-wired
answers. Tests never go to the internet: not to resolvers, not to blocklist
URLs. A test that needs the network is a test that will be red at some point
without any code having changed.

Test cases that must exist:

* Query is answered (A, AAAA, CNAME chain, MX, TXT, NS, PTR).
* Query for a blocklisted domain returns the configured block answer.
* Allowlist beats blocklist.
* A second identical query comes from the cache (upstream sees exactly one
  query).
* 100 concurrent identical queries → exactly one upstream query (dedup).
* Upstream dead → next resolver; all dead → `serve_stale`, then SERVFAIL.
* A client over the limit is throttled, another is not (`tests/ratelimit.rs`).
  The second address is `127.0.0.2` — Linux gives the whole `127.0.0.0/8` to
  loopback, nothing needs configuring. A unit test alone would not do here: what
  has to be verified is that the packet's source address arrives, not the
  listener's.
* An answer with the wrong query ID or wrong QNAME is discarded and not cached.
* An answer over 1232 bytes over UDP sets TC; the same query over TCP returns
  the full answer.
* Rebinding: upstream answers `192.168.1.1` for a public name → blocked.
* SIGHUP with a broken config → old config stays active, the server keeps
  answering.
* Policy time window: the same query at two simulated clock times, two results.
  (Time must be injectable — no direct `SystemTime::now()` call in the policy.)

### 5. Conformance and load

* **Conformance:** a collection of real queries as a pcap, replayed against
  AlpenDNS and against `unbound`; the answers must agree in the relevant fields
  (RCODE, answer section, flags). Differences are either bugs or deliberate
  deviations, which get documented.
* **Load:** `dnsperf` or `flamethrower` against a fake upstream. Measured are
  queries/s, p50/p99/p999 latency, RSS. The numbers go into
  `docs/BENCHMARKS.md` and are re-collected for each phase. Without a baseline,
  "this is faster now" is a claim. As long as neither tool is installed, the
  load generator in `crates/alpendns/tests/load.rs` takes that role:

  ```bash
  cargo test --release --test load -- --ignored --nocapture --test-threads=1
  ```

  Because of `#[ignore]` it runs neither in CI nor under `cargo test` — a load
  measurement in the definition of done would slow down every run and would not
  be comparable on foreign hardware anyway.

### 6. Measurement runs against corpora (since phase 8)

The heuristics have acceptance criteria with numbers: "under 0.1 % false
positives on a top-100k corpus". Numbers like that need real data, and that data
is **not in the repo** — it is third-party, one to two megabytes, and not needed
to build. `corpus/` is in `.gitignore`.

```bash
mkdir -p corpus
curl -sSL https://downloads.majestic.com/majestic_million.csv | tail -n +2 \
  | cut -d, -f3 > /tmp/majestic.txt
head -100000            /tmp/majestic.txt > corpus/top-100k.txt    # measurement
sed -n '100001,600000p' /tmp/majestic.txt > corpus/train-500k.txt  # training

cargo test --release --test detect_corpus -- --ignored measure --nocapture
```

Majestic Million, CC-BY 3.0. `ALPENDNS_CORPUS_DIR` points at a different
directory.

**The split is not out of a love of order.** On the first attempt, training and
measurement ran on the same list, and the false positive rate was better by a
factor of 500 — 0.001 % against 0.54 % on unseen names. A model recognizes the
names it was built from. Anyone re-collecting one of these numbers has to
re-collect the split along with it.

The model itself is produced with the same tool; how is described in
`crates/alpendns/src/detect/dga/model.bin.md`.

## The replay harness

The tool that pays off most, and that you build early:

```
alpendns-replay --corpus queries.jsonl --config test.toml --expect expected.jsonl
```

One file of queries (name, type, client), one of expected verdicts. That turns
every change to lists, policies or heuristics into a measurable diff instead of
a gut feeling. The harness is also the basis for the blocklist diff review in
[FEATURES.md](FEATURES.md) (O3).

The heuristics need two corpora:

* **Benign:** the top 100k domains of a public popularity list. Expectation:
  false positive rate under 0.1 %. That is the real quality measure for DGA and
  typosquat detection, not the hit rate.
* **Malign:** known DGA families and tunneling samples. Expectation: hit rate
  documented per family, not averaged into a single number.

## Definition of Done

A change is done when:

```bash
cargo fmt --all --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test --all-features
cargo deny check
```

pass, **and** the change either brings a new test or a line of justification for
why it needs none. "Compiles" is not done.

This is the canonical definition; other documents point here rather than
repeating the commands.

While developing, there is no need to run the whole suite every time:

```bash
cargo test -p alpendns name_of_the_test  # a single test
cargo test -p alpendns --lib parser::    # everything under a module path
cargo test -p alpendns -- --nocapture    # see the test's output
```

And the fuzz target invoked by hand — nightly, and the only nightly exception
in the project:

```bash
cargo +nightly fuzz run parse_message -- -max_total_time=120
```

The load measurement is explicitly **not** part of it (see §5).
