# Contributing

Thanks for looking. This is a single-maintainer project, so the most useful
thing you can do first is open an issue describing what you want to change and
why — before writing the patch.

## Getting set up

Rust stable is the only prerequisite; [`rust-toolchain.toml`](rust-toolchain.toml)
selects it. There is no other build step.

```bash
cargo build
cargo test --all-features
```

Integration tests start a real AlpenDNS process on a loopback port. They never
contact real upstream resolvers or download real blocklists — please keep it
that way, so the suite stays runnable offline and in CI.

## Before you open a pull request

A change is done when the four commands in
[**Definition of Done**](docs/TESTING.md) pass. That document is the single
source of truth for how this project is tested; it also covers fuzzing, the
property tests, and the manual smoke test.

Two rules from [CLAUDE.md](CLAUDE.md) B.1 that trip people up, because they are
unusual and they are enforced by the compiler rather than by review:

* **No `unwrap()`, `expect()`, `panic!()` or slice indexing** anywhere on a
  path that touches network data. Allowed in `#[cfg(test)]` code only.
* **No `unsafe`** — it is `forbid` at the workspace level.

A new dependency needs one line of justification in the commit message.
`cargo deny` runs in CI and rejects GPL-incompatible licenses and crates with
open advisories (one documented exception, argued in [`deny.toml`](deny.toml)).

## Commits

Conventional Commits, and the message explains *why* — the *what* is in the
diff. One commit per logical change. [CLAUDE.md](CLAUDE.md) B.7 has the full
rule, including the reason there is no patch release level.

Please do not add attribution trailers or "generated with" lines.

## How this project is built

Worth knowing before you read the history: AlpenDNS is developed alongside
coding agents, and [CLAUDE.md](CLAUDE.md) is the working agreement that makes
that predictable — it is part of the project, not tooling cruft. Commits land
on a branch, and once the Definition of Done is green they are fast-forwarded
to `main`. Every push to `main` publishes a release, so version bumps happen
before the commit rather than in CI.

## License

Contributions are accepted under [AGPL-3.0-or-later](LICENSE), the license the
project already uses. By opening a pull request you agree to that.
