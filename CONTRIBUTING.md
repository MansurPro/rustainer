# Contributing to rustainer

Thanks for your interest in contributing! rustainer is a Docker-like container
engine written in Rust from first principles. The project is currently in the
**R&D phase** — there is no code yet, and research is the primary workstream.

## Project status

Read [`research/README.md`](research/README.md) before anything else. It tracks:

- Which research domains (D1–D5) are complete or partial
- The claim-tagging system (`[F]` fact / `[C]` convention / `[R]` recommendation /
  `[O]` observation) with confidence levels
- The remaining roadmap: finish Phase 2 → adversarial verification → architecture
  risk pass → write `RND.md`

The R&D effort has a defined stop condition: `RND.md` written. No code is written
before that.

## How you can help right now

- **Research contributions**: fill gaps in D3 (images/registries/storage) and D5
  (cgroups/security/rootless), or complete the missing tail sections of D2/D4.
  Follow the existing file structure and tag every claim.
- **Fact-checking**: verify claims against primary sources (kernel docs, OCI spec,
  man pages). Outdated or wrong claims are the most valuable finds — open an issue.
- **Environment probing**: run read-only probes on Linux systems and report results
  as `[O]` observations (name your host, kernel version, and exact commands).
- **Writing**: improve clarity, fix broken links, add diagrams.

## Ground rules

- **Cite primary sources** for factual claims (kernel source/docs, OCI spec,
  man pages). Blog posts are fine for context but must be marked lower confidence.
- **Never mutate production systems.** Research references live environments;
  observations are welcome, destructive experiments on shared hosts are not.
- Keep recommendations honest: state the tradeoff when arguing what the engine
  *should* do.

## Development (once code starts)

- Rust stable; `cargo fmt` and `cargo clippy` clean
- Linux-first: kernel-facing features require a Linux environment (VM recommended)
- Every feature lands with tests and a short note in the implementation ladder

## Submitting changes

1. Open an issue first for anything non-trivial so we can align on direction.
2. Fork, create a feature branch, commit with clear messages.
3. Open a pull request describing what changed and why, linking the issue.

## Reporting bugs

Open a GitHub issue using the bug report template. For security-sensitive
findings (anything about isolation boundaries, privilege escalation, etc.),
see [`SECURITY.md`](SECURITY.md).
