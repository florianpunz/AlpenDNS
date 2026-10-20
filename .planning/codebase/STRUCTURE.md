---
last_mapped_commit: 630297fc35c98188292c5305d0e279aa6039b07a
last_mapped_at: 2026-10-20
---
# Codebase Structure

**Analysis Date:** 2026-10-20

## Directory Layout

```text
AlpenDNS/
├── Cargo.toml                 # Workspace, lints, pinned dependency versions
├── Cargo.lock                 # Locked dependency graph
├── rust-toolchain.toml        # Pinned stable toolchain (nightly only for fuzz)
├── clippy.toml                # Allows unwrap/expect in #[cfg(test)] code
├── deny.toml                  # cargo-deny license/advisory policy
├── CLAUDE.md                  # Behavioral and project rules
├── README.md, SECURITY.md, CONTRIBUTING.md, LICENSE
├── config/                    # Example and local configurations
│   ├── alpendns.example.toml  # Target format (unimplemented sections marked)
│   ├── alpendns.minimal.toml  # Small runnable configuration
│   ├── local.test.toml        # Local test configuration
│   └── local.dev.toml         # Local development configuration (gitignored)
├── crates/
│   └── alpendns/              # The only workspace member today
│       ├── Cargo.toml
│       ├── src/               # Library modules + binary entry
│       ├── tests/             # Integration tests (separate crate)
│       ├── fuzz/              # cargo-fuzz targets (own workspace, nightly)
│       └── proptest-regressions/
├── docs/                      # Architecture, roadmap, operations, ADRs
│   └── adr/                   # Numbered architecture decision records
├── web/                       # UI sources embedded into the binary
│   ├── index.html
│   ├── app.css
│   ├── app.js
│   └── demo/                  # Static demo build for GitHub Pages
├── packaging/                 # systemd, Debian, logrotate, release config
├── lists/                     # Local blocklist cache (gitignored)
├── corpus/                    # Measurement corpora (gitignored, --ignored tests)
├── scripts/                   # Version bump and README image generation
├── .github/workflows/         # CI, release and Pages workflows
└── .planning/codebase/        # Generated codebase maps (this directory)
```

## Directory Purposes

**`crates/alpendns/src/`:**
- Purpose: all testable library code plus the thin binary.
- Contains: `lib.rs` (module index), `main.rs` (startup/wiring), and one module
  per architectural concern.
- Key files: `lib.rs`, `main.rs`, `resolve.rs`, `trace.rs`, `config.rs`.

**`crates/alpendns/src/server/`:**
- Purpose: listener and per-request pipeline.
- Contains: `mod.rs` (`Server`, `Bound`, `handle_request`, `build_event`),
  `udp.rs`, `tcp.rs` (framing, connection limits, counters).

**`crates/alpendns/src/policy/`:**
- Purpose: client identity and policy evaluation.
- Contains: `mod.rs` (`Engine`, `PolicySet`, `Blueprint`, `PolicyBackend`,
  `run_updater`, `explain`), `clients.rs`, `rules.rs` (regex),
  `schedule.rs`, `temporary.rs`.

**`crates/alpendns/src/filter/`:**
- Purpose: blocklist formats, matching, loading and block answers.
- Contains: `mod.rs` (`Lists`, `LoadedLists`), `parser.rs`, `matcher.rs`,
  `source.rs` (file/HTTP loader, age, caching), `block.rs` (`BlockMode`,
  synthesis).

**`crates/alpendns/src/detect/`:**
- Purpose: heuristic detectors.
- Contains: `mod.rs` (traits, `Detector`, `Detectors`, counters), `dga/mod.rs`
  plus model data, `tunneling.rs`, `rebinding.rs`, `typosquat.rs`, `nrd.rs`.
- Note: this is the future `alpendns-detect` crate, intentionally still a
  module.

**`crates/alpendns/src/upstream/`:**
- Purpose: upstream pools and encrypted transports.
- Contains: `mod.rs` (`Encrypted`, `ForwardBackend`), `pool.rs` (`Pool`,
  `Upstream`, health, seed rotation), `strategy.rs` (`split_by_zone` math),
  `transport.rs` (DoT/DoH/DoQ), `odoh.rs` (Oblivious DoH).

**`crates/alpendns/src/logging/`:**
- Purpose: the only place query names may survive.
- Contains: `mod.rs` (`Mode`, `QueryLog`, `QueryEvent`, `BlockReason`),
  `counts.rs` (aggregate counting), `ring.rs` (RAM ring buffer).

**`crates/alpendns/src/api/`:**
- Purpose: HTTP API, metrics endpoint and embedded UI.
- Contains: `mod.rs` (`StatusSource`, `ApiState`, routes, auth, SSE),
  `ui.rs` (three static routes via `include_str!` and UI tests).

**`crates/alpendns/tests/`:**
- Purpose: integration tests against a real in-process or loopback server.
- Contains: one file per feature area: `integration.rs`, `policies.rs`,
  `local_zones.rs`, `encrypted.rs`, `odoh.rs`, `dnssec.rs`, `detection.rs`,
  `detect_corpus.rs` (`--ignored` measurement), `blocklists.rs`,
  `ratelimit.rs`, `reload.rs`, `observability.rs`, `load.rs`, `language.rs`.

**`crates/alpendns/fuzz/`:**
- Purpose: parser fuzzing with `cargo-fuzz`.
- Contains: `fuzz_targets/parse_request.rs`, `parse_blocklist.rs`,
  `parse_config.rs`, `parse_upstream_addr.rs`; checked-in `corpus/`,
  `artifacts/`, `known-crashes/`; its own `Cargo.toml` workspace because it
  needs nightly.

**`docs/`:**
- Purpose: target picture, operations and rationale.
- Contains: `ARCHITECTURE.md`, `ROADMAP.md`, `OPERATIONS.md`, `TESTING.md`,
  `THREAT-MODEL.md`, `FEATURES.md`, `TODOS.md`, `SECURITY-AUDIT.md`,
  `PERFORMANCE_ANALYSIS.md`, `BENCHMARKS.md`, and `adr/` with ADR-0001 … ADR-0029.

**`web/`:**
- Purpose: UI sources compiled into the binary.
- Contains: `index.html`, `app.css` (design language documented in the header),
  `app.js`; `demo/` builds the static GitHub Pages demo into `web/dist/`.

**`packaging/`:**
- Purpose: delivery artifacts.
- Contains: `systemd/alpendns.service` (hardened unit, `AmbientCapabilities`,
  `ExecStartPre` check), `debian/` maintainer scripts, `logrotate/alpendns`,
  `alpendns.toml` (shipping config), `abnahme.py` (acceptance helper).

**`config/`:**
- Purpose: configuration examples.
- Contains: the target-format specification and small runnable configs. The
  live example is `config/alpendns.example.toml`; local variants are ignored by
  git.

**`scripts/`:**
- Purpose: developer and release helpers.
- Contains: `bump-version.sh` (major/minor/patch, updates `Cargo.toml` and
  `Cargo.lock`), `make-readme-images.sh`.

**`.github/workflows/`:**
- Purpose: automation.
- Contains: `ci.yml` (fmt, clippy, test, cargo-deny, fuzz), `release.yml`
  (builds `.deb` for amd64/arm64 and publishes a GitHub release on `main`),
  `pages.yml` (builds the demo into GitHub Pages).

## Key File Locations

**Entry Points:**
- `crates/alpendns/src/main.rs:127`: `main`, CLI parsing and wiring.
- `crates/alpendns/src/main.rs:991`: `check` subcommand.
- `crates/alpendns/src/main.rs:1233`: `policy test` subcommand.
- `crates/alpendns/src/server/mod.rs:163`: `handle_request`, the per-request
  pipeline entry.
- `crates/alpendns/src/server/udp.rs:20`, `crates/alpendns/src/server/tcp.rs`:
  listener loops.
- `crates/alpendns/src/api/mod.rs:127`: API router.
- `crates/alpendns/src/api/mod.rs:153`: metrics router.
- `crates/alpendns/src/api/ui.rs:19`: UI router.

**Configuration:**
- `crates/alpendns/src/config.rs:53`: top-level `Config` with
  `deny_unknown_fields`.
- `crates/alpendns/src/config.rs:1103`: `Config::load`.
- `crates/alpendns/src/config.rs:1118`: semantic `validate`.
- `Cargo.toml`: workspace lints and pinned dependency versions.
- `rust-toolchain.toml`, `clippy.toml`, `deny.toml`: toolchain and policy.
- `packaging/alpendns.toml`: shipping configuration for `/etc/alpendns`.

**Core Logic:**
- `crates/alpendns/src/resolve.rs`: `ResolveBackend` trait and `ResolveError`.
- `crates/alpendns/src/trace.rs`: `Ctx`, `Step`, `MatchKind`, `ScheduleEffect`.
- `crates/alpendns/src/policy/mod.rs`: policy engine and backend.
- `crates/alpendns/src/caching.rs`, `crates/alpendns/src/cache.rs`: cache layer
  and store.
- `crates/alpendns/src/local.rs`, `crates/alpendns/src/router.rs`: local zones
  and zone routing.
- `crates/alpendns/src/upstream/`: pools and transports.
- `crates/alpendns/src/filter/`: blocklists.
- `crates/alpendns/src/detect/`: heuristics.
- `crates/alpendns/src/logging/`: log modes and query log.

**Testing:**
- `crates/alpendns/tests/`: integration tests.
- Unit tests live in `#[cfg(test)] mod tests` at the bottom of each source file.
- `crates/alpendns/fuzz/fuzz_targets/`: fuzz entry points.
- `crates/alpendns/fuzz/corpus/`: checked-in seeds.
- `crates/alpendns/proptest-regressions/`: property-test regressions.

**Documentation:**
- `docs/ARCHITECTURE.md`: target picture.
- `docs/ROADMAP.md`: current phase and acceptance criteria.
- `docs/OPERATIONS.md`: install, upgrade, backup, observation week.
- `docs/TESTING.md`: test strategy and definition of done.
- `docs/adr/NNNN-title.md`: why a decision was made.

## Naming Conventions

**Files:**
- Rust modules: `snake_case.rs`; submodules live in a directory with `mod.rs`
  or in `name.rs` beside peers (`server/mod.rs`, `server/udp.rs`).
- Documentation: `UPPERCASE.md` for top-level docs; ADRs use
  `NNNN-kebab-case-title.md` with a four-digit number.
- Web assets: lowercase, no hashing: `index.html`, `app.css`, `app.js`.
- Configs: `alpendns.*.toml`, `local.*.toml`.

**Directories:**
- One directory per architectural concern under `src/`; nested directories only
  where a concern has submodules (`server/`, `upstream/`, `filter/`, `policy/`,
  `logging/`, `api/`, `detect/`).
- Test directories mirror feature areas, not source modules.

**Functions:**
- `snake_case`, verb-first where it describes an action (`resolve`, `evaluate`,
  `record_failure`, `build_event`); constructors `new`/`from_*`/`with_*`.
- Public items have doc comments; private functions only when the *why* is not
  obvious from the code.

**Variables:**
- `snake_case`; configuration field names are also `snake_case` in TOML.
- Rust types: `PascalCase`; enums variants: `PascalCase`.
- Constants: `SCREAMING_SNAKE_CASE` (`MAX_CONNECTIONS`, `FAILURE_TTL`,
  `SHARDS`).

**Tests:**
- Test names are full behavioral sentences in English, not `test_foo`:
  `udp_query_is_forwarded_and_answered`,
  `upstream_answer_with_wrong_name_is_discarded`,
  `a_new_token_file_is_only_readable_by_the_owner`.
- `--ignored` marks measurement tests that need external corpora
  (`detect_corpus.rs`).

**Documentation language:**
- Module-level and inline comments are German; public API names and tests are
  English. Match this split rather than translating existing comments.

## Where to Add New Code

**New pipeline layer:**
- Implementation: new module under `crates/alpendns/src/`, implement
  `ResolveBackend` (`crates/alpendns/src/resolve.rs:64`).
- Wiring: insert it into the chain in `crates/alpendns/src/main.rs:421`; document
  its position relative to filtering and caching.
- Trace: add a `Step` variant in `crates/alpendns/src/trace.rs:43` and record it
  in the layer.

**New heuristic detector:**
- Implementation: `crates/alpendns/src/detect/<name>.rs`, implement
  `NameDetector` or `AnswerDetector` (`crates/alpendns/src/detect/mod.rs:232`).
- Registration: add a `Detector` variant, include it in `Detector::ALL`,
  `BlockReason::ALL`, `Detectors::new`, and the config section in
  `crates/alpendns/src/config.rs`.
- Default: `flag`, never `block` without the acceptance decision required by
  CLAUDE.md B.8.
- Tests: `crates/alpendns/tests/detection.rs`; measurement runs in
  `detect_corpus.rs` behind `--ignored`.

**New blocklist format:**
- Implementation: `Format` and `parse` in
  `crates/alpendns/src/filter/parser.rs`.
- Tests: `crates/alpendns/tests/blocklists.rs`, including broken and truncated
  input; add a fuzz seed if the parser takes raw bytes.

**New API endpoint:**
- Route and handler: `crates/alpendns/src/api/mod.rs:127`.
- Data access: extend `StatusSource` (`crates/alpendns/src/api/mod.rs:35`) and
  implement it on `Runtime` in `crates/alpendns/src/main.rs:760`.
- Keep query names out of tokenless routes.

**New UI element:**
- Source: `web/index.html`, `web/app.css`, `web/app.js`; both files are embedded
  by `crates/alpendns/src/api/ui.rs:15`.
- Follow the design rules in CLAUDE.md B.6 and the header of `web/app.css`; add
  or extend the checks in `crates/alpendns/src/api/ui.rs`.

**New configuration field:**
- Struct: `crates/alpendns/src/config.rs` with `#[serde(default)]` as
  appropriate and `deny_unknown_fields` on the containing struct.
- Validation: add to `Config::validate` or the section validator.
- Documentation: add the field to `config/alpendns.example.toml`; mark
  unimplemented sections `[PHASE n]`.

**New integration test:**
- File: `crates/alpendns/tests/<area>.rs`; use a loopback server and fake
  upstreams, never real resolvers or list URLs.
- Add `#![allow(clippy::expect_used, clippy::indexing_slicing)]` at the top of
  new test files as needed (integration tests are a separate crate).

**New ADR:**
- File: `docs/adr/NNNN-kebab-case-title.md`, next free number; link it from the
  affected module docs and `docs/ARCHITECTURE.md`.

**New crate:**
- Only when the corresponding phase arrives; create under `crates/<name>/`,
  add it to the workspace in `Cargo.toml`, and move the existing module without
  changing call paths.

## Special Directories

**`target/`:**
- Purpose: Cargo build output.
- Generated: Yes.
- Committed: No (gitignored).

**`lists/`:**
- Purpose: cached blocklists and local `api.token`.
- Generated: Yes, at runtime.
- Committed: No (gitignored).

**`corpus/`:**
- Purpose: measurement corpora for heuristic evaluation.
- Generated: No (downloaded separately).
- Committed: No (gitignored; tests are `--ignored`).

**`web/dist/`:**
- Purpose: built static demo for GitHub Pages.
- Generated: Yes (`web/demo/build.sh`).
- Committed: No in the current `.gitignore` (the demo workflow rebuilds it).

**`crates/alpendns/fuzz/artifacts/`, `fuzz/corpus/`:**
- Purpose: fuzz crash artifacts and generated corpus.
- Generated: Yes.
- Committed: No; curated seeds and `known-crashes/` are checked in.

**`.planning/codebase/`:**
- Purpose: generated codebase maps consumed by GSD commands.
- Generated: Yes.
- Committed: Depends on the repository's planning workflow.

**`CLAUDE-SECURITY-*` directories:**
- Purpose: audit artifacts left in the working tree.
- Generated: Yes.
- Committed: No (they are not referenced by the build).

---

*Structure analysis: 2026-10-20*
