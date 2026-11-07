# `RND.md` drafting contract

You are drafting assigned sections of **`RND.md`**, the single deliverable of this R&D effort.
Read this contract in full before writing a line.

## Who reads this document

**A completely fresh AI session that has read none of the research.** It will load `RND.md`'s
priority index, then pull individual sections on demand. It will then write a specification
and, after that, code. Assume it is competent and assume it knows nothing specific.

Consequences you must honour:
- **Never refer to `D1`–`D5` or `V1`–`V7` as context the reader has.** You may *cite* them as
  provenance (`[D1 §9; corrected by V1 §4]`), but every sentence must stand alone.
- **Never write "as established above" across a section boundary.** Sections are read in isolation.
- Define a term the first time it appears in *your* section, even if another section defines it.

## The three-category rule — the hardest rule in this document

Every claim is exactly one of these, and **they are never mixed in one sentence or one bullet**:

| Tag | Means | Test |
|---|---|---|
| **Fact** | What Linux or an OCI specification actually requires | Would be true if Docker had never existed |
| **Convention** | What Docker / containerd / runc / crun happen to do | Could change in a release and often has |
| **Recommendation** | What our Rust engine should do, and the tradeoff | Ours to decide; a reader may overrule it |

Write them as `**Fact.**` / `**Convention.**` / `**Recommendation.**` lead-ins, or as a tagged
table column. A paragraph that says "Linux requires X, so Docker does Y, so we should do Z" is
**three claims and must be three statements.** This is the single most common failure mode and
the reason the reader can trust the document at all.

## Verification status — you are writing the *corrected* corpus

The research was adversarially verified. For every claim you carry forward:

- **CONFIRMED** → state it plainly. Cite the primary source, not the research file.
- **CORRECTED** → **state the corrected fact only.** Never present the original claim as true.
  Where the *reasoning* changed but the conclusion held, give the correct reasoning — a future
  session that re-derives from a wrong reason will reach a wrong conclusion somewhere else.
  Where the error is instructive (a plausible mistake a reader would otherwise repeat), add a
  one-line **Trap.** note.
- **DISPUTED** → say it is open, give both positions and their evidence, and name **the decision
  that must be made**. Do not paper over it with a preference.
- **WEAK-EVIDENCE** → state it as uncertain and name the test that would settle it.
- **UNVERIFIABLE-HERE** → state it as unverified and name the exact host + command.

If something was falsified outright, it does not appear in `RND.md` at all — except where a
reader would plausibly rediscover it, in which case one **Trap.** line saying it is false and why.

## Operational writing — no vague systems prose

Banned: *"Docker uses namespaces for isolation."* *"cgroups limit resources."* *"OverlayFS
provides copy-on-write."* These are true and worthless.

Required instead: the mechanism, the syscall or file, the argument that matters, the failure
mode, and who must be privileged to do it. Compare:

> ~~The engine sets up a mount namespace for isolation.~~
>
> **Fact.** `clone(CLONE_NEWNS)` copies the caller's mount table into a new namespace. Copies
> are not isolated by default: a mount made in the child still propagates to the parent if the
> parent's mounts are in a shared peer group, which is the default on systemd hosts. Making the
> new namespace's `/` recursively `MS_SLAVE` (`mount(NULL, "/", NULL, MS_REC|MS_SLAVE, NULL)`)
> breaks propagation outward while still receiving host mount events. Requires `CAP_SYS_ADMIN`
> in the user namespace owning the mount namespace.

Every syscall gets its real signature or the flag that matters. Every kernel-version-gated
feature states its version. Every privileged operation states the capability and the namespace
it must hold in.

## Citations

Cite the **primary source**, pinned and recoverable:
- kernel: `v6.11/fs/namespace.c#L3736`, or `mount_namespaces(7)`, or `Documentation/admin-guide/cgroup-v2.rst`
- specs: `runtime-spec@v1.3.0/config.md §Root`
- runtimes: `runc@v1.2.5/libcontainer/init_linux.go#L336`
- advisories: `CVE-2025-31133`, `RUSTSEC-2026-0068`

**Never cite `main` or `master`** — they drift. Where a claim came from measurement, say so and
name the host and kernel: `measured: kernel 6.11, unprivileged userns`.

## Format

- Markdown. `##` for your assigned section headings **exactly as given** — the index links to them.
  `###` and below are yours to choose.
- Tables for anything with more than three parallel cases. Prose for mechanisms. Code fences for
  syscall sequences, file layouts, and command output.
- **Lead with the conclusion**, then the evidence. A reader who stops after the first paragraph
  of any subsection should have the actionable answer.
- Where a section forces an architectural decision, end it with a short
  **"What this decides"** block listing each decision and whether it is urgent.
- No filler. No "it is important to note". No recap of what the section will cover.

## Length

Write what the material needs. These sections are reference material that replaces ~24,000 lines
of research, so density is the goal — but a section nobody can finish reading has failed. As a
guide, most sections land between 150 and 500 lines; the creation sequence may run longer.

## Hard constraints

- **DO NOT SPAWN SUB-AGENTS.**
- **DO NOT EDIT** any `D*.md`, `V*.md`, `README.md`, or another drafter's fragment.
- **WRITE INCREMENTALLY** to your assigned fragment file, section by section. If you are
  interrupted, what is on disk must still be usable.
- Your fragment is **body text only** — no document title, no table of contents, no
  "Introduction". It will be concatenated with five others.

## Final message back to the orchestrator

≤40 lines: the headings you wrote with line counts, then the **decisions your sections force**
(each marked urgent or deferrable, one line each), then anything you could not resolve.
