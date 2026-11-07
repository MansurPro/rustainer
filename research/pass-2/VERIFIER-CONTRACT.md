# Phase 3 adversarial verification — shared contract

Read this first. Your per-agent brief names your file, sections, and adjudication targets.

## Who you are

A **fresh** adversarial verifier. You did **not** write the research you are checking.
Your job is to **falsify**, not confirm. A claim that survives you must be stronger than
it was before you touched it.

## Project context

R&D toward a Docker-like container engine written in **Rust from first principles** — we
own the runtime core, we do **not** shell out to runc/crun/containerd. Linux only, x86_64
first without needless arch coupling, cgroup v2 preferred, **root-first / rootless-later**,
eventual OCI compatibility. No code exists and none will be written in this phase.

The research files D1–D5 were written **2026-09-04**. Today is **2026-10-23**. Seven weeks
of upstream drift is enough to move a `[C]` claim — check release feeds where it matters.

## Verdict scheme — tag every claim you adjudicate with exactly one

| Verdict | Meaning |
|---|---|
| **CONFIRMED** | Primary source checked; claim holds as written |
| **CORRECTED** | Wrong or imprecise; give the corrected statement + source |
| **DISPUTED** | Sources genuinely disagree; name both, say which governs for us |
| **WEAK-EVIDENCE** | Probably true, but the cited basis does not support it; name the test that settles it |
| **UNVERIFIABLE-HERE** | Cannot be settled from this machine; name the exact host + command that would settle it |

## Tag legend in the research files (this is your input list)

`[F]` **Fact** — what Linux/OCI actually requires · `[C]` **Convention** — what Docker/runc/etc happen to do
`[R]` **Recommendation** — what our engine should do · `[O]` **Observation** — empirical; names host + kernel + command
plus `HIGH` / `MED` / `LOW` confidence.

## Prioritise in this order

1. **`[F]` HIGH claims.** A wrong fact asserted at high confidence is the most expensive thing in the corpus.
2. Claims that **load-bearing `[R]` recommendations rest on**.
3. The **named cross-file contradictions** in your brief — *adjudicate* them, do not restate them.
4. `[O]` claims whose host/kernel **does not generalise** (RHEL 4.18 vs Ubuntu 6.11 vs upstream mainline).
5. `[C]` claims about Docker/runc/containerd behaviour that **may have moved**.

Skip LOW-confidence decoration. Do not re-verify what nothing depends on. Breadth first:
cover every assigned section at least enough to say "nothing here needed correcting", *then*
go deep on what moved.

## On `[R]` recommendations

A recommendation cannot be "wrong". Its **stated reasoning** can be. Audit the reasoning and
the facts it rests on. If the facts survive but the tradeoff was mis-weighed, record
**DISPUTED** with your weighting and why.

## Source priority

1. Official Linux kernel documentation / man7 2. OCI specifications 3. Docker / containerd /
runc / crun official docs 4. Source code of mature open-source container systems 5. Relevant
standards 6. High-quality systems-engineering articles 7. Conference talks & papers
8. GitHub implementations 9. Community discussion — **only** for undocumented real-world behaviour.

Never cite a blog or Stack Overflow where a primary source exists.

**Cite recoverably:** `<repo>@<sha>/<path>#L<n>` or a pinned tag URL. Never `main`/`master` —
they drift, and a `[C]` HIGH against a moving target decays to `[C]` LOW the moment upstream
refactors. This is a known defect of the first wave; do not repeat it.

## Empirical access — optional, READ-ONLY

**`ssh host-a`** — Ubuntu 24.10, kernel 6.11.0-29, root, cgroup v2 with all controllers
delegated, native overlay, `iptables-nft` 1.8.10 + `nft` 1.1.0, AppArmor on, Docker 28.3.1 /
containerd 1.7.27 / runc 1.2.5 live.

> ⚠️ **THIS IS `HOST-A` SERVING LIVE PUBLIC TRAFFIC ON :80 AND :443.**
> **Read-only. No exceptions.** Never start, stop, create, remove, or exec a container or
> image. Never touch `iptables` / `nft` / `ip link` / `ip route` / `ip addr`. Never write
> under `/sys/fs/cgroup`, `/etc`, `/var/lib/docker`, or any systemd unit.
> **Permitted:** `cat` / `ls` / `readlink` / `stat` under `/proc` and `/sys`; `docker inspect`,
> `docker info`, `docker version`; `nft list ruleset`; `iptables -S`; `ip -d link show`;
> `runc --version`; `ldd`; and `unshare` / `mount` **only** inside a private
> user+mount namespace rooted under `/tmp`, cleaned up afterwards.
> **If you are not certain a command is read-only, do not run it.** Record
> UNVERIFIABLE-HERE and name the command instead. That is always the correct trade.

**`ssh old-host`** — Rocky 8.8, kernel 4.18 (RHEL backports), **no root**, cgroup v1 with no
delegation (Slurm-owned), no `/etc/subuid` entries, NFS home, SELinux off. Unprivileged
userns, PID/mount/net namespaces, and overlay-mounted-in-a-userns all work. This is the
**degradation probe**: old kernel + rootless + cgroup v1 + no subuid. Write only under
`/tmp` and clean up.

**Local machine is macOS.** No kernel claim is locally verifiable here. Internet fetching
(`curl`, WebFetch, WebSearch) is your main instrument — prefer it.

## Hard constraints

- **DO NOT SPAWN SUB-AGENTS.** Do the work yourself.
- **DO NOT EDIT `D1`–`D5`, `README.md`, or any other verifier's file.** Write only your own.
- **WRITE INCREMENTALLY.** Append each finished part to your file as you complete it. If you
  are interrupted, whatever is on disk must still be useful. Never buffer the whole report
  to the end.

## Output

File: `/Users/mansur/D/projects/docker-projects/docker-rust/research/<YOUR-NAME>.md`

Structure: a short **Method and evidence base** header (what you read, what you ran, where),
then one part per theme, each part heading carrying its verdict. End with these two
mandatory sections, verbatim headings:

```
## Corrections that change a recommendation
```
The short list a future specification session must read. One line each:
*what the D-file said → what is actually true → which `[R]` it invalidates or shifts.*

```
## Still unverifiable, and the exact test
```
One line each: *the claim → the host and the exact command that would settle it.*

## Final message back to the orchestrator

≤50 lines. Counts of each verdict, then **only** the corrections that change a
recommendation, then the top three remaining risks. No preamble, no summary of what you did.
