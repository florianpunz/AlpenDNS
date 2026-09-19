# Security Policy

AlpenDNS is a DNS resolver: it sits directly in the network path and parses
untrusted bytes from it. Reports are welcome and taken seriously.

## Reporting a vulnerability

Use GitHub's **private vulnerability reporting** — the *Security* tab of this
repository, then *Report a vulnerability*. That opens a private thread visible
only to the maintainer; no email address is published for this purpose.

Please include what you need to make the report actionable: the affected
version or commit, a way to reproduce, and what an attacker gains. A sketch is
fine — better an early, incomplete report than a late, polished one.

There is no bug bounty. This is a single-maintainer project; expect an
acknowledgement within a few days rather than within hours. Only the most
recent release is supported: every push to `main` publishes a release, so
there are no maintained older branches to backport to.

## What counts as a vulnerability

Anything that lets an attacker who cannot already run code on the host do one
of these:

* **Crash the resolver.** A panic in the request path is a denial of service
  for the entire network behind it. This is the failure mode the project
  guards against hardest — see [CLAUDE.md](CLAUDE.md) B.1.
* **Escape the sandbox.** Read or write files the service user should not
  reach, or execute code.
* **Poison the cache.** Get an answer into the cache that does not match the
  question that was asked.
* **Bypass filtering or policy.** Make a blocked name resolve, or make one
  client's policy leak into another's answers.
* **Exfiltrate query names.** `privacy.logging.mode = "aggregate"` is the
  default, and it is a promise: under it, no query name may leave the process.
  A path that violates it is a vulnerability, not a bug.
* **Reach the network.** The process contacts exactly three kinds of
  destination — configured upstreams, configured blocklist URLs, and nothing
  else. Anything that opens another connection is a vulnerability.

## What is not a vulnerability

[`docs/THREAT-MODEL.md`](docs/THREAT-MODEL.md) states what this project
protects against and, just as deliberately, what it does not: traffic
analysis, a compromised client, malware protection, and anonymity. A report
that DNS encryption does not hide the IP addresses you subsequently connect to
is describing a documented limitation, not a flaw.

Amplification via a spoofed source IP is likewise inherent to being a resolver,
not a bug. The rate limiter caps per-client volume; it cannot stop a reflection
aimed at a third party. Do not expose the resolver to the public internet.

## Security-relevant documentation

| Document | Contents |
|---|---|
| [`docs/THREAT-MODEL.md`](docs/THREAT-MODEL.md) | Adversaries A1–A6, explicit non-coverage, and the project's own attack surface |
| [`docs/SECURITY-AUDIT.md`](docs/SECURITY-AUDIT.md) | Manual audit of the codebase, with the status of each finding |
| [`CLAUDE.md`](CLAUDE.md) B.1 | The non-negotiable coding rules that keep the request path panic-free |
| [`CLAUDE.md`](CLAUDE.md) B.5 | Process hardening and why rate limiting is mandatory |
| [`deny.toml`](deny.toml) | Advisory and license policy, including the one documented exception |
