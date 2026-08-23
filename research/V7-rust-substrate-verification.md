# V7 — Adversarial verification: the Rust implementation substrate

**Scope.** D2 §12 + §8, D1 §11, D3 §13, D4 §12, D5 §18 — every crate and Rust-language
claim in the corpus.

**Method.** Where a claim was checkable against a primary source, I checked it rather than
reasoning about it. `.crate` tarballs for `rustix 1.1.4`, `nix 0.31.3`, `caps 0.5.6`,
`seccompiler 0.5.0`, `tar 0.4.46` were downloaded from `static.crates.io` and grepped
directly. Registry metadata came from the crates.io JSON API, repository health from the
GitHub REST API, advisories from the RustSec advisory database, all on **2026-09-05**
(the same day the corpus was written — so the maintenance snapshots are same-day, and the
re-check is a re-derivation, not a refresh).

**Verdict key.** CONFIRMED / CORRECTED / REFUTED / OUTDATED / UNSUPPORTED / MISCLASSIFIED.

**One methodological note, recorded because I got it wrong first.** A RustSec advisory file
existing in the database is not the same as an advisory *standing*: advisories carry a
`withdrawn` field, and `cargo audit` ignores withdrawn ones. My first pass read
`ring`'s RUSTSEC-2025-0007 and `daemonize`'s RUSTSEC-2021-0147 as live; both are withdrawn
(2025-02-22 and 2023-02-19). Corrected in §2.5. Anyone repeating this sweep should grep for
`withdrawn` before reporting a hit, and should check `[versions] patched` and
`[affected.functions]` against the version actually being pinned.

**Headline.** The `rustix`-primary decision **survives every check I could make against
source**, and D2 §12's factual substrate is unusually accurate — dozens of coverage claims,
download counts, commit counts and author breakdowns re-derived and matching, several to the
digit. But the decision's *justification* is mis-weighted: the argument it leads with is the
weakest one it has, and the strongest argument for the same conclusion is one it never makes.

Four claims are **REFUTED**: D3's reasoning for trusting `tar`'s parser (a second 2026
advisory lives there), D2's preferred `signalfd` replacement (it silently drops the
supervisor's own `SIGTERM`), D3's stated reason for choosing `ureq` (it spawns threads), and
D5's "runc is the only dynamically-linked binary in the chain" (four others are). One health
claim is **OUTDATED** (`rust-vmm/seccompiler` was archived into a monorepo eighteen days
before the corpus was written). One **cross-file conflict is live and unresolved** — D4
requires a dedicated OS thread that D2 §8.3 forbids, and each document holds one half of the
resolution without combining them. One question the corpus flagged as unresolvable is now
**settled from primary sources**, and the original agent's refusal to speculate was correct.

---

## 1. The foundational decision — `rustix` primary, `libc` for numbers and layouts

### 1.1 The syscall-coverage table (D2 §12.1.2) — CONFIRMED, essentially line by line

Every row I could check against `rustix-1.1.4` and `nix-0.31.3` sources held.

| Claim | Verdict | Evidence |
|---|---|---|
| rustix has `pidfd_open`, `pidfd_send_signal`, `pidfd_getfd` | **CONFIRMED** | `src/process/pidfd.rs:29`, `:41`, `src/process/pidfd_getfd.rs:50` |
| nix has none of the three | **CONFIRMED** | zero occurrences of `pidfd` in all of `nix-0.31.3/src/` |
| nix has no `clone3` | **CONFIRMED** | zero occurrences of `clone3` in `nix-0.31.3/src/` |
| nix has no `capget`/`capset` | **CONFIRMED** | zero occurrences of either token in `nix-0.31.3/src/` |
| rustix has the full capability surface | **CONFIRMED** | `thread/libcap.rs:123 capabilities`, `:136 set_capabilities`; `thread/prctl.rs:467 capability_is_in_bounding_set`, `:489 remove_capability_from_bounding_set`, `:698 capability_is_in_ambient_set`, `:725 clear_ambient_capability_set`, `:739 configure_capability_in_ambient_set`, `:46/:59 get/set_keep_capabilities`, `:556/:570 capabilities_secure_bits` |
| rustix `set_thread_res_uid` is the raw, non-broadcasting per-thread call | **CONFIRMED, and stronger than stated** | `src/thread/id.rs:108`. Its own doc comment reads: *"This is not the `setresuid` you are looking for… this call only changes the uid for the current **thread**, not the entire process even though that is in violation of the POSIX standard… This call implements the kernel behavior."* It calls `backend::thread::syscalls::setresuid_thread`. This is exactly the D1 §11.2 glibc-`setxid`-broadcast hazard, solved and documented. |
| rustix has `signalfd`: **no** | **CONFIRMED** | `signalfd` appears in `rustix-1.1.4/src/` **only** at `not_implemented.rs:300`, inside `pub mod yet`. Zero implementation anywhere. |
| nix has `signalfd` | **CONFIRMED** | `src/sys/signalfd.rs:50 pub fn signalfd`, `:97 pub struct SignalFd` |
| `mount_setattr` in `not_implemented::yet` | **CONFIRMED** | `not_implemented.rs:301` |
| nix has `openat2` (D2's correction of D1) | **CONFIRMED** | `nix-0.31.3/src/fcntl.rs:389` |
| `nix::sched::clone` is `pub unsafe fn` | **CONFIRMED** | `nix-0.31.3/src/sched.rs:107` |
| rustix `Errno` is `pub struct Errno(u16)` in linux_raw | **CONFIRMED** | `src/backend/linux_raw/io/errno.rs:51` |
| `nix::Result<T> = Result<T, Errno>`, `Errno` a plain enum | **CONFIRMED** | `nix-0.31.3/src/lib.rs:229`; `errno.rs` defines it as a `#[repr]` enum per-platform |

### 1.2 The `clone3`/`execve` "out of scope" claim (priority 2) — CONFIRMED verbatim

`rustix-1.1.4/src/not_implemented.rs:66` declares `pub mod libc_internals`, and it does
contain `exit`, `fork`, `clone`, `clone3`, `brk`, `sigaction` — plus a longer list the
corpus did not quote: `sigaltstack`, `sigprocmask`, `sigwait`, `set_tid_address`, `tkill`,
`rseq`, and **`setuid`/`setgid`/`seteuid`/`setegid`/`setreuid`/`setregid`/`setresuid`/
`setresgid`/`setgroups`** (the *process-wide POSIX* semantics — the per-thread kernel calls
live in `thread::id` as verified above; no contradiction, but worth knowing that the name
`setresuid` is deliberately absent while `set_thread_res_uid` is present).

The module docstring is quoted accurately by the corpus. `execve`/`execveat`/`_exit`/
`exit_group` are in `quite_yet` at `:332-339`, whose docstring names issue **#1314**.

**So the rhetorical claim holds: `clone3` and `execve` are the two syscalls at the centre
of this project and no substrate crate will provide them.** Verified against source, not
docs.

**Does any other crate provide `clone3`?** — see §1.3 below. The answer changes the framing
slightly but not the conclusion.

### 1.3 CORRECTED — "no crate is going to abstract `clone3` for us" is not quite true

Checked crates.io directly. `clone3` **does exist as a crate** (`clone3`, `0.2.3`), and
`nc 0.9.x` — which **youki already depends on** — exposes `nc::clone3`. `syscalls 0.8.x`
gives a typed `Sysno::clone3` plus an inline-asm `syscall!` macro.

This does not change the `[R]`: none of these gives you the *hard* part (the child stack,
the `CLONE_INTO_CGROUP` fd, the pidfd return, the closure-ownership problem youki solves
with double-boxing), and taking a fourth syscall-numbering source contradicts D2 §12.13's
own row on `syscalls`/`nc`. **But the corpus states as fact something that is a
preference.** The honest statement is: *no crate abstracts `clone3` at the level we need,
and the crates that do expose it are thinner than `libc::syscall(SYS_clone3, …)` is.*
Marked **MISCLASSIFIED** (recommendation stated as an availability fact), conclusion
unaffected.

### 1.4 The `linux_raw`-is-default claim — CONFIRMED, with one fact the corpus omitted

`rustix-1.1.4/Cargo.toml` lines 194–228 confirm the predicates exactly as described: on
`cfg(all(not(rustix_use_libc), not(miri), target_os="linux", …, target_arch="x86_64"))`
the crate pulls `linux-raw-sys 0.12` with `default-features = false`, and `libc 0.2.182`
appears there only as `optional = true`, enabled by the **`use-libc`** feature (`[features]`
line 143). Default is `["std"]`, which does **not** enable `use-libc`. So on
`x86_64-unknown-linux-{gnu,musl}` with default features, rustix issues syscalls by inline
asm and does not link libc *for its own calls*. **CONFIRMED.**

**Omitted fact that matters:** we are separately depending on `libc` for `SYS_*` and struct
layouts, and Rust's global allocator is libc `malloc` on gnu targets (D1 §11.2 says so).
So "libc is not in the process" is never true for us. The correct and narrower statement —
which D2 §12.1.1's `[R]` does actually make ("a syscall that does not route through libc
cannot deadlock on a libc lock") — is the one to keep. The §12.1.7 justification #1
restates it more loosely as *"structurally reduces [the post-clone hazard]"* and *"removes
one entire class of post-clone hazard for free."* See §1.5.

### 1.5 UNSUPPORTED as stated — justification #1, the load-bearing argument, is the weakest one

D2 §12.1.7 ranks its six reasons "in order of weight" and puts first:

> *"Post-clone safety is the project's single largest correctness risk (§8.3), and rustix
> is the only option that structurally reduces it… `nix` routes every call through glibc."*

**What the cited evidence establishes:** that rustix's syscalls do not pass through a glibc
PLT entry. That is true.

**What it does not establish:** that this removes a hazard *class*. Enumerate what actually
takes a lock in glibc, because that is the claim's real content:

- `malloc` / `free` — arena locks. **Not avoided by rustix**; the Rust global allocator is
  still libc `malloc`. This is the hazard that matters and rustix does nothing about it.
- the `setxid` family — signal broadcast to all registered threads. **Genuinely avoided**,
  by `set_thread_res_uid`. This is a real win and it is justification #3, ranked below #1.
- stdio `FILE` locks, NSS/`getpwnam`, `dlopen`, locale. **Not called by any thin syscall
  wrapper.** Avoiding them is a matter of not calling them, not of which crate wraps
  `mount(2)`.
- glibc's own thin wrappers for `mount`, `unshare`, `setns`, `pivot_root`, `prctl`:
  these take **no lock at all**. They marshal arguments, issue `syscall`, and set `errno`
  through `__errno_location()`, a TLS read.

So the class rustix removes from *these* calls is: one TLS access per call. That is not
nothing — but it is a much smaller thing than "one entire class of post-clone hazard."

**The stronger argument the corpus never makes, and should:** `__errno_location()` reads
thread-local storage. In a child created by `clone` **with `CLONE_VM` and without
`CLONE_SETTLS`** — the shape youki uses for its intermediate process, and the shape the
`nix::sched::clone` soundness bugs are about — the child's TLS pointer still refers to the
*parent's* thread struct. Every glibc wrapper in that child writes `errno` into the
parent's slot. That is a genuine data race with a genuine wrong-errno failure mode, and
rustix's linux_raw backend structurally cannot have it, because it returns the error in the
register with no TLS access whatsoever. **That is a concrete, checkable, mechanism-level
reason, and it is strictly better than the one §12.1.7 leads with.** Recommend rewriting
justification #1 around it, or demoting #1 below #3.

Verdict: **UNSUPPORTED** as written (the magnitude claim is not established by the cited
evidence); the *decision* is unaffected and is independently supported by justifications
#2–#6, which I verified above and which are all CONFIRMED.

### 1.6 The "only crate with" list — CONFIRMED against `nix`, UNSUPPORTED as a universal

The decision table row reads *"Only crate with `linux_raw` no-libc syscalls, pidfds, the
full capability/prctl surface, non-broadcasting `setresuid` and the new mount API."*

Against `nix` — the only alternative the section actually evaluated — every element is
CONFIRMED above. As a universal claim over crates.io it is **UNSUPPORTED**, and two
counterexamples exist: the `caps` crate covers the five capability sets (D1 §11.1 says so
itself, and I confirmed `caps-0.5.6` has bounding and ambient), and `nc` exposes the new
mount API (which is exactly why youki uses `nc` for it — D2 §12.8 records this and then
the table forgets it). The right phrasing is *"the only crate that has all of these at
once, and the only one with the no-libc property at all."* That phrasing is true and is
sufficient for the decision.

---

## 2. Advisories and licences — the highest-consequence factual claims

Source: the **RustSec advisory database**, cloned fresh at HEAD `5a0ebed` (2026-09-02), and
`cveawg.mitre.org` for CVE records. I swept **57 crates** — every crate any of the five
documents recommends, defers, or evaluates.

### 2.1 RUSTSEC-2026-0067 — CONFIRMED, verbatim

`advdb/crates/tar/RUSTSEC-2026-0067.md`. Every element of D3 §13.3's claim checks out:

- `package = "tar"`, `date = "2026-03-19"`, `aliases = ["CVE-2026-33056", "GHSA-j4xf-2g29-59ph"]`
- `[affected.functions]`: `"tar::Entry::unpack" = ["< 0.4.45"]`, `"tar::Entry::unpack_in" = ["< 0.4.45"]`
- `patched = [">= 0.4.45"]`
- The sentence D3 quotes is **verbatim** from the advisory text, `fs::metadata()` and all.

D3's `[R] MED` "pin `tar >= 0.4.45`" is correct and necessary.

### 2.2 REFUTED — D3's reasoning for taking `tar` as a parser. RUSTSEC-2026-0068 was missed.

**There is a second `tar` advisory, filed the same day, that D3 does not mention, and it is
the one that actually bites D3's plan.**

`RUSTSEC-2026-0068` / **CVE-2026-33055** / GHSA-gchp-q4r4-x4ff, `date = 2026-03-19`,
`patched = [">= 0.4.45"]`:

> *"tar-rs incorrectly ignores PAX size headers if header size is nonzero… Any discrepancy
> in how tar parsers honor file size can be used to create archives that appear differently
> when unpacked by different archivers. In this case, the tar-rs (Rust tar) crate is an
> outlier in checking for the header size — other tar parsers (including e.g. Go
> `archive/tar`) unconditionally use the PAX size override."*

Note what this advisory has that 0067 does not: **no `[affected.functions]` block.** 0067 is
scoped to `unpack`/`unpack_in`. 0068 is in the **header-parsing** path — precisely and only
the part D3 recommends keeping.

D3 §13.3's load-bearing sentence is:

> *"the parsing is boring, well-tested, and not where the danger lives; the **applying** is
> where every CVE in this area lives"*

**REFUTED.** Not every CVE in this area lives in the applier. One of the two 2026 `tar`
advisories lives in the parser, and three of the five `astral-tokio-tar` advisories
(RUSTSEC-2025-0110, 2026-0112, 2026-0145) are PAX-header desynchronisation — also parser
bugs. For a container-image layer applier a parser desync is not a lesser bug than a chmod
bug: it is a **layer-content-versus-digest confusion primitive**, an archive whose content
differs between our engine and Docker's while both accept the same digest. That is squarely
in D3's own threat model.

The *recommendation* (`tar` for headers, write the applier) survives — hand-writing a PAX
parser would be worse. But the **justification must change**, and two things follow:

1. D3 §13.3's `[R] MED` on `cargo-audit` in CI should be `[R] HIGH`. It is the only thing
   standing between us and the next parser advisory, and "we don't call the vulnerable
   function" is not a defence against 0068 — there is no vulnerable function named.
2. Add a differential-parse test to the plan: unpack the same crafted archive with our
   applier and with Go's `archive/tar` (or `bsdtar`) and require identical output. D3's
   §6 already contemplates crafted archives; this is the specific case.

### 2.3 CORRECTED — the two companion CVEs are misattributed

D3 §13.3 writes: *"The same shape of bug has hit `async-tar` (CVE-2025-62518) and
`astral-tokio-tar` (CVE-2025-59825)."*

- **CVE-2025-59825 → `astral-tokio-tar`.** Correct. MITRE record: *"astral-tokio-tar has a
  path traversal in tar extraction"*, affected `< 0.5.4`.
- **CVE-2025-62518 → NOT `async-tar`.** It maps to **RUSTSEC-2025-0110** (`astral-tokio-tar`,
  *"Vulnerable to PAX Header Desynchronization"*, patched `>= 0.5.6`) and **RUSTSEC-2025-0111**
  (`tokio-tar`, *"parses PAX extended headers incorrectly, allows file smuggling"*,
  `patched = []` — i.e. **never fixed**). The crate `async-tar`
  (`dignifiedquire/async-tar`, 0.6.1) has **no advisory in the RustSec database at all**.
- And the "same shape" framing is wrong for that CVE: 62518 is a **PAX desync**, i.e. the
  same shape as 0068 (the one D3 missed), not as 0067 (the one D3 cites).

Correction of record: the `astral-tokio-tar` line has **five** advisories
(2025-0110, 2026-0066, 2026-0112, 2026-0113, 2026-0145), the latest patched only in 0.6.2.
The tar ecosystem's *parser* layer is under sustained, active attack. That is the real
lesson, and it is the opposite of D3's "the parsing is boring".

### 2.4 `rustables` is GPL-3.0-or-later — CONFIRMED

crates.io metadata for `rustables 0.8.8` (2026-06-23):
`license = "GPL-3.0-or-later"`, `repository = "https://gitlab.com/rustwall/rustables"`,
20,210 downloads in the last 90 days. D4 §12.3's figures match to the digit.

D4's verdict — skip, and the licence is the stronger reason — is **CONFIRMED and correct**.
Linking a GPL-3.0-or-later crate into an engine intended to be MIT/Apache would be a
project-level relicensing smuggled in through `Cargo.toml`. This is the single most
consequential correct catch in the corpus.

For contrast, verified in the same pull: `nftables` (nftables-rs) 0.6.3 is
`MIT OR Apache-2.0`; `nftnl` 0.9.4 is `MIT OR Apache-2.0`; `neli` 0.7.4 is `BSD-3-Clause`;
the three `netlink-*` crates are `MIT`; `rustix` is
`Apache-2.0 WITH LLVM-exception OR Apache-2.0 OR MIT`; `oci-spec` and `seccompiler` are
`Apache-2.0` (seccompiler dual with BSD-3-Clause). No other licence landmine in the set.

### 2.5 New advisory findings the corpus did not report

| Crate | Advisory | Status vs. the corpus |
|---|---|---|
| **`tar`** | **RUSTSEC-2026-0068** (CVE-2026-33055), parser, patched ≥ 0.4.45 | **Missed.** See §2.2. Same pin fixes it; the *reasoning* does not survive. |
| **`nftnl`** | **RUSTSEC-2025-0126** — *"Heap-buffer-overflow in `nftnl::Batch::with_page_size`"*, patched ≥ 0.9.0 | **Missed.** D4 §12.3 spends a paragraph on `nftnl`'s health, calls it *"real production code… actively released"*, counts its 194 `unsafe` — and does not mention that those `unsafe` blocks produced a heap buffer overflow advisory. D4's verdict (read, don't adopt) is unaffected and is now better supported than D4 knew. |
| **`ring`** | RUSTSEC-2025-0007 *"`*ring*` is unmaintained"* — but **`withdrawn = "2025-02-22"`**, two days after filing. RUSTSEC-2025-0010 applies only to *"versions prior to 0.17"*; D3 cites 0.17.14. | **No finding. D3 was right and I was initially wrong** — I first read 0007 as live. `ring 0.17.14` is flagged by neither advisory, and `cargo audit` reports withdrawn advisories not at all. **D3 §13.5's `[R] MED` "excellent crypto but… slow-moving" is exactly the correct characterisation** — not an unmaintained verdict, and it should not be upgraded to one. Recorded because it is the trap this cluster is about: an advisory file existing is not an advisory standing. |
| **`daemonize`** | **RUSTSEC-2025-0069** (2025-09-14, `informational = "unmaintained"`) — live. RUSTSEC-2021-0147 is **withdrawn** (2023-02-19). | **Strengthened, by one advisory not two.** D2 §12.10 says "last published 2023-02-25, do not use" and reasons from the *design* — which remains the better argument. One live RustSec unmaintained advisory backs it. |
| **`anyhow`** | RUSTSEC-2026-0190, unsound, `Error::downcast_mut()`, patched **≥ 1.0.103** | Not mentioned. D2 §12.6 cites `anyhow 1.0.104`, which is above the floor. **No action beyond writing the floor into `Cargo.toml`.** |
| **`tracing-subscriber`** | RUSTSEC-2025-0055, *"Logging user input may result in poisoning logs with ANSI escape sequences"*, patched **≥ 0.3.20** | Not mentioned, and it is on-topic: D2 §12.7 adopts `tracing` in the supervisor, whose log lines contain container IDs, image references and child error strings — all attacker-influenced. D2 cites 0.3.23, above the floor. **Write the floor down and do not regress it.** |
| **`tracing`** | RUSTSEC-2023-0078, unsound, patched ≥ 0.1.40 | Cited version 0.1.44 is clear. |

**Clean — no advisory of any kind:** `rustix`, `libc`, `clap`, `serde`, `serde_json`,
`thiserror`, `oci-spec`, `ureq`, `reqwest`, `flate2`, `zstd`, `sha2` (at 0.11), `caps`,
`cgroups-rs`, `libcontainer`, `libcgroups`, `liboci-cli`, `pentacle`, `prctl`, `procfs`,
`signal-hook`, `seccompiler`, `libseccomp`, `aya`, `libbpf-rs`, `netlink-packet-route`,
`netlink-packet-core`, `netlink-sys`, `rtnetlink`, `neli`, `nftables`, `rustables`,
`iptables`, `ipnet`, `fs2`, `oci-client`, `oci-distribution`, `clone3`, `nc`, `syscalls`,
`sc`, `zbus`, `rayon`, `subtle`, `miniz_oxide`. The primary substrate is clean.

---

## 3. Maintenance health — re-derived, and one crate has moved

Re-derived from `api.github.com` (`/repos`, `/commits?since=2026-03-09`) and the crates.io
JSON API on **2026-09-05**.

**The corpus's numbers are accurate.** This deserves saying, because the brief warned that
maintenance judgements are where preference gets laundered as observation. I re-derived
them and they match, several of them to the digit:

| Claim | Corpus | Re-derived | Verdict |
|---|---|---|---|
| `nix` open issues+PRs | 354 | `open_issues_count: 354` | **CONFIRMED** |
| `nix` commits/180d | 34 (xtqqczze ×11, SteveLauC ×6, kamalmarhubi ×3, cgzones ×2, +2) | 34 (xtqqczze ×11, SteveLauC ×6, kamalmarhubi ×3, cgzones ×2, wngr ×1, oech3 ×1) | **CONFIRMED exactly** |
| `rustix` commits/180d | 8, six named authors | 8, the same six | **CONFIRMED exactly** |
| `caps-rs` commits/180d | 0, last push 2025-10-17 | 0, `pushed_at 2025-10-17` | **CONFIRMED** |
| `oci-spec-rs` commits/180d | 40 (saschagrunert ×7, flavio ×6, tommady ×6, nayuta723 ×3, DavSanchez ×2, dependabot ×12) | identical, author for author | **CONFIRMED exactly** |
| 90-day downloads (`rustix` 246M, `nix` 172M, `caps` 7.38M, `oci-spec` 4.39M, `seccompiler` 5.75M, `libseccomp` 346K, `nftables` 551k, `nftnl` 59k, `neli` 10.1M, `rustables` 20.2k) | as stated | 246,068,760 / 171,746,140 / 7,376,573 / 4,392,733 / 5,746,160 / 346,362 / 551,117 / 58,947 / 10,123,716 / 20,210 | **CONFIRMED, all ten** |
| `tar`'s repo is now `composefs/tar-rs` | flagged by D2 §12.12 | crates.io `repository` for `tar 0.4.46` = `https://github.com/composefs/tar-rs` | **CONFIRMED** — D2 was right to flag it and D3 should close it |
| `libc` latest = 0.2.189, 2026-07-21 | as stated | `max_stable_version = 0.2.189` (2026-07-21). Note `max_version` is **`1.0.0-alpha.4`**, published the same day — a pre-release 1.0 line now exists | **CONFIRMED**, with a note: pin the `0.2` line deliberately, not by accident |

### 3.1 OUTDATED — `rust-vmm/seccompiler` was archived on 2026-08-18

**The one health fact the corpus got wrong, and it is on a crate two documents recommend.**

`GET /repos/rust-vmm/seccompiler` returns `"archived": true`, `pushed_at 2026-08-18`, and a
repository description that says so in plain words:

> *"This is a public archive. The code now lives in the mono-repo:
> https://github.com/rust-vmm/rust-vmm/"*

D2 §12.11 and D5 §18.2 both cite `rust-vmm/seccompiler` as the repository and both argue
from its health — D2: *"5.75M dl/90d and rust-vmm/Firecracker provenance is stronger
maintenance evidence than `libseccomp`'s 346K."* Neither noticed the archive, which
predates the corpus by 18 days.

**This is NOT an "unmaintained" finding, and I want to be explicit about that**, because the
brief specifically warns against calling a moved or finished crate dead. seccompiler was
**consolidated into a monorepo**, not abandoned. But three practical consequences follow and
none of them is in the corpus:

1. The crates.io `repository` link — which is what a reader follows — now lands on a
   **read-only archive with a closed issue tracker**. Bugs and questions go to
   `rust-vmm/rust-vmm`. The corpus's citation is a dead end.
2. `seccompiler 0.5.0` was published **2025-03-07** — eighteen months before the corpus.
   D2 §12.11's health argument rests on downloads, not release cadence, and the cadence is
   not obviously better than `libseccomp`'s.
3. It is the **same shape of finding** as the `containers/youki` → `youki-dev/youki` move
   that D2 §12.12 flagged, right down to the crate metadata lagging the move. D2 §12.12
   closes with *"Re-run the checks before writing any `Cargo.toml`. Maintainership is the
   one property in this section with a short half-life."* That discipline is correct and it
   was **not applied to D5's crates.** Apply it there too.

### 3.2 The counterpart — `libseccomp-rs` has 0 commits in 180 days

`GET /repos/libseccomp-rs/libseccomp-rs`: `pushed_at 2026-06-19`, **0 commits since
2026-03-09**, 49 stars, 11 open issues. Neither D2 §12.11 nor D5 §18.2 checked this.

So the seccomp choice is between **an archived-into-a-monorepo crate last released 18
months ago** and **a C-FFI binding with no commits in six months**. Neither is a strong
maintenance story. This does not change the verdict — D5 §18.2's actual recommendation is
*"write the `sock_filter` array by hand, keep `seccompiler` as the escape hatch"*, and this
finding makes the hand-written path **more** attractive, not less. But D2 §12.11's stated
reason for preferring seccompiler ("stronger maintenance evidence") is **OUTDATED**;
the surviving reasons are the real ones: pure Rust, no C toolchain, static-musl preserved.

### 3.3 CORRECTED — D4's bus-factor argument against `neli` applies equally to the crate it prefers

D4 §12.2 rejects `neli` partly on this:

> *"Maintenance is **one person**, consistently, since 2018… it is a bus factor of one on
> our most load-bearing dependency, versus `rust-netlink`, which is a GitHub
> **organisation** maintained by the nmstate/Red Hat networking people."*

Re-derived:

- `jbaublitz/neli`: 3 commits/180d — and by **two** people (`epilys` ×2, `jbaublitz` ×1).
  So even the narrow claim is now slightly stale.
- `rust-netlink/netlink-packet-route`: 88 commits/180d, of which **`cathay4t` ×78** — 89%
  by one person. (`cathay4t` is Gris Ge; the nmstate/Red Hat attribution D4 rated `[LOW]`
  is correct.)
- `rust-netlink/netlink-sys`: **4** commits/180d, `cathay4t` ×3.

**The bus factor is one on both sides.** `rust-netlink` is an organisation the way
`bytecodealliance` is for rustix — governance cover over one person's code. D2 §12.1.5
found exactly this about rustix and said so plainly ("the *code* is one person's"); D4 used
the same property as a *discriminator* without checking whether its preferred crate shares
it. It does.

D4's **conclusion still stands** — but only on its *other*, stronger ground: coverage.
`netlink-packet-route` has `InfoVeth::Peer(LinkMessage)` and 293 attribute files; `neli`
does not. Activity (88 vs 3 commits) is also a real difference. **Drop the bus-factor
argument; keep the coverage argument.** Marked **CORRECTED**: a maintenance judgement stated
as an observed discriminator, where the observation does not discriminate.


---

## 4. The allocation hazard — thresholds CONFIRMED, the proposed fix is NOT SUFFICIENT

### 4.1 The thresholds and the fallbacks — CONFIRMED exactly

| Claim | Verdict | Evidence |
|---|---|---|
| `nix`: `MAX_STACK_ALLOCATION = 1024`, `>= 1024` falls to `CString::new` | **CONFIRMED** | `nix-0.31.3/src/lib.rs:329` `const MAX_STACK_ALLOCATION: usize = 1024;` · `:331` `if self.len() >= MAX_STACK_ALLOCATION { return with_nix_path_allocating(self, f); }` · `:354/:358` `fn with_nix_path_allocating` → `CString::new(from)` |
| `rustix`: `SMALL_PATH_BUFFER_SIZE = 256`, `>= 256` falls to `CString::new` | **CONFIRMED** | `rustix-1.1.4/src/path/mod.rs:9` · `src/path/arg.rs:959` `if bytes.len() >= SMALL_PATH_BUFFER_SIZE { return with_c_str_slow_path(bytes, f); }` · `:997` `f(&CString::new(bytes)…)` |
| `impl Arg for &CStr` is `f(self)` — no copy, no allocation | **CONFIRMED** | `src/path/arg.rs:482-488` and `:517-523`, both literally `f(self)` |

The cited line ranges are accurate. The hazard is real and is exactly as described.

### 4.2 The mitigation the corpus did not find — rustix without `alloc` cannot allocate here at all

`with_c_str_slow_path` is **feature-gated**. Under `#[cfg(not(feature = "alloc"))]`
(`arg.rs:1000-1035`) it uses a `MaybeUninit<[u8; PATH_MAX]>` stack buffer and returns
`Errno::NAMETOOLONG` for anything longer. **There is no allocation on any path.**

`[features]`: `default = ["std"]` and `std = ["bitflags/std", "alloc", …]`. So the
recommended feature set (§12.1.7: "rustix default features + […]") turns the allocating
path **on**. Turning it off means `default-features = false` — i.e. `no_std` rustix.

Honest assessment, since this looks like a free win and is not one: Cargo unifies features
across a single build graph, so a workspace whose CLI enables `rustix/std` turns `alloc`
back on for the child code too. The `no_std` route therefore only works if the child is a
**separately built artifact**, not merely a separate module or even a separate workspace
member. That is a real option and it is the same shape of answer as §8.3's "separate
process for the pull path" — **structural, not disciplinary** — but it costs `std` in the
child, which is a large ask. **Record it as an option; do not adopt it now.** It is worth
knowing that a hard guarantee exists, because the `&CStr` rule below is only a soft one.

### 4.3 CORRECTED — the `&CStr`-in-signatures rule is necessary but not sufficient

D2 §12.1.3's `[R]` HIGH is: *"enforce it with a newtype: the child's setup functions take
`&CStr` in their signatures and nothing else, so a `&str` is a compile error rather than a
latent deadlock."*

Good rule. It closes the **path-argument** hole and nothing else. The child, by D2 §8.2's
own list, must *"mount ~8 filesystems, `pivot_root`, `sethostname`, write sysctls, drop
capabilities, load a seccomp filter, set rlimits, open the exec fifo"* — and at least four
of those allocate through channels the `&CStr` rule does not touch:

1. **The seccomp filter program. This is the big one and it is unaddressed anywhere in the
   corpus.** D5 §18.2 recommends `seccompiler`; verified in `seccompiler-0.5.0`:
   `src/backend/bpf.rs:9` — `pub type BpfProgram = Vec<sock_filter>`. **Compiling a filter
   allocates a `Vec`.** D2 §8.2 explicitly names "the seccomp program" as a thing the child
   "wants to allocate", flags it as a hazard, and then neither D2 §12.11 nor D5 §18.2 ever
   resolves it.
   **The resolution exists and is clean, and it must be written down as a rule:**
   `src/backend/bpf.rs:12` defines `pub type BpfProgramRef<'a> = &'a [sock_filter]`, and
   `apply_filter`/`apply_filter_all_threads` (`src/lib.rs:312`, `:324`) take
   `BpfProgramRef`. I read `apply_filter_with_flags` (`:335-365`): it builds `sock_fprog`
   **on the stack**, calls `prctl(PR_SET_NO_NEW_PRIVS)` and `syscall(SYS_seccomp, …)`, and
   allocates nothing. So: **compile the filter to a `Vec<sock_filter>` before the clone,
   pass the child a `&[sock_filter]`, install with `apply_filter`.** Same rule as argv/envp,
   different subsystem, and it needs to be in §12.1.3's one-liner alongside paths.
2. **argv / envp.** The child needs `*const *const c_char` arrays. D1 §10.1 step 7 covers
   this ("every `CString` for path, argv and envp must be built before the clone") but
   §12.1.3's `&CStr` signature rule does not — an array of pointers is not a `&CStr`. Needs
   its own type: pass the child a pre-built `&[*const c_char]`.
3. **Reading and writing `/proc` in the child** — `uid_map`/`gid_map`/`setgroups`, sysctls,
   `/proc/self/attr/…` for AppArmor/SELinux (D5 §18.5 says these are `std::fs::write`).
   `std::fs::write` allocates (it opens a `File`, and `read_to_string` certainly does).
   Nothing in the `&CStr` rule prevents `std::fs::write` in the child. **Rule needed:
   the child does `openat`/`write` on pre-opened fds, never `std::fs`.**
4. **`rustix` functions that allocate by contract**, regardless of argument type:
   `thread::name()`/`process::name()` return `CString`; `fs::readlink` grows a `Vec`.
   None are needed in the child, but nothing stops one being called.

**Verdict: CORRECTED.** The `&CStr` rule is right and should be adopted; the claim that
*"With that discipline both crates are allocation-free on the paths we use"* is true only
of paths, and the child touches three other allocation channels. The seccomp one is the
one that will actually bite, because it is the only one where the corpus recommends a
crate whose primary API returns an owned `Vec`.

**The enforcement mechanism that would actually cover all four:** D1 §3.4 / D2 §12.7's
`Threads == 1` assert catches thread-spawning but not allocation. For allocation, the
child's entry point can install a `#[global_allocator]` shim whose `alloc` aborts (or
writes a byte to the error pipe and `_exit`s) once a static `IN_CHILD` flag is set. That
turns every one of the four holes from a nondeterministic deadlock into a deterministic,
loud failure at the exact call site. It is ~20 lines and it is the only mechanism that
does not depend on auditing every transitive call. Recommend it alongside the `&CStr` rule.

---

## 5. `signalfd` — absence CONFIRMED; option (c) is NOT equivalent, and option (b) costs more than stated

### 5.1 The absence — CONFIRMED

`signalfd` appears in `rustix-1.1.4/src/` at exactly one location: `not_implemented.rs:300`,
inside `pub mod yet`. There is no implementation. `nix-0.31.3/src/sys/signalfd.rs` has
`pub fn signalfd` and `pub struct SignalFd`. D2 §12.1.2's row is correct.

### 5.2 CORRECTED — option (b) "hand-write it, it's a 3-argument syscall" understates the cost

D2 §12.1.7 option (b): *"hand-write it (`signalfd4` is a 3-argument syscall and
`struct signalfd_siginfo` is a fixed 128-byte layout in `libc`)."*

True as far as it goes. But `signalfd` is useless without first **blocking** the signals in
question — otherwise default disposition fires and the process dies before you read the fd.
And `sigprocmask` is in **`rustix::not_implemented::libc_internals`** (`not_implemented.rs:74`,
verified above), alongside `sigaction`, `sigaltstack`, `sigwait`, `sigwaitinfo` and
`sigtimedwait`. rustix refuses the entire signal-mask surface **by policy**, for the same
reason it refuses `fork`.

There *is* a `rustix::runtime::kernel_sigprocmask` (`src/runtime.rs:596`) — but the module
doc is unambiguous and D1 §11.1 quotes it correctly:

> *"Some of the functions in this module cannot be used in a process which also has a libc
> present… they invoke Undefined Behavior if called in a process which has a libc present…
> This module is intended to be used for implementing a runtime library such as libc. Use
> of these features for any other purpose is likely to create serious problems."*

D1 §11.1's "do not build on `rustix::runtime`" is **CONFIRMED and correct.**

So option (b) is: `libc::sigprocmask` (or `pthread_sigmask`) **plus** a raw `signalfd4`
**plus** the 128-byte struct. Two libc entry points, not zero — which is worth noting
because "no libc in the child" was §12.1.7's headline argument. The cost is still small
(~25 lines), but it should be priced correctly.

*(Incidental correction while I was there: D2 §12.1.2 says rustix "has [`execve`]
internally, not exposed". Sharper: `rustix::runtime::execve(path: &CStr, argv: *const
*const u8, envp: *const *const u8) -> io::Errno` **is** reachable behind
`feature = "runtime"` — `src/runtime.rs:484`, and it has exactly the allocation-free
signature the child wants. The reason not to use it is that the module is `doc(hidden)`
and explicitly unstable, not that it does not exist. Verdict on the recommendation
unchanged: write our own; ~5 lines of `syscall(SYS_execve, …)` beats depending on a
`doc(hidden)` module for the single most important call in the project.)*

### 5.3 REFUTED — option (c), the pidfd route, is not equivalent, and D2's own caveat is too weak

D2 §12.1.7 option (c), rated `[R]` MED and **preferred**: *"drop signalfd: `pidfd_open` the
container init and `poll` the pidfd for exit… then do a `waitpid(-1, WNOHANG)` sweep for
subreaped orphans on each wake."*

Three losses, of which D2 names only the third:

1. **The supervisor loses the ability to receive its own signals — at all.** `signalfd`
   delivers *every* signal directed at the supervisor: `SIGTERM` from `systemctl stop` or
   an operator, `SIGINT`, `SIGHUP`. D2 §5.5 requires the supervisor to *forward* `SIGTERM`
   to the container's PID 1, and D2 §12.10 states *"we do not want a signal handler at
   all."* Under option (c) there is no signalfd and no handler, so a `SIGTERM` to the
   supervisor takes **default disposition: the supervisor dies immediately**, mid-`poll`,
   without forwarding anything, without writing an exit status to `state.json`, and leaving
   the container running orphaned. **This is a functional regression, not a caveat**, and
   it is unmentioned in §12.1.7. Option (c) as written cannot implement §5.5.
2. **A pidfd wakes only on the direct child's exit.** D2's mitigation — *"a `waitpid(-1,
   WNOHANG)` sweep on each wake"* — assumes wakes happen. With no `SIGCHLD` path into the
   loop, **an orphan reparented by `PR_SET_CHILD_SUBREAPER` produces no wake at all**, so
   it stays a zombie until the container's init exits and the pidfd finally fires. "The
   sweep is not optional" is right; the sweep having something to run on is the missing
   half. Severity is genuinely reduced by D2 §7.2's own design — the container's init is
   PID 1 in its own PID namespace, so *in-container* orphans reparent to it, not to us, and
   host-side subreaped orphans are limited to hooks and helpers — but "reduced" is not
   "none", and the failure is silent zombie accumulation.
3. **Signal coalescing** — a wash, and D2 handles it correctly. Standard signals coalesce;
   `signalfd` does not fix that; the `waitpid(-1, WNOHANG)` **loop** is required either
   way. No advantage to either route here.

**Non-child processes: not a loss.** `pidfd_open(2)` works on any thread-group leader, not
only our children, and `POLLIN` on a pidfd reports exit for non-children too. What you
cannot do for a non-child is `waitpid` it — but that is a `wait` limitation, not a pidfd
one, and it applies equally under signalfd.

**Recommended revision:** option (c) is a good mechanism for *"has the container's init
exited"* — race-free, pollable, and D1 §8.2 already makes the pidfd the container handle.
It is **not** a replacement for signalfd, because signalfd's job in §7.2's loop is two
jobs, and the pidfd does one of them. **The supervisor needs both**: the pidfd for exit
detection *and* one of {`signalfd`, a minimal `sigaction` handler that writes to a
self-pipe} for `SIGTERM`/`SIGINT`/`SIGCHLD`. Since option (b) is ~25 lines and gives a
handler-free design that D2 §12.10 explicitly wants, **prefer (b) + pidfd, not (c)**.
This inverts §12.1.7's stated preference and it resolves D2 §14 open question 3.

---

## 6. D5 §18.3's capability count — CONFIRMED, with a naming correction

D5 claims D2 §12.3's LOW-confidence loose end is settled: *"all **41** capabilities
`CAP_CHOWN` (0) … `CAP_CHECKPOINT_RESTORE` (40) are present as associated constants,
explicitly including `CAP_BPF`, `CAP_PERFMON` and `CAP_CHECKPOINT_RESTORE`."*

**CONFIRMED by direct count** of the `bitflags!` block at
`rustix-1.1.4/src/thread/libcap.rs:22-115`. I enumerated all of them:

`CHOWN, DAC_OVERRIDE, DAC_READ_SEARCH, FOWNER, FSETID, KILL, SETGID, SETUID, SETPCAP,
LINUX_IMMUTABLE, NET_BIND_SERVICE, NET_BROADCAST, NET_ADMIN, NET_RAW, IPC_LOCK, IPC_OWNER,
SYS_MODULE, SYS_RAWIO, SYS_CHROOT, SYS_PTRACE, SYS_PACCT, SYS_ADMIN, SYS_BOOT, SYS_NICE,
SYS_RESOURCE, SYS_TIME, SYS_TTY_CONFIG, MKNOD, LEASE, AUDIT_WRITE, AUDIT_CONTROL, SETFCAP,
MAC_OVERRIDE, MAC_ADMIN, SYSLOG, WAKE_ALARM, BLOCK_SUSPEND, AUDIT_READ, PERFMON, BPF,
CHECKPOINT_RESTORE` — **41, exactly.**

Each is defined as `1 << linux_raw_sys::general::CAP_<NAME>`, so the bit positions come
from the kernel UAPI headers, not a hardcoded table (unlike `caps`, whose
`CAP_CHECKPOINT_RESTORE = 40` is hardcoded — `caps-0.5.6/src/lib.rs:145`). Against
`CAP_LAST_CAP` on kernel 6.11 the set is complete: `CAP_CHECKPOINT_RESTORE = 40` is the
last capability the kernel defines.

**CORRECTED, one thing, and it will cost someone a compile error:** the constants are
**not** named `CAP_BPF`, `CAP_PERFMON`, `CAP_CHECKPOINT_RESTORE`. The `CAP_` prefix is
stripped — they are `CapabilitySet::BPF`, `CapabilitySet::PERFMON`,
`CapabilitySet::CHECKPOINT_RESTORE`. D5 §18.3 reports the docs.rs *doc comments* (which do
say `/// CAP_BPF`) as if they were the identifiers. Minor, but D5 states it as an
enumeration of "associated constants", and the identifiers are what code types.

**Also worth recording, because it makes the whole question moot:** the bitflags block ends
with `const _ = !0;` — bitflags' externally-defined-flags escape — and `capget` builds its
result with `CapabilitySet::from_bits_retain`. So an unknown future capability bit round-
trips correctly and any missing constant is `from_bits_retain(1 << n)`, never a blocker.
D2 §12.3's LOW-confidence hedge was well-placed and the answer is better than "complete".

**Related, unflagged by either document:** rustix's `capabilities(pid)`/`set_capabilities`
(`thread/libcap.rs:123`, `:136`) are `capget`/`capset` with
`_LINUX_CAPABILITY_VERSION_3` and return/take a `CapabilitySets { effective, permitted,
inheritable }` — three `Copy` bitflags, **no allocation**, which is the property that
matters post-clone and is the actual reason to prefer them over `caps`'s
`HashSet<Capability>` (`caps-0.5.6/src/lib.rs:266`) and `CapsError(String)`
(`caps-0.5.6/src/errors.rs:5`). Both `caps` claims **CONFIRMED**; `caps` also has **zero**
occurrences of `NO_NEW_PRIVS`, confirming D1 §11.1.

---

## 7. Why youki left the `containers` org — SETTLED from primary sources

D2 §12.12 rates this `[R]` LOW, says *"why it moved, I could not establish… I am not going
to guess between them"*, and names the settling sources as *"the youki GitHub Discussions
or the CNCF sandbox application PR for youki (`cncf/sandbox` repo)"*.

**That instinct was exactly right, the named source was exactly right, and the answer is
the first of the two readings D2 offered.** Found via the GitHub search API:

**Primary source 1 — youki issue #2962, "Move to an independent organization"**, opened by
**`utam0k`** (youki's lead maintainer) on **2024-10-25**, closed 2024-10-30:

> *"Since youki has become the CNCF Sandbox, youki needs to be moved to a separate org. I
> have many fond memories of my work with containers org here. Unfortunately, youki repo
> needs to become independent. Thanks, `containers`."*

**Primary source 2 — `cncf/sandbox` issue #297, "[PROJECT ONBOARDING] Youki"** (created
2024-10-17, closed 2025-07-25). Comment by **`idvoretskyi`** (Ihor Dvoretskyi, CNCF) on
**2024-10-22**, the exact comment youki's issue links to:

> *"@utam0k yep, you'll have to move to a separate GitHub organization, and we'll invite it
> to join the CNCF Enterprise."*

**Verdict: the move was a CNCF onboarding requirement for Sandbox acceptance.** Nothing
adversarial, no fork, no maintainer dispute, no loss of governance — the opposite: it is
what *gaining* foundation governance required. D2 §12.12's `[R]` MED "the practical impact
on us is small" is **CONFIRMED** and can now be raised to HIGH.

One correction of date framing: D2 presents this as a current-state finding. The move
happened in **October 2024**, roughly two years before the corpus. `oci-spec-rs` followed
for the same reason. The GitHub redirect and the crate metadata are long settled.

**The refusal to speculate was correct.** It cost one search to settle, and had the agent
guessed, the coin-flip between its two readings was 50/50 with a reputational claim on the
losing side.


---

## 8. Cross-file crate conflicts — four found, one of them structural and unresolved

Four agents chose crates semi-independently. These are the pairs that cannot both be acted on
as written.

### 8.1 **UNRESOLVED CONFLICT — D4 requires a dedicated OS thread; D2 §8.3 forbids one. Neither noticed.**

This is the serious one. D4 states the requirement **three times** and explicitly hands it to
D2 **twice**:

- D4 §2.3 (line 186): *"`setns` for a netns must be called from a single-threaded process, or
  more precisely: it affects only the calling thread, so in a multithreaded Rust program you
  must `setns` on a dedicated thread and restore afterwards."*
- D4 §2.6(4) (line 557): *"a netlink socket is permanently bound to the netns it was created
  in. So you need one socket per namespace, opened after `setns`, and since `setns` is
  per-thread that means **a dedicated OS thread. This is an architectural consequence, not a
  detail → Handoff to D2.**"*
- D4 §2.x (line 201): *"Tradeoff: needs a dedicated OS thread for the setns dance… **Handoff
  to D2**: this forces 'we must be able to run a closure on a pinned thread in another netns'
  into the runtime architecture."*
- D4 §12.2 repeats it for `netlink-sys`: *"it needs a dedicated OS thread since `setns` is
  per-thread."*

D2 §8.3: *"**The container creation path must run in a process that has never had a second
thread.** Not 'a single-threaded runtime'. Not 'we're careful'. **Never had a second
thread**, because a thread that has exited can still have left a lock held at the moment it
was cancelled."*

And D2 §13's handoff to D4 places the netns work **in the supervisor**: *"Netns plumbing is
done by the **parent** (our supervisor), after `clone()` returns."* D2's handoff then answers
only the *async* half of D4's question ("if your rtnetlink design wants tokio, it cannot live
in the supervisor") and **never addresses the thread requirement at all**, even though D4
flagged it as an architectural handoff before D2's §13 was written.

**These two `[R]` HIGH statements are incompatible in one process.** As written, the design
says: the supervisor must never have a second thread, and the supervisor must spawn a thread.

**The resolution — and it is a good one, because it falls out of combining the two premises
rather than compromising either.** D4's requirement is conditional and D4 says so in its own
words: *"in a **multithreaded** Rust program you must `setns` on a dedicated thread."* D2
**guarantees the supervisor is single-threaded.** In a single-threaded process, the only
thread *is* the process, so `setns(netns_fd, CLONE_NEWNET)` on it moves the whole process,
and there is nothing to isolate the change from. The pattern is:

```
host_ns = open("/proc/self/ns/net", O_RDONLY|O_CLOEXEC)   // before anything
setns(container_netns_fd, CLONE_NEWNET)
socket(AF_NETLINK, …)          // now bound to the container netns
… netlink work …
setns(host_ns, CLONE_NEWNET)   // restore
```

No thread. The socket stays bound to the netns it was created in — which is the property
D4 §2.6(4) wanted the thread for — and it outlives the `setns` back. Caveats worth writing
down: the restore must be on every exit path including the error path (a `Drop` guard, and
`panic = "abort"` in the supervisor so a panic cannot skip it), and everything between the
two `setns` calls runs in the container's netns, so no host-netns I/O may happen there.

**Each document holds one half of this and neither combines them.** D4 reasons from
"multithreaded Rust program" as if that were the given; D2 forbids exactly that. **Action:
D2 §13's handoff to D4 needs a fourth bullet saying the supervisor is single-threaded and
therefore `setns` needs no thread, and D4 §2.6(4)/§12.2 need the conditional made explicit.**
Until that is written down, this is the highest-risk unstated assumption across the corpus,
because the naive reading — "D4 says spawn a thread, so spawn one" — silently breaks §8.3's
central invariant in the process that calls `clone()`.

### 8.2 **CONFLICT — D3's HTTP client can silently make its process multithreaded, and D3 offers that process as a candidate for the supervisor**

Answering the brief's question directly: **does D3's HTTP-client choice force async into a
runtime D2 says must stay sync? No — but it does something subtler and worse.**

D3 §13.1 picks `ureq` (blocking, no runtime) — which is *more* conservative than D2 §8.3
requires, since D2 puts the whole pull path in a separate process. So far, no conflict. But
D3 gives as its **third stated reason**:

> *"it keeps the option open of one day linking the puller into the supervisor."*

**REFUTED.** Verified in `ureq-3.4.0/src/unversioned/resolver.rs`:

```rust
let use_sync = timeout.after.is_not_happening();
let iter = if use_sync {
    // "When timeout is not set, we do not spawn any threads."
    addr.to_socket_addrs()?
} else {
    resolve_async(addr, timeout)?
};
…
fn resolve_async(addr: String, timeout: NextTimeout) -> … {
    let (tx, rx) = mpsc::sync_channel(1);
    thread::spawn(move || tx.send(addr.to_socket_addrs()).ok());   // :147
    match rx.recv_timeout(*timeout.after) { … }
}
```

`ureq` **spawns an OS thread per DNS resolution** whenever a `resolve` or `global` timeout is
configured. `Timeouts::default()` (`src/config.rs:894`) has `global: None, resolve: None`, so
the *default* config spawns nothing — but any production registry client sets a timeout, and
D3's own §2 pull design would want one.

Three properties make this worse than a tokio runtime, not better:

1. **It is lazy and conditional.** The thread appears only when a timeout is set *and* a name
   needs resolving. A `Threads == 1` assert before `clone()` — D1 §3.4 and D2 §12.7's
   enforcement mechanism for this entire class — passes on most runs.
2. **The thread is detached and never joined.** `thread::spawn(…)` with the handle dropped.
   On timeout the caller returns while the thread keeps running.
3. **That thread is inside glibc `getaddrinfo`/NSS** — which allocates, takes locks, and
   `dlopen`s NSS modules. Forking while a thread sits in NSS is the textbook post-`fork`
   deadlock. This is precisely D2 §8.3's *"nondeterministic deadlocks… that reproduce once in
   a thousand runs"* — and it would reproduce only when DNS is slow.

**D3's recommendation of `ureq` stands** — it is the right client for a separate pull
process, and D2 §8.3's structural separation makes the thread harmless there. **D3's third
reason must be struck**, and it is worth saying why it matters beyond a footnote: it is
exactly the discipline-invariant reasoning D2 §8.3 rejected by name (*"Structural beats
disciplined"*), reappearing in another document as a selling point for a crate. That is how
these invariants actually get broken.

*(For completeness on `reqwest`: D3 §13.1's claim that its `blocking` feature *"still starts
a tokio runtime thread"* is correct and well known; D3 rates it MED and accepts it only
because pulls are a separate process. Consistent with D2. No conflict.)*

### 8.3 CONFIRMED CONSISTENT — D5's cgroup verdict and D2 §12.8

The brief asks whether D5's *"no cgroup crate, cgroupfs is just `std::fs::write`"* agrees
with D2 §12.8 on the youki crates. **It does, exactly**, and the reasoning is independently
arrived at: D2 §12.8 (*"cgroup v2 for our purposes is `mkdir`, `std::fs::write` to
`cgroup.subtree_control`/`cgroup.procs`/`memory.max`, `cgroup.kill`… That is a
file-writing module we should own"*) and D5 §18.1 (*"the reasoning is arithmetic… the two
crates' main value is the parts we do not want"*) reach the same verdict on the same
grounds — v1 support and the systemd D-Bus driver are the crates' value and we want neither.
`cgroups-rs 0.5.1` health re-derived: 21 commits/180d under `kata-containers`, no advisory —
so D5's *"both are healthy. Neither should be adopted"* is a **correctly separated**
health-vs-fit judgement, which is the discipline this cluster most often fails. No conflict.

D5 §18.1's `std::fs::write`-is-one-`write(2)` claim is also correct and is a real subtlety
worth the space it got: `std::fs::write` on a small buffer issues a single `write`, while
`BufWriter` may split — and cgroup interface files are not stream files.

### 8.4 MINOR CONFLICTS — three stale or contradictory rows

| Pair | Conflict | Resolution |
|---|---|---|
| D2 §12.13 rejects **`fs2`/`fd-lock`/`file-lock`** outright; D4 §12.5's table lists **`fs2` *(or `rustix::fs::flock`)*** as **"adopt or stdlib"** | The table rows literally disagree. Both prefer rustix/std in prose, so it is cosmetic — but a reader building `Cargo.toml` from D4's table adds `fs2`. | **D2 wins.** `rustix::fs::flock` exists (verified, `src/fs/`), and `std::fs::File::lock` is `flock(2)` on Linux. Delete the `fs2` row. |
| D3 §13.2 attributes **`oci-spec` to "containers-org"** | **OUTDATED.** The repo moved to `youki-dev/oci-spec-rs` in **October 2024** (§7 above), and crates.io metadata says so. D2 §12.2/§12.12 has it right. | Correct D3's row to `youki-dev`. No consequence beyond accuracy. |
| D4 §12.5 lists **`nix` / `rustix` — "adopt (one of)"**, deferring to *"D1 §11.1's call"* | D1 §11.1 recommended **`nix` + `rustix` together**; D2 §12 **explicitly supersedes** D1 §11.1 with rustix-primary and "`nix`: defer (probably never)". D4 cites the superseded document. | Point D4 at D2 §12.1.7. Consequence is real: D4's netlink code will use whichever crate it thinks is "the call", and `netlink-sys` brings its own `libc` dependency regardless. |

*(D2 §12.12 also flagged that `tar`'s repository metadata moved to `composefs/tar-rs` and
asked D3 to verify rather than assume. **Verified: it has** — crates.io `repository` for
`tar 0.4.46` reads `https://github.com/composefs/tar-rs`. D3 never picked this up; D3 §13.3
still says only "now maintained at `github.com/composefs/tar-rs`", which is right, so this
one is a non-issue — D3 had it first.)*

---

## 9. The sync/async boundary (D2 §8) — hazard CONFIRMED, one transitive leak found

### 9.1 The underlying hazard — CONFIRMED against the primary source

D2 §8.1's `fork(2)` quotations are accurate: the child gets one thread; only async-signal-safe
functions may be called until `execve`; the entire address space including mutex states is
replicated. `signal-safety(7)`'s allowlist genuinely excludes everything that allocates. The
claim that `clone(2)` with namespace flags is the same kernel path as `fork` is correct —
glibc's `fork()` is `clone(SIGCHLD)`.

D2 §8.2's corroboration from youki's `Cargo.toml` — that `crates/libcontainer` depends on no
async and no thread-spawning crate while `tokio` appears only in the binary crate behind a
WASM feature — is the strongest external evidence available and it is correctly read. The
architectural conclusion (*"the process that calls `clone()` never has more than one thread,
ever"*) is **CONFIRMED as the right rule**.

### 9.2 Does any recommended crate silently pull a runtime into the wrong process?

I checked every crate the corpus recommends for the supervisor or the child.

| Crate | Thread/runtime? | Verdict |
|---|---|---|
| `rustix` (default + the nine features) | No. Deps are `bitflags` + `linux-raw-sys` only on the raw backend. | Clean |
| `libc` | No | Clean |
| `serde`, `serde_json`, `thiserror` | No; `thiserror` is a proc-macro with no runtime footprint | Clean |
| `clap` (`default-features = false` + the six youki features) | No — and D2 §12.4's rule that clap lives in the CLI subcommand only makes it moot | Clean |
| `tracing` / `tracing-subscriber` | No thread spawn on the emit path | Clean, but see `tracing-appender` |
| **`tracing-appender`** | **Yes — verified in source.** `tracing-appender-0.2.5/src/worker.rs:71-73` is `thread::Builder::new()…​.spawn(move ‖ …)`, and `non_blocking.rs:385`/`:468` spawn more. | D2 §12.7 bans it outright. **CONFIRMED against source**, and it is the right call: a structural violation, not a style one. (Latest stable is **0.2.5**, not 0.2.6.) |
| `seccompiler` | No. Deps: `libc` + optional `serde`/`serde_json`. `apply_filter` is stack-only. | Clean — but see §4.3 on `BpfProgram = Vec` |
| `netlink-packet-route` / `-core` | No — pure codecs, `zerocopy` derives, no socket | Clean |
| `netlink-sys` `default-features = false` | No — deps `bytes, libc, log`; the tokio/smol/mio sockets are separate types behind separate features | Clean; D4's `default-features = false` insistence is load-bearing |
| `nftables` (nftables-rs) | No by default — verified: `src/helper.rs:97` is `Command::new(program)` on the default path; the async variants at `:128`, `:136`, `:153` are `#[cfg(any(feature = "tokio", feature = "async-process"))]`, and `:21-22` makes the two features a `compile_error!` if both are set | Clean **if** the default is kept; note that `nft` becomes a runtime binary dependency |
| **`ureq`** | **Yes, conditionally — see §8.2** | **The one leak.** Harmless in a separate pull process, fatal if linked into the supervisor |
| `reqwest` (`blocking`) | Yes, by design | Correctly priced by D3; separate process only |

**Net: one transitive leak, and the corpus's own structural rule already contains it.**
That is a good result for §8.3 — the separate-process design absorbs the one crate that
would otherwise have broken the invariant, without anyone having noticed the crate did it.
It is also the clearest possible argument for why §8.3 chose *structural* over *disciplined*.

### 9.3 One caveat on the enforcement mechanism

D1 §3.4 / D2 §12.7 make `Threads == 1` from `/proc/self/status` a **hard assert in release
builds** immediately before the clone. That is the right mechanism and it should be kept.
But §8.2 notes that *"the thread that has exited can still have left a lock held"* — and a
`Threads == 1` read cannot see an exited thread. So the assert catches persistent runtimes
(tokio, rayon, `tracing-appender`) and **misses transient ones** — including, precisely,
`ureq`'s detached DNS thread after it has finished. **The assert is necessary and not
sufficient; say so where it is prescribed.** The only mechanism that covers the transient
case is the structural one: the process that clones never *links* the offending crate.

---

## 10. "Depend on what hides typing, write what hides mechanism" — applied adversarially

The test itself is good and I have no quarrel with it. Applied against the grain:

**Is any "adopt" verdict hiding mechanism the project exists to learn?**

- `oci-spec` — no. Verified: `src/runtime/version.rs` is a `const_format!` of three integers,
  and the crate's own test asserts `version() == "1.1.0"`. **D2 §12.2's claim that
  `oci_spec::runtime::VERSION` says `1.1.0` while the structs carry v1.3.0 Linux fields is
  CONFIRMED**, and its `[R]` (write our own `ociVersion` string) is correct. This is a schema,
  not a mechanism. Adopt stands.
- `netlink-packet-route` — **the closest call in the corpus, and D4 argues it honestly.** The
  TLV encoding *is* mechanism, and D4 §12.1 says so, then splits: write the socket/ack/seq/
  dump loop (the mechanism), adopt the codec (the typing). D4 also offers the "hand-roll the
  codec with the crate as a `dev-dependency` round-trip oracle" middle path at LOW→MED and
  explicitly declines to pretend the call is obvious. That is the right handling of a genuine
  judgement call.
- `nftables` (nftables-rs) — **this one does hide mechanism, and D4 does not fully own it.**
  It shells out to `nft -j -f -`. The mechanism (what an nftables transaction *is*, at the
  netlink level) is hidden behind a subprocess and a JSON schema versioned by whatever `nft`
  binary is installed. D4 prices the costs honestly (runtime binary dependency, one
  fork+exec per op, JSON round-trip, schema drift) and justifies it by what moby and netavark
  actually ship — a strong argument. But by D2 §12.8's test, "what an nftables transaction
  is" is mechanism, and we are choosing not to learn it. **That is a defensible choice; it
  should be stated as one.** Note also that `nftables` at 0.6.3 was last published
  **2025-08-15**, over a year before the corpus — the only "adopt" verdict in the whole
  corpus on a crate with no release in a year.
- `clap`, `serde`, `thiserror`, `sha2`, `flate2`, `tar`(parser), `ipnet` — all typing. Fine.

**Is any "write ourselves" verdict re-implementing something with a well-tested,
well-licensed, maintained crate behind it?**

- **`pentacle` → "read, then write"** is the weakest of them. D2 §12.5's stated reason is
  maintenance ("one maintainer, untouched since 2024-10-03") — but the crate has **no
  advisory**, does one narrow security-critical job (`memfd_create` + copy + `F_SEAL_*` +
  re-exec against CVE-2019-5736), is used by youki in production, and D2 itself concedes it
  *"appears to do it correctly"*. This is the one place the corpus prices "untouched" as
  "risky" where "finished" is the better reading — a two-year-old crate that wraps four
  stable syscalls does not need commits. **The counter-argument is real and D2 makes it:** it
  is a security mechanic we want visible, and ~60 lines. So the verdict survives on
  *pedagogy*, not on health. **Recommend restating the reason**; the maintenance framing is
  the kind of laundering the brief warns about, and here it is aimed at a crate that does not
  deserve it.
- `daemonize` → write: **strongly justified**, and now more so — one live RustSec
  unmaintained advisory, RUSTSEC-2025-0069 (§2.5). D2's design reason (we deliberately keep
  0/1/2 and the handshake pipe) is the better one and is correct.
- `caps` → write: justified on allocation, verified (§6).
- content-addressed store, the applier, the supervisor, cgroup v2, the socket loop → all
  genuinely mechanism. Fine.

**The "plausibly six entries" claim (D2 §12.13).** `rustix`, `libc`, `clap`, `serde`,
`serde_json`, `thiserror` (+`anyhow`, `tracing`). **Plausible and CONFIRMED as arithmetic** —
these eight have no transitive runtime, no C dependency, no advisory, and (verified §1.4)
rustix's nine feature modules are gated exactly by the names D2 lists, so the recommended
feature set does cover every function §12.1.2 relies on **at module granularity**. Two
honest asterisks: (a) phase 1 as scoped has no networking and no image pull, so "six entries"
is a statement about phase 1's *scope*, not about the engine; the moment D4 and D3 land it is
six plus `netlink-packet-route`, `netlink-packet-core`, `netlink-sys`, `ipnet`, `nftables`,
`ureq`, `tar`, `flate2`, `sha2`, `oci-spec` — sixteen, all justified. (b) `seccompiler` is
absent from the six, but D5 §18.2's actual recommendation is to hand-write the `sock_filter`
array, so the count holds if that is done.

---

## 11. Empirical — the host check, and a claim it refutes

`ssh host-a` (read-only; `file`, `ldd`, `--version`, `uname` only):

```
/usr/bin/runc: ELF 64-bit LSB pie executable, dynamically linked,
               interpreter /lib64/ld-linux-x86-64.so.2
    libseccomp.so.2 => /lib/x86_64-linux-gnu/libseccomp.so.2
    libc.so.6      => /lib/x86_64-linux-gnu/libc.so.6
/usr/bin/containerd-shim-runc-v2: statically linked  ("not a dynamic executable")
runc version 1.2.5 | spec: 1.2.0 | go1.23.7 | libseccomp: 2.5.5
kernel 6.11.0-29-generic
```

**CONFIRMED:** runc is dynamically linked against `libseccomp.so.2`; the containerd shim is
static; `runc --version` reports `spec: 1.2.0` (D2 §12.2's calibration for "even production
runtimes lag the spec by a minor version"). The static-musl recommendation this feeds
(D2 §7.3, §12.11; D5 §18.2) is **soundly based**.

**REFUTED — the generalisation built on top of it.** D5 §18.2 states flatly:

> *"runc is the **only dynamically-linked binary in the docker→containerd→shim→runc
> chain**, and `ldd /usr/bin/runc` shows `libseccomp.so.2`."*

On this very host that is false:

```
/usr/bin/containerd  : dynamically linked → libc.so.6
/usr/bin/dockerd     : dynamically linked → libsystemd.so.0, libc.so.6, libcap.so.2
/usr/bin/docker-proxy: dynamically linked → libc.so.6
/usr/bin/ctr         : dynamically linked → libc.so.6
/usr/bin/containerd-shim-runc-v2: statically linked
```

**The shim is the only *statically* linked binary in the chain — the exact inverse.**
`dockerd` in particular links three shared libraries, one more than runc.

D2 §12.11's own phrasing is careful and survives — it says *"the reason runc is the one
dynamically-linked binary in the chain is, **at least in part**, libseccomp"* and cites only
the runc/shim comparison it actually ran. **D5 escalated a two-binary observation into a
five-binary universal and attributed it to D2.** The load-bearing inference is untouched
(taking `libseccomp` means C FFI, which breaks static-musl), because that rests on runc's
own `ldd`, not on the generalisation. But the sentence as written is wrong, and it is the
kind of wrong that gets quoted.


---

## 12. Smaller corrections

**12.1 CORRECTED — "our state paths sit near rustix's threshold" is overstated by ~2×.**

D2 §12.1.3 justifies the `&CStr` rule partly with: *"which is not a hypothetical: our state
directory is `/var/lib/mydocker/containers/<64-hex-id>/…`"*. Arithmetic on the layout D2 §6.4
actually fixes:

| Path | Length |
|---|---|
| `/var/lib/mydocker/containers/` | 29 |
| `+ <64-hex-id>` | 93 |
| `+ /rootfs` | 100 |
| `+ /sys/fs/cgroup` (deepest routine in-container mount target) | **114** |
| **Headroom to rustix's 256** | **142 bytes** |

Our own state paths are **less than half** the threshold. **The rule is still right, but for
a different reason:** the path that can exceed 256 is the **user-supplied bundle path**
(`--bundle /home/…/deeply/nested/…`), which we do not control and cannot bound. That is the
real hazard and it is a better argument than the one given, because it cannot be designed
away by shortening our own directory names. Restate it that way.

(This also partially settles D2 §14 Q9: youki uses `nix`, threshold **1024**, and no
realistic bundle path reaches 1024 — so youki is almost certainly fine in practice, and the
256-byte exposure is *created by choosing rustix*. Worth naming as a real, if small, cost on
the rustix side of the ledger, which §12.1.7's "what we give up" list does not mention.)

**12.2 CORRECTED — `rustix::thread::CapabilitySet` constants drop the `CAP_` prefix.** See §6.
`CapabilitySet::BPF`, not `CapabilitySet::CAP_BPF`.

**12.3 CORRECTED — rustix's `*at` functions are named `chownat`/`chmodat`, not
`fchownat`/`fchmodat`.** D3 §13.8 lists the POSIX names. `linkat`, `symlinkat`, `mknodat`,
`utimensat`, `renameat`, `openat`, `openat2` are as D3 names them (`src/fs/at.rs`); `fsync`
is `src/fs/fd.rs`. Trivial, but D3's list is meant to be copy-able.

**12.4 SETTLED — D3 §13.8's open `[R] MED`: "verify `rustix` exposes the new mount API in the
version we pin."** It does, at 1.1.4. Enumerated from `src/mount/`: `fsopen`, `fsmount`,
`fsconfig_create`, `fsconfig_create_exclusive`, `fsconfig_reconfigure`, `fsconfig_set_binary`,
`fsconfig_set_fd`, `fsconfig_set_flag`, `fsconfig_set_path`, `fsconfig_set_path_empty`,
`fsconfig_set_string`, `move_mount`, `open_tree`, plus `mount`, `mount_bind`,
`mount_bind_recursive`, `mount_change`, `mount_move`, `mount_remount`, `unmount`. D2 §12.1.2's
`mount_bind_recursive` name is correct (`src/mount/mount_unmount.rs:109`). No `mount_setattr`,
as stated.

**12.5 Line-number drift (no substantive effect).** D2 §12.3 cites the `CapabilitySet`
deprecation at *"`src/thread/prctl.rs:~358`"*; it is at `:184` (the tilde was doing honest
work). D2 §12.1.2 cites `set_secure_computing_mode` at *"`src/thread/prctl.rs:163-176`"*;
`secure_computing_mode` is at `:159` and `set_secure_computing_mode` at `:173`. Both claims
substantively correct.

**12.6 Note, not a correction — `libc` now has a `1.0.0-alpha.4` line** (published
2026-07-21, the same day as `0.2.189`). `max_stable_version` is `0.2.189`, so the corpus is
right, but a `1.0` migration is in flight and a `Cargo.toml` should pin the `0.2` line
deliberately.

---

## 13. Verdict summary

| # | Claim | Verdict |
|---|---|---|
| 1 | rustix syscall-coverage table vs `nix` (13 rows) | **CONFIRMED** — every row, against source |
| 2 | `clone3`/`fork`/`execve` in `not_implemented::libc_internals` / `quite_yet` | **CONFIRMED** verbatim |
| 3 | "no crate will abstract `clone3` for us" | **MISCLASSIFIED** — `nc 0.9.8` (which youki uses), the `clone3` crate and `syscalls` all expose it. Conclusion survives |
| 4 | `linux_raw` is the default backend on x86_64-linux, no libc | **CONFIRMED** from `Cargo.toml` predicates |
| 5 | §12.1.7 justification #1 ("only option that structurally reduces the post-clone hazard") | **UNSUPPORTED as stated** — glibc's thin syscall wrappers take no lock; the real win is `__errno_location()` TLS under `CLONE_VM`, which the corpus never argues. Decision unaffected |
| 6 | "only crate with" (pidfds, caps, non-broadcasting setresuid, new mount API) | **CONFIRMED vs `nix`**; **UNSUPPORTED as a universal** (`caps` has the cap sets, `nc` has the mount API, a `pidfd` crate exists) |
| 7 | `nix` 1024 / `rustix` 256 allocation thresholds + `CString::new` fallback | **CONFIRMED** at the cited lines |
| 8 | `&CStr`-in-signatures fixes it | **CORRECTED** — necessary, not sufficient. Misses the seccomp `Vec<sock_filter>`, argv/envp, `std::fs` in the child |
| 9 | "our state paths sit near rustix's 256 threshold" | **CORRECTED** — 114 bytes, 142 of headroom. The real risk is the user-supplied bundle path |
| 10 | rustix has no `signalfd` | **CONFIRMED** — `not_implemented::yet` only |
| 11 | signalfd option (b) is "a 3-argument syscall" | **CORRECTED** — also needs `libc::sigprocmask`; rustix refuses the signal-mask surface by policy |
| 12 | signalfd option (c) (pidfd) preferred | **REFUTED** — loses the supervisor's own `SIGTERM` entirely (breaks §5.5) and produces no wake for subreaped orphans. **Prefer (b) + pidfd** |
| 13 | **RUSTSEC-2026-0067** exists and says what D3 says | **CONFIRMED** verbatim, `[affected.functions]` and all |
| 14 | *"the parsing is boring… the applying is where every CVE lives"* | **REFUTED** — **RUSTSEC-2026-0068** (CVE-2026-33055), same crate, same day, **parser**, no affected-functions scope |
| 15 | "`async-tar` (CVE-2025-62518)" | **CORRECTED** — 62518 is `astral-tokio-tar`/`tokio-tar`; `async-tar` has no advisory. CVE-2025-59825 → `astral-tokio-tar` is right |
| 16 | `rustables` is GPL-3.0-or-later | **CONFIRMED** on crates.io. Best catch in the corpus |
| 17 | Advisory sweep over all 57 recommended crates | 3 new findings: `tar` **RUSTSEC-2026-0068**, `nftnl` **RUSTSEC-2025-0126** (heap overflow), `daemonize` **RUSTSEC-2025-0069** (unmaintained, live). Pin floors: `anyhow ≥ 1.0.103`, `tracing-subscriber ≥ 0.3.20`. `ring`: **no live advisory** — D3's MED rating was right. **Primary substrate is clean** |
| 18 | Maintenance figures (10 download counts, 4 commit counts, author breakdowns, open-issue counts) | **CONFIRMED**, several exactly, including `nix`'s 354 and `oci-spec`'s 40/6-authors |
| 19 | `seccompiler` health ("rust-vmm/Firecracker provenance") | **OUTDATED** — `rust-vmm/seccompiler` **archived 2026-08-18** into the rust-vmm monorepo. Not abandonment; but the cited repo is read-only and 0.5.0 is 18 months old. `libseccomp-rs`: **0 commits/180d**. Verdict (hand-write) unaffected and strengthened |
| 20 | D4's bus-factor argument against `neli` | **CORRECTED** — `netlink-packet-route` is 78/88 commits by one person (`cathay4t`); `netlink-sys` 3/4. Bus factor is one on both sides. Keep the *coverage* argument |
| 21 | D5 §18.3: `CapabilitySet` has all 41 constants incl. `CAP_BPF`/`PERFMON`/`CHECKPOINT_RESTORE` | **CONFIRMED — 41 exactly**, counted at `thread/libcap.rs:22-115`, bits from `linux_raw_sys` UAPI, complete vs `CAP_LAST_CAP`=40 on 6.11. Names drop the `CAP_` prefix |
| 22 | `caps` allocates (`HashSet`, `CapsError(String)`), no `NO_NEW_PRIVS`, 0 commits/180d | **CONFIRMED**, all four |
| 23 | Why youki/`oci-spec-rs` left the `containers` org | **SETTLED** — CNCF Sandbox onboarding required a separate org. youki #2962 (utam0k) + cncf/sandbox #297 (idvoretskyi, 2024-10-22). Benign. **The refusal to speculate was correct** |
| 24 | D4 needs a dedicated OS thread for `setns` / D2 forbids threads in the cloner | **UNRESOLVED CONFLICT** — and it dissolves: single-threaded process ⇒ `setns` needs no thread. Neither document combines its own premises |
| 25 | D3: `ureq` "keeps the option open of linking the puller into the supervisor" | **REFUTED** — `resolver.rs:147` spawns a detached thread per DNS lookup whenever a timeout is set |
| 26 | D5 cgroups vs D2 §12.8 | **CONSISTENT**, independently reached |
| 27 | D2 rejects `fs2` / D4's table adopts it; D3 says `oci-spec` is "containers-org"; D4 cites superseded D1 §11.1 | **Three minor conflicts**, all resolvable in D2's favour |
| 28 | `oci_spec::runtime::VERSION == "1.1.0"` while structs carry v1.3.0 fields | **CONFIRMED** — the crate's own test asserts it |
| 29 | runc dynamically linked to `libseccomp.so.2`; shim static; `spec: 1.2.0` | **CONFIRMED** on `host-a` |
| 30 | *"runc is the only dynamically-linked binary in the chain"* (D5 §18.2) | **REFUTED** — `containerd`, `dockerd`, `docker-proxy`, `ctr` are all dynamic; the **shim is the only static one**. The inference it supports survives |
| 31 | `tracing-appender`'s `non_blocking` spawns a thread → ban | **CONFIRMED** and correctly rated structural |
| 32 | Transitive-runtime sweep over supervisor/child crates | **One leak: `ureq`.** Everything else clean. §8.3's separate-process rule already contains it |
| 33 | "Phase 1's `Cargo.toml` is plausibly six entries" | **CONFIRMED as arithmetic** for phase 1's scope; ~sixteen once D3 and D4 land |
| 34 | `pentacle` → "read, then write" on maintenance grounds | **MISCLASSIFIED** — no advisory, narrow job, used in production by youki; "untouched" here means finished. Verdict survives on *pedagogy*, not health. Restate the reason |
| 35 | D1 §11.1: "do not build on `rustix::runtime`" | **CONFIRMED** — module doc says functions there *"invoke Undefined Behavior if called in a process which has a libc present"* |

---

## 14. What remains unsettled

1. **D2 §14 Q1 and Q2 stay open and I could not close them either** — no Rust toolchain here
   (macOS; `host-a` is production and must not get one). Q1 (does a *compiled* binary
   actually keep libc off rustix's syscall path) and Q2 (are the functions reachable under
   our feature set) both need `cargo`. I closed Q2 **at module granularity** (§10: the nine
   modules are gated exactly by the nine feature names D2 lists); per-function `cfg` gating
   inside those modules still needs a compiler. Both experiments are ten minutes on the dev
   VM and both are worth doing before any `Cargo.toml` is written.
2. **The `nftables`-hides-mechanism question is a judgement call, not a fact**, and it should
   be recorded as one rather than settled by citing moby and netavark.
3. **Whether `tracing-subscriber` specifically deadlocks post-fork** (D2 §14 Q5) — I did not
   read `fmt`'s lock usage either. As D2 says, the design answer does not depend on it.
4. **The child-side allocator-abort shim** — I recommend it in §4.3 as the enforcement
   mechanism for the whole child, not only for error paths. **D2 §14 Q13 already proposes
   exactly this shim** as a settling experiment; the extension is to make it a permanent
   guard rather than a one-off test, and to cover the seccomp `Vec` case Q13 does not name.

---

## 15. Sources consulted

- **Crate sources**, extracted from `static.crates.io` tarballs and grepped directly:
  `rustix-1.1.4`, `nix-0.31.3`, `caps-0.5.6`, `seccompiler-0.5.0`, `ureq-3.4.0`,
  `oci-spec-0.10.0`, `nc-0.9.8`.
- **RustSec advisory database**, `github.com/rustsec/advisory-db` at HEAD `5a0ebed`
  (2026-09-02): `crates/tar/RUSTSEC-2026-0067.md`, `RUSTSEC-2026-0068.md`,
  `crates/astral-tokio-tar/*`, `crates/tokio-tar/RUSTSEC-2025-0111.md`,
  `crates/nftnl/RUSTSEC-2025-0126.md`, `crates/ring/RUSTSEC-2025-0007.md`,
  `crates/daemonize/*`, `crates/anyhow/RUSTSEC-2026-0190.md`,
  `crates/tracing-subscriber/RUSTSEC-2025-0055.md`, plus a null sweep over 45 more.
- **crates.io JSON API** — versions, licences, publish dates, 90-day downloads, repository
  metadata for 18 crates.
- **GitHub REST API** — `/repos` and `/commits?since=2026-03-09` for
  `bytecodealliance/rustix`, `nix-rust/nix`, `jbaublitz/neli`, `mullvad/nftnl-rs`,
  `kata-containers/cgroups-rs`, `lucab/caps-rs`, `youki-dev/oci-spec-rs`,
  `rust-netlink/netlink-packet-route`, `rust-netlink/netlink-sys`, `rust-vmm/seccompiler`,
  `libseccomp-rs/libseccomp-rs`.
- **Primary sources for the org move**: `youki-dev/youki` issue **#2962**; `cncf/sandbox`
  issue **#297** and its 2024-10-22 comment.
- **`cveawg.mitre.org`** for CVE-2025-59825.
- **`host-a`** (`HOST-A`, kernel 6.11.0-29) — read-only `file`, `ldd`,
  `runc --version`, `uname` only. No container operations, no `systemctl`, no nft/iptables.
- `old-host` was not attempted; `old-host.example.edu` is NXDOMAIN.

