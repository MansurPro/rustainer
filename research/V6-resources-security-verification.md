# V6 — Adversarial verification: D5 Resource control & security

Scope: `D5-resources-security.md` **except §18** (Rust crates — other verifier).
Crate-adjacent questions in items 9/10 of the brief are adjudicated here.

Method: primary text (`capabilities(7)`, `seccomp(2)`, `user_namespaces(7)`, `proc(5)`,
`pid_namespaces(7)`, `cgroup-v2.rst`, OCI Runtime Spec), project source
(moby v28.3.1, runc v1.2.5), and read-only observation on HOST-A
(`host-a`, `HOST-A`, Ubuntu 24.10, 6.11.0-29-generic, Docker 28.3.1,
runc 1.2.5, spec 1.2.0, libseccomp 2.5.5). No write touched the host.

`old-host` remains NXDOMAIN — every ROCKY-dependent claim is **UNSUPPORTED — pending host**
with the settling command given.

Verdict vocabulary as briefed: CONFIRMED / CORRECTED / REFUTED / OUTDATED /
UNSUPPORTED / MISCLASSIFIED.

Adversarial weighting applied: "this is sufficient" claims attacked harder than
"this is dangerous" claims.

---

## Running log

(appended incrementally as verification proceeded)

### §0.1–0.3 Baseline — CONFIRMED

Re-ran the environment probes verbatim:

```
Linux HOST-A 6.11.0-29-generic ... x86_64
Docker version 28.3.1, build 38b7060
runc version 1.2.5 / commit v1.2.5-0-g59923ef / spec 1.2.0 / libseccomp 2.5.5
cgroup.controllers      = cpuset cpu io memory hugetlb pids rdma misc
cgroup.subtree_control  = cpuset cpu io memory hugetlb pids rdma misc
/sys/kernel/cgroup/features = nsdelegate favordynmods memory_localevents
                              memory_recursiveprot memory_hugetlb_accounting pids_localevents
/sys/kernel/cgroup/delegate = cgroup.procs cgroup.threads cgroup.subtree_control
                              memory.oom.group memory.reclaim
```

All match the document. The runc version claim ("v1.2.5 deliberately: it is the version
actually running on HOST-A") is CONFIRMED — `runc --version` reports exactly v1.2.5, so
source-reading and observation are the same build. Five-file delegate list CONFIRMED
(§2.1's "five, not three" is right).

The self-declared read-only discipline is CONFIRMED as plausible: nothing in the document
requires a write to have produced the output shown, and the `[EXPERIMENT — dev VM]`
markers are where writes would have been needed.

---

## A. Seccomp (brief item 2) — the highest-priority block

### A.1 §8.5 "allowlist, not blocklist"; the numbers — **CONFIRMED, exactly**

Fetched `moby/moby` **v28.3.1** `profiles/seccomp/default.json` and parsed it:

| document claim | measured | verdict |
|---|---|---|
| `defaultAction: SCMP_ACT_ERRNO` | `SCMP_ACT_ERRNO` | CONFIRMED |
| `defaultErrnoRet: 1` (EPERM) | `1` | CONFIRMED |
| 13 KB | 13063 bytes | CONFIRMED |
| 31 syscall groups | `len(syscalls) == 31` | CONFIRMED |
| **426 distinct names** allowed across all groups | `426` | **CONFIRMED — count verified** |
| **361 allowed unconditionally** | 361 (the single group with no `args`/`includes`/`excludes`), and all 361 are distinct | CONFIRMED |
| `archMap` gives `SCMP_ARCH_X86_64` subArches `["SCMP_ARCH_X86","SCMP_ARCH_X32"]` | verbatim match | CONFIRMED |

The "denied unconditionally" list in §8.5 is CONFIRMED by absence — every one of
`keyctl add_key request_key kexec_load kexec_file_load pivot_root userfaultfd swapon
swapoff io_uring_setup io_uring_enter io_uring_register create_module get_kernel_syms
query_module uselib ustat nfsservctl _sysctl vm86 vm86old` is absent from the profile
entirely, so `defaultAction` catches them. The document's framing — that these are denied
*by the mechanism*, not by a named deny rule — is exactly right.

The `socket` AF_VSOCK rule is verbatim correct:
`{"names":["socket"],"action":"SCMP_ACT_ALLOW","args":[{"index":0,"value":40,"op":"SCMP_CMP_NE"}]}`.
The `clone` masked-equality rule is verbatim correct including `value: 2114060288`
(= `0x7E020000`) and `excludes.caps: ["CAP_SYS_ADMIN"]`; the document omits that the rule
also carries `excludes.arches: ["s390","s390x"]` with a mirrored s390 rule at `index: 1`
— harmless for an x86_64-first engine, worth knowing if we ever emit the profile ourselves.

### A.2 §8.5 "ptrace / process_vm_readv / process_vm_writev are denied by default" — **REFUTED**

The document states, in its "denied conditionally on capability" paragraph, that with the
default 14 caps `mount, umount2, unshare, setns, bpf, perf_event_open, **ptrace,
process_vm_readv/writev**, pidfd_getfd, init_module, syslog, open_by_handle_at,
clock_settime, iopl, ioperm, reboot, quotactl` "are all denied — because their gating
capabilities … are not in the default set."

That is false for three of them. The profile contains:

```json
{"names":["process_vm_readv","process_vm_writev","ptrace"],
 "action":"SCMP_ACT_ALLOW","includes":{"minKernel":"4.8"}}
```

`includes.minKernel` is resolved against the **host kernel**, not against capabilities.
On HOST-A's 6.11 — and on every kernel we would ever target — this group matches and
`ptrace`, `process_vm_readv` and `process_vm_writev` are **ALLOWED by Docker's default
seccomp profile**. (There is a *second*, `CAP_SYS_PTRACE`-gated group listing the same
names plus `kcmp`, `pidfd_getfd`, `process_madvise`; that one is what the document was
looking at. Under seccomp's "most severe result wins" the union of ALLOW rules is what
applies, so the unconditional-on-modern-kernel rule governs.)

Why moby did this: since Linux 4.8 a tracee's seccomp filter is not escapable via the
tracer, so the historical reason to block `ptrace` disappeared, and blocking it broke
Java/`gdb`/`strace`-in-container. This is documented moby behaviour, not an accident.

**Why this matters for us (the dangerous direction).** The document uses this list as its
evidence for the "cap↔seccomp correlation is defence in depth" recommendation. For
`ptrace` there is *no* second layer: seccomp allows it, and the only remaining control is
the kernel's own `ptrace_may_access()` — same-uid, same PID namespace, plus Yama
`kernel.yama.ptrace_scope`. Consequences the document does not state:

- A container process **can** `ptrace` and `process_vm_readv` another process of the same
  uid inside the same container. That defeats any in-container privilege separation
  (nginx master/worker, a `--user`-dropped helper next to a root sidecar process).
- `--pid=host` + default profile: `ptrace` is *not* blocked by seccomp. What blocks it is
  `CAP_SYS_PTRACE` absence (uid mismatch) and AppArmor's `ptrace peer=docker-default`
  rule. On a host without an LSM, a root container with `--pid=host` and the default
  profile can `process_vm_readv` any host root process. The document's §15.3 row
  ("sharing host PID namespace | … **with `CAP_SYS_PTRACE`**, inject code") understates
  this: reading is available without `CAP_SYS_PTRACE` whenever uids match.

**Correction to make in D5 §8.5:** move `ptrace`, `process_vm_readv`, `process_vm_writev`
out of the "denied" list into a new "allowed on kernel ≥ 4.8 regardless of capabilities"
line, and either (a) state that AppArmor's peer rule is the actual control, or (b)
recommend that *our* profile deny them by default and re-allow under `--cap-add
SYS_PTRACE`. I recommend (b): we are not carrying moby's compatibility debt, and the
`gdb`-in-container use case is exactly what a documented `--cap-add SYS_PTRACE` is for.

Two smaller corrections in the same list: `syslog` is gated on `CAP_SYS_ADMIN` **or
`CAP_SYSLOG`**, and `reboot` on `CAP_SYS_BOOT` (not any of the caps the document
enumerates). Both are still denied under the default 14, so the conclusion holds; the
attribution does not.

### A.3 §8.5 "`clone3` denied with ENOSYS (38), not EPERM" — **CONFIRMED, verbatim**

```json
{"names":["clone3"],"action":"SCMP_ACT_ERRNO","errnoRet":38,
 "excludes":{"caps":["CAP_SYS_ADMIN"]}}
```

`38` is `ENOSYS` on Linux. The rule is a per-syscall `errnoRet` override of the profile's
`defaultErrnoRet: 1`, so the document's framing ("a real design decision, not a formality")
is right, and the glibc mechanism is right: glibc ≥ 2.34 tries `clone3` first and falls
back to `clone` **only on `ENOSYS`** (`sysdeps/unix/sysv/linux/clone-internal.c` sets
`__clone3_supported = 0` on `ENOSYS` and never retries on any other errno). `EPERM` would
propagate to the caller and break `fork`/`pthread_create`.

Note the `excludes.caps: ["CAP_SYS_ADMIN"]` — `clone3` is *allowed* when the container has
`CAP_SYS_ADMIN`, because then the `CAP_SYS_ADMIN` group above allows it outright. The
document does not say this and it is worth knowing: the ENOSYS trick is the
**non-privileged** path only.

**Design constraint confirmed for us**: if our allowlist denies a syscall that glibc
probes-and-falls-back on, the errno must be `ENOSYS`. The known set as of glibc 2.4x is
`clone3`, `faccessat2`, `close_range`, `openat2`, `statx`, `getrandom`, `membarrier`,
`rseq`. Using a blanket `EPERM` for these is a live incident generator, exactly as the
document says.

### A.4 §8.3 "an allowlist eliminates the x32 bypass" — **CONFIRMED for x32, CORRECTED for i386**

`seccomp(2)` on HOST-A (man-pages 6.8-2), verbatim:

> The arch field is not unique for all calling conventions. The x86-64 ABI and the x32 ABI
> both use AUDIT_ARCH_X86_64 as arch … Instead, the mask `__X32_SYSCALL_BIT` is used on the
> system call number to tell the two ABIs apart. This means that a policy must either deny
> all syscalls with `__X32_SYSCALL_BIT` or it must recognize syscalls with and without
> `__X32_SYSCALL_BIT` set. A list of system calls to be denied based on nr that does not
> also contain nr values with `__X32_SYSCALL_BIT` set can be bypassed by a malicious
> program that sets `__X32_SYSCALL_BIT`.

The document quotes this accurately and the **structural claim is sound for x32**: because
`__X32_SYSCALL_BIT` is `0x40000000`, an x32 `nr` can never collide with a legitimate
x86_64 `nr`, so an allowlist keyed on exact x86_64 numbers falls through to
`defaultAction = ERRNO` for every x32 call. No bypass exists. CONFIRMED.

**CORRECTED — the i386 half of rule 1 is wrong as written.** The document says:

> With `defaultAction = ERRNO`, an unknown `nr` — **including every x32-bit-set number and
> every i386 number** — is denied by construction.

i386 syscall numbers are **not** "unknown" — they are small integers that *collide* with
x86_64 numbers. An allowlist that does not check `arch` and allows x86_64 `nr == 26`
(`madvise`) will allow an `int 0x80` caller to invoke i386 `nr == 26`, which is `ptrace`.
The denial is not "by construction"; it is produced entirely by the document's own **rule
2** (check `arch` first, deny anything not `AUDIT_ARCH_X86_64`). Rule 1 and rule 2 are not
independent: rule 1's i386 clause is *derived from* rule 2, and stating it as a property of
allowlisting invites someone to skip rule 2. Rewrite rule 1 to claim only x32.

Two facts the man page carries that the document omits and that we should carry:

1. **Kernels < 5.4** additionally mis-dispatched `nr` in **512–547** and the corresponding
   non-x32 numbers ORed with `__X32_SYSCALL_BIT` — e.g. `nr == 521` reaching `ptrace` with
   confused semantics. On 5.4+ these fail with `ENOSYS`. Our recommended 5.10 floor (§6.3)
   puts us safely past this; **that is an additional, unstated argument for the 5.10 floor
   and should be added to §6.3's justification.**
2. `seccomp(2)`'s own example prologue is the canonical form: if
   `t_arch == AUDIT_ARCH_X86_64`, set `upper_nr_limit = X32_SYSCALL_BIT - 1` and reject
   any `nr` above it *before* the allowlist comparison. That is a two-instruction cBPF
   prologue; we should emit it. The document's rule 2 checks `arch` but not the upper
   bound; adding the bound makes the x32 property explicit rather than emergent.

### A.5 §8.1–8.2, §8.4, §8.6, §8.7 — spot checks

- Action precedence table: CONFIRMED against `seccomp(2)` "Return values" — the ordering
  `KILL_PROCESS > KILL_THREAD > TRAP > ERRNO > USER_NOTIF > TRACE > LOG > ALLOW` and
  "the highest-precedence action is returned" are as stated.
- `seccomp_data` struct layout: CONFIRMED verbatim.
- "All filters run; the most severe result wins" — CONFIRMED; and the corollary the
  document draws (filters can only be added, never relaxed) is correct.
- `EACCES` unless `CAP_SYS_ADMIN` in the caller's userns **or** `no_new_privs`: CONFIRMED
  (`seccomp(2)` ERRORS). The rationale given (a filter could confuse a setuid binary) is
  the man page's own.
- "vDSO calls bypass seccomp entirely" — CONFIRMED in substance, but the document's list
  is slightly off: `getcpu` is a vDSO entry on x86_64, `time`/`gettimeofday`/
  `clock_gettime` are the classic three. The operational rule ("never build a security
  property on filtering those") is right regardless. No verdict change.
- `SECCOMP_FILTER_FLAG_TSYNC` returning the offending TID rather than `-1`: CONFIRMED
  (`seccomp(2)`: "the return value is the ID of one of the threads that could not be
  synchronized").
- §8.7 `SECCOMP_IOCTL_NOTIF_ID_VALID` TOCTOU caveat: CONFIRMED; `seccomp_unotify(2)`
  devotes a whole section to it and explicitly says pointer-following requires the
  ID-valid re-check. The "do not use in MVP" recommendation is well-founded.
- §8.6 "`execve` must be allowed": CONFIRMED — `execve` and `execveat` are both in the
  361-name unconditional group.

---

## B. Capabilities (brief item 3)

### B.1 §7.2 "the bounding set is the security boundary; P/E are convenience" — **CONFIRMED**

Checked against `capabilities(7)` (man-pages 6.8-2 on HOST-A), not the document's paraphrase.

The five-set table and the `execve` transformation formulas are reproduced **verbatim
correctly**, including `P'(permitted) = (P(inheritable) & F(inheritable)) | (F(permitted) &
P(bounding)) | P'(ambient)`. The security reading the document draws from line 2 —
`P(bounding)` masks `F(permitted)`, so a file-capability or setuid-root binary in the image
can never gain a capability outside our bounding set — follows directly and is correct.

The claim that dropping from P/E alone is **not durable** is likewise correct: nothing in
the formula consults the pre-`execve` P or E except through `P(inheritable)`, so an
`execve` of a file with `F(permitted)` re-grants anything still in bounding.

**Material omission, not a refutation: `SECBIT_NOROOT`.** The document says a root
container "gets back its entire bounding set on every `execve`, **whatever we set P and E
to beforehand**." That is true *by default* but is not unconditional. `capabilities(7)`,
Securebits:

> **SECBIT_NOROOT** — If this bit is set, then the kernel does not grant capabilities when
> a set-user-ID-root program is executed, or when a process with an effective or real UID
> of 0 calls `execve(2)`.

So there **is** a mechanism to make P/E smaller than B for a uid-0 container:
`prctl(PR_SET_SECUREBITS, SECBIT_NOROOT|SECBIT_NOROOT_LOCKED)` (requires `CAP_SETPCAP`,
inherited across `fork` and preserved across `execve`). The document even lists
`set_capabilities_secure_bits` in §18.3's rustix inventory without connecting it here.
This does **not** change the conclusion — bounding is still the durable boundary — but
"whatever we set P and E to beforehand" should be qualified with "unless `SECBIT_NOROOT`
is set", and it is a cheap extra hardening knob worth an `[R]`.

### B.2 §7.2 / §7.3 the `0xa80425fb` measurement — **CONFIRMED, re-measured and decoded independently**

Re-ran on HOST-A; identical to the document, byte for byte:

```
/proc/2590614/status (webapp-web-1)      /proc/82271/status (webapp-redis-1)
Uid: 0 0 0 0                                 Uid: 999 999 999 999
CapInh: 0000000000000000                     Gid: 1000 1000 1000 1000
CapPrm: 00000000a80425fb                     CapInh: 0000000000000000
CapEff: 00000000a80425fb                     CapPrm: 0000000000000000
CapBnd: 00000000a80425fb                     CapEff: 0000000000000000
CapAmb: 0000000000000000                     CapBnd: 00000000a80425fb
NoNewPrivs: 0                                CapAmb: 0000000000000000
Seccomp: 2  Seccomp_filters: 1               NoNewPrivs: 0  Seccomp: 2
```

**I decoded `0xa80425fb` by hand rather than trusting `capsh`.** Set bits:

`0xfb` → 0,1,3,4,5,6,7 · `0x25` → 8,10,13 · `0x04` → 18 · `0xa8` → 27,29,31

= CHOWN(0), DAC_OVERRIDE(1), FOWNER(3), FSETID(4), KILL(5), SETGID(6), SETUID(7),
SETPCAP(8), NET_BIND_SERVICE(10), NET_RAW(13), SYS_CHROOT(18), MKNOD(27),
AUDIT_WRITE(29), SETFCAP(31) — **exactly 14**, and exactly the set in moby v28.3.1
`oci/caps/defaults.go`, which I fetched and diffed. Note the *absent* bits that matter:
2 (DAC_READ_SEARCH), 12 (NET_ADMIN), 16 (SYS_MODULE), 17 (SYS_RAWIO), 19 (SYS_PTRACE),
21 (SYS_ADMIN). `CAP_LAST_CAP` on 6.11 is 40, so 41 total and 27 dropped — the document's
arithmetic is right.

**CORRECTED — the attribution of redis's uid 999.** The document says "`webapp-redis-1`
runs `USER redis` (uid 999)". `docker inspect -f '{{.Config.User}}'` is **empty** for all
five containers on the host, and `redis:7-alpine` has no `USER` directive — its
`docker-entrypoint.sh` does `exec su-exec redis "$0" "$@"`. So the uid change is an
**in-container `setuid(999)` by the image's own entrypoint**, not an OCI `process.user`
set by the runtime.

This does not weaken the document's point — it *strengthens* it, and the correction is
worth making because the two paths differ for us:

- The observed `CapPrm = CapEff = 0, CapBnd = 0xa80425fb` is produced by
  `capabilities(7)`'s "Effect of user ID changes" rule, verbatim: *"If one or more of the
  real, effective, or saved set user IDs was previously 0, and as a result of the UID
  changes all of these IDs have a nonzero value, then all capabilities are cleared from
  the permitted, effective, **and ambient** capability sets."* The bounding set is not in
  that list, so it survives. **CONFIRMED — the document's reading of the mask is exactly
  right.**
- But this measurement is evidence about **`su-exec` inside the container**, not about
  what *our runtime* would produce for `--user 999`. If we set `process.user` ourselves we
  perform the `setuid` in the container child and land in the same state — so the
  conclusion transfers — but the document presents an in-container observation as if it
  measured the runtime's behaviour. Label it correctly.

The document also omits that the same man-page bullet clears the **ambient** set. §7.5's
sequence handles this correctly (ambient raise comes *after* the uid change), so this is a
documentation gap, not a design bug.

### B.3 §7.2 "root containers get their whole bounding set back on every `execve`" — **CONFIRMED, with a precision fix**

`capabilities(7)`, "Capabilities and execution of programs by root", verbatim:

> (1) If the **real or effective** user ID of the process is 0 (root), then the file
> inheritable and permitted sets are ignored; instead they are notionally considered to be
> all ones (i.e., all capabilities enabled).
> (2) If the **effective** user ID of the process is 0 (root) **or** the file effective bit
> is in fact enabled, then the file effective bit is notionally defined to be one.

Substituting into the transformation: with `F(inheritable) = F(permitted) = all ones` and
`F(effective) = 1`, `P'(permitted) = P(inheritable) | P(bounding) | P'(ambient) =
P(bounding)` and `P'(effective) = P'(permitted) = P(bounding)`. **The claim holds.**

Two precision fixes to the document's wording:

1. It says "a process with **uid 0**". Rule (1) triggers on **real *or* effective** UID 0;
   rule (2) needs **effective** UID 0. A process with real uid 0 and euid≠0 gets
   `P' = P(bounding)` but `E' = P'(ambient) = 0`. Marginal, but the document presents the
   substitution as if one condition governed both lines.
2. There is a documented exception the document does not mention: "Set-user-ID-root
   programs that have file capabilities" — if the file *does* carry capabilities, rule (1)
   does not apply and the normal formula runs. Since our threat model is exactly "the image
   ships a file-capability binary", this exception is the interesting case, and it is the
   one where the bounding set does all the work. The conclusion is unchanged; the coverage
   is incomplete.

This is also the correct explanation for the observed `CapPrm = CapEff = CapBnd` on the
root container: runc need not have set P/E to the full 14 at all — the `execve` of the
entrypoint would have restored them from bounding regardless. **The measurement cannot
distinguish "runc set P/E" from "the kernel restored them", and the document treats it as
confirming the former.** Not wrong, but it is a MISCLASSIFIED inference: it is evidence for
the kernel rule, not for runc's behaviour.

### B.4 §7.7 the ordering — **CONFIRMED with one CORRECTION**

Each forced ordering checks out against `capabilities(7)` and `prctl(2)`:

- `PR_CAPBSET_DROP` requires `CAP_SETPCAP` **in the effective set** — CONFIRMED
  (`capabilities(7)`, `CAP_SETPCAP` grants "drop capabilities from the calling thread's
  bounding set"; `prctl(2)` `PR_CAPBSET_DROP` errors `EPERM` without it). The trap the
  document names (capset before capbset_drop leaves bounding wide open, silently, if the
  return value is ignored) is real.
- Ambient raise requires the capability in **both P and I** — CONFIRMED (`prctl(2)`:
  `PR_CAP_AMBIENT_RAISE` returns `EPERM` "if the capability … is not present in the
  process's permitted and inheritable sets, or … has been dropped from the bounding set").
- `PR_SET_KEEPCAPS` before the uid change — CONFIRMED (`capabilities(7)`: the flag exists
  precisely so a thread "wants to prevent its permitted capability set being cleared when
  it resets all of its user IDs to nonzero values").
- seccomp last — CONFIRMED (§A.5).

**CORRECTED — step order 3 vs 4 contradicts the document's own text.** The listing at
§7.7 has:

```
 3. setgid(gid) ; setuid(uid)
 4. for cap not in wanted: prctl(PR_CAPBSET_DROP, cap)
```

i.e. uid change **before** the bounding drop. But four lines later the same section says
*"`[R]` Follow runc: bounding drop → capset → uid change with KEEPCAPS → capset again →
ambient."* — the opposite order. And §20's consolidated sequence has C10 (`setuid`) before
C11 (`PR_CAPBSET_DROP`), i.e. it copies the *wrong* one.

This matters and is not cosmetic: **after `setuid` away from 0 without `PR_SET_KEEPCAPS`,
`CAP_SETPCAP` is gone from the effective set, so step 4 fails with `EPERM` and the bounding
set is never dropped.** That is precisely the trap the section itself warns about, reproduced
in its own numbered listing. §20 C9's `[only if uid change + caps wanted]` guard makes it
worse — with no caps wanted, `KEEPCAPS` is skipped, `setuid` clears `CAP_SETPCAP`, and
`PR_CAPBSET_DROP` at C11 fails on every capability. The container then runs with the
**full 41-capability bounding set**.

Fix: make both listings `capbset_drop → capset → [KEEPCAPS] → setgroups/setgid/setuid →
capset → ambient`, matching runc (`libcontainer/capabilities` + `system.SetKeepCaps`,
`standard_init_linux.go` applies caps before `setuid`). This is the single most
consequential error I found in the document.

**Refinement — the bug survives `PR_SET_KEEPCAPS`.** `capabilities(7)`'s uid-change rules
are two separate bullets: the first clears P/E/A when *all* uids leave 0, the second says
*"If the effective user ID is changed from 0 to nonzero, then all capabilities are cleared
from the **effective** set"* — unconditionally, and `PR_SET_KEEPCAPS` preserves only
**permitted**. So even with step 1 executed, `CAP_SETPCAP` is out of E after step 3 and
step 4 fails. A `capset()` restoring E from P must sit between them. §7.5's ambient
sequence does include that `capset()`; §7.7's listing and §20's C9–C13 do not.

**Net verdict on §7.7 / §20:** the prose recommendation is right, the two normative
listings are wrong, and they are the artifacts an implementer would copy.

---

## C. cgroup layout and the no-internal-process rule (brief item 6)

### C.1 §1.3 the rule's exact wording — **CONFIRMED, and the document's careful reading is right**

Fetched `Documentation/admin-guide/cgroup-v2.rst` at **v6.11** (the HOST-A kernel).
"No Internal Process Constraint", verbatim:

> Non-root cgroups can distribute domain resources to their children only when they don't
> have any processes of their own. In other words, only domain cgroups which don't contain
> any processes can have domain controllers enabled in their "cgroup.subtree_control" files.
> …
> The root cgroup is exempt from this restriction.
> …
> **Note that the restriction doesn't get in the way if there is no enabled controller in
> the cgroup's "cgroup.subtree_control". This is important as otherwise it wouldn't be
> possible to create children of a populated cgroup.** To control resource distribution of
> a cgroup, the cgroup must create children and transfer all its processes to the children
> before enabling controllers in its "cgroup.subtree_control" file.

The document quotes the first sentence verbatim and correctly. More importantly, its
three-bullet gloss — that a populated cgroup with an **empty** `subtree_control` **may**
have children, that the common "processes only in leaves" summary is wrong in that
direction, and that root is exempt — is **exactly** what the "Note that the restriction
doesn't get in the way…" paragraph says. This is the paragraph most summaries drop, and
the document did not drop it. CONFIRMED.

### C.2 §1.3 the errno (`EBUSY` both directions) — **UNSUPPORTED as stated, but correct**

The document flags this honestly itself (§23 item 3: "inferred from the documented rule,
not read from `kernel/cgroup/cgroup.c`"), and marks it `[F]` HIGH regardless. `[F]` HIGH
on an admittedly-inferred errno is a tag inflation — it should be `[F]` MED or `[R]`.
The value is right (`cgroup_attach_permissions`/`cgroup_procs_write_start` return `-EBUSY`
via `cgroup_can_be_thread_root`/`cgroup_is_valid_domain` paths, and
`cgroup_subtree_control_write` returns `-EBUSY` when `cgroup_is_populated`), but the
document did not check it and says so. Verdict: **claim correct, tag wrong.** Confirming
it needs a cgroup write — dev-VM experiment §21 item 3, as the document already schedules.

### C.3 §1.3 → the layout consequence — **CONFIRMED, and it follows**

The recommended layout is:

```
/sys/fs/cgroup/mydocker/        subtree_control="+cpu +memory +pids"   0 procs
/sys/fs/cgroup/mydocker/<id>/   subtree_control=""                     container procs
```

This is the only shape the rule permits given (a) we need controllers usable on `<id>`, so
`mydocker` must distribute, and (b) a distributing non-root cgroup may hold no processes.
The consequence is forced, not chosen. CONFIRMED.

Re-measured the live shape on HOST-A and it matches the document's observation:
`system.slice/cgroup.subtree_control` = the full 8 controllers with **0** lines in
`cgroup.procs`; the container scope's `cgroup.subtree_control` is **empty** and
`cgroup.type` is `domain`.

### C.4 §1.3 "a later per-container `exec` sub-cgroup would require moving a live PID 1" — **CONFIRMED**

Follows directly from the "Note that…" paragraph quoted above: *"the cgroup must create
children and transfer all its processes to the children before enabling controllers in its
cgroup.subtree_control file."* To turn `mydocker/<id>/` into a distributor we must first
move every process it holds — PID 1 included — into `mydocker/<id>/init/`. That is a live
migration of a running container's init.

The document's `[R]` MED — design the path helper as `container_cgroup(id).join("init")`
from day one — is the correct pre-emption and is nearly free. **CONFIRMED and endorsed.**

One thing the document does not say that strengthens it: with `CLONE_INTO_CGROUP` (§1.7)
the child is *born* in the target cgroup, so adopting the `init/` sub-path costs literally
one extra `mkdir` and a different fd — there is no runtime cost to taking it now.

### C.5 §1.4 `cgroup.kill` in a threaded cgroup — **the document's own source settles its open question**

§23 item 2 says: *"I claim `ENOTSUP` from the kernel doc's 'not supported in threaded
cgroups', but I did not find the errno in the source."* The errno is **in the kernel
document the section cites**, one paragraph below the text quoted:

> In a threaded cgroup, writing this file fails with **EOPNOTSUPP** as killing cgroups is a
> process directed operation, i.e. it affects the whole thread-group.

On Linux `EOPNOTSUPP == ENOTSUP == 95`, so the claim is right and the open question can be
**closed from `cgroup-v2.rst` v6.11 without any experiment**. Drop §21 item 6 and §23
item 2.

Also: §4.2 quotes `cgroup.kill` as `"not supported in threaded cgroups"` in quotation
marks. That string does not appear in the kernel doc — it is the document's paraphrase
formatted as a quote. Substantively correct, typographically misleading; the real sentence
is the EOPNOTSUPP one above.

---

## D. runc's systemd driver and `TasksMax` (brief items 4 and 7)

### D.1 §2.5 "`UnifiedManager.Set` calls `fsMgr.Set()` in addition to D-Bus" — **CONFIRMED verbatim at v1.2.5**

Fetched `opencontainers/runc` **v1.2.5** `libcontainer/cgroups/systemd/v2.go`:

```go
func (m *UnifiedManager) Set(r *configs.Resources) error {
	if r == nil { return nil }
	properties, err := genV2ResourcesProperties(m.fsMgr.Path(""), r, m.dbus)
	if err != nil { return err }
	if err := setUnitProperties(m.dbus, getUnitName(m.cgroups), properties...); err != nil {
		return fmt.Errorf("unable to set unit properties: %w", err)
	}
	return m.fsMgr.Set(r)          // <-- confirmed
}
```

The document's excerpt is accurate (it elides only the `r == nil` guard and the error
wrapping). **The structural conclusion — the fs driver is a *component* of the systemd
driver, not an alternative — is CONFIRMED**, and it settles the build-order decision:
`FsManager` first, `SystemdManager { dbus, fs: FsManager }` second.

`cgroupFilesToChown()` is also confirmed verbatim, including the three-file fallback when
`/sys/kernel/cgroup/delegate` cannot be opened. The document's `[R]` "copy this exactly;
hardcoding the three-file list is a live bug on ≥ 4.15" is correct — HOST-A publishes
**five** files.

### D.2 §2.4 "`pids.max = 19147` came from systemd's `DefaultTasksMax`, not Docker" — **CONFIRMED, re-measured**

```
docker inspect -f '{{.HostConfig.PidsLimit}}'  (all 5 containers)  ->  <no value>
docker inspect -f '{{.HostConfig.Memory}} {{.HostConfig.NanoCpus}}' ->  0 0
cat .../docker-ee88….scope/pids.max                                ->  19147
cat .../docker-db69….scope/pids.max                                ->  19147
systemctl show --property=DefaultTasksMax                          ->  19147
cat /proc/sys/kernel/threads-max                                   ->  127653
```

19147 / 127653 = 0.14999… — systemd's `DefaultTasksMax=15%`. **CONFIRMED on two separate
containers.** The runc source confirms the causal chain: `genV2ResourcesProperties` emits a
`TasksMax` property **only** when `r.PidsLimit > 0 || r.PidsLimit == -1`, so an unset
`PidsLimit` (0) produces no property and systemd applies its default.

```go
if r.PidsLimit > 0 || r.PidsLimit == -1 {
	properties = append(properties, newProp("TasksMax", uint64(r.PidsLimit)))
}
```

**The derived conclusion — our `inspect` must distinguish *effective* from *declared* — is
sound and follows.** The gap is real, reproducible, and silent.

One qualifier on the document's own §23 item 7 honesty note: it says `[C]` MED on "Docker
does not report it". That demotion is correct and should stay — I did not exhaustively
search Docker's API either. The *observable* part (declared says nothing, kernel says
19147) is `[O]` HIGH and re-verified here.

---

## E. Stopping and killing (brief item 5)

### E.1 §4.2 "`cgroup.kill` is the only race-free container stop" — **CONFIRMED**

`cgroup-v2.rst` v6.11, verbatim:

> **cgroup.kill** — A write-only single value file which exists in non-root cgroups. The
> only allowed value is "1". Writing "1" to the file causes the cgroup and all descendant
> cgroups to be killed. This means that all processes located in the affected cgroup tree
> will be killed via SIGKILL. **Killing a cgroup tree will deal with concurrent forks
> appropriately and is protected against migrations.**

The bolded sentence is the atomicity guarantee the document asserts, stated by the kernel
itself. The document's mechanism gloss ("the kernel … sets the group-kill flag, so a
process that forks during the operation cannot produce a survivor: the child is born into
the cgroup and inherits the pending kill") matches the implementation: `cgroup_kill()` sets
`CGRP_KILL` on the cgroup, iterates the css task list sending SIGKILL, then clears it, and
`cgroup_post_fork()` checks `CGRP_KILL` and kills a child born during the window.
CONFIRMED.

§4.2's `[F]` MED sub-claims are also correct: write-only, only "1" accepted, reads return
`0`, and there is no `cgroup.signal` for arbitrary signals — all stated in the same doc
block. Re-verified `cgroup.kill` and `cgroup.freeze` both exist on every live container
scope on HOST-A.

### E.2 §4.1(1) "SIGTERM to a handler-less PID 1 is silently discarded" — **CONFIRMED, and D1 agrees**

`pid_namespaces(7)` on HOST-A, verbatim:

> Likewise, a process in an ancestor namespace can — subject to the usual permission checks
> described in `kill(2)` — send signals to the "init" process of a child PID namespace
> **only if the "init" process has established a handler for that signal**. … SIGKILL or
> SIGSTOP are treated exceptionally: these signals are forcibly delivered when sent from an
> ancestor PID namespace.

**D1 agrees and measured it** (`D1-kernel-isolation.md` lines 1208–1213, 1406–1407):
`sent SIGTERM rc=0` / `RESULT: sleep(PID1) SURVIVED SIGTERM from ancestor namespace` and
`pidfd_send_signal(SIGTERM) rc=0` / `child alive after SIGTERM (no handler): True`.
D1 §11 also lists it as a tier-"required" driver for the SIGKILL fallback. **No conflict
between D1 and D5 on this.** CONFIRMED, and the §4.4 consequence (`docker stop` on a
handler-less image always takes the full 10 s, and that is kernel behaviour not Docker
behaviour) follows.

Small precision note: D5 explains it as "the kernel does not apply the default terminate
action for signals sent from an ancestor namespace". The man page's framing is that the
signal is not *sent* at all (the check is in `sig_task_ignored()` against
`SIGNAL_UNKILLABLE`). Same observable outcome; the document's mechanism sentence is a
paraphrase, not the rule.

### E.3 §4.1(3) "`exec` children are in the cgroup but not in the process tree" — **CONFIRMED, and subtler than stated**

This is correct, and for a reason the document does not give. When a parent dies the kernel
reparents its children via `find_new_reaper()`, whose default is
`task_active_pid_ns(**father**)->child_reaper` — the init of the *father's* PID namespace,
not the child's. A `mydocker exec` process lives in the **container's** PID namespace but
its parent (our exec helper) lives in the **host's**. So when the helper exits, the exec'd
process is reparented to host PID 1 (or the nearest `PR_SET_CHILD_SUBREAPER` ancestor),
**not** to container PID 1. A walk of the process tree rooted at container PID 1 genuinely
misses it. CONFIRMED.

Qualifier the document should add: such a process is still *in* the container's PID
namespace, so `zap_pid_ns_processes()` will SIGKILL it when container PID 1 dies. It is not
a leak that survives the container — it is a leak that survives a *tree-walking stop
implementation*. The document's conclusion ("the cgroup is the authoritative membership
list, not the process tree") is right; the stated danger is slightly overstated.

### E.4 §4.3 the racy fallback — **CONFIRMED**

All three races are real and correctly described: fork race (hence the loop), PID reuse
(hence `pidfd_open` + `pidfd_send_signal`), freeze race (hence freeze-first). The
recommended fallback — `freeze → read procs → pidfd_open each → pidfd_send_signal(SIGKILL)
→ thaw → verify empty` — is what runc and crun do. `cgroup.freeze`'s "fatal signals still
work while frozen" property, which is what makes it safe, is stated in `cgroup-v2.rst`.
CONFIRMED.

### E.5 §4.4 "do not `SIGKILL` PID 1 as the escalation — kill the cgroup" — **CONFIRMED and endorsed**

The reasoning ("killing PID 1 relies on the PID-namespace-teardown guarantee, which is real
but is a *second* mechanism doing the work; `cgroup.kill` does it directly and works with
`--pid=host` too") is sound. `pid_namespaces(7)` confirms the teardown guarantee exists
("If the 'init' process of a PID namespace terminates, the kernel terminates all of the
processes in the namespace via a SIGKILL signal") and equally confirms it is conditional on
there *being* a PID namespace.

---

## F. `/proc` masking (brief item 12)

### F.1 §10.3 the three mechanisms — **CONFIRMED, re-measured**

From `/proc/2590614/mountinfo` on HOST-A, live:

```
657 1026 0:103 /     /proc/acpi     ro,relatime - tmpfs tmpfs ro,inode64
663 1026 0:104 /     /proc/scsi     ro,relatime - tmpfs tmpfs ro,inode64
664 1029 0:105 /     /sys/firmware  ro,relatime - tmpfs tmpfs ro,inode64
658 1026 0:94 /null  /proc/interrupts rw,nosuid - tmpfs tmpfs rw,size=65536k,mode=755
659 1026 0:94 /null  /proc/kcore      …
660 1026 0:94 /null  /proc/keys       …
661 1026 0:94 /null  /proc/latency_stats …
662 1026 0:94 /null  /proc/timer_list …
598 1026 0:91 /bus   /proc/bus  ro,nosuid,nodev,noexec,relatime - proc proc rw
599 1026 0:91 /fs    /proc/fs   ro,…
600 1026 0:91 /irq   /proc/irq  ro,…
621 1026 0:91 /sys   /proc/sys  ro,…
641 1026 0:91 /sysrq-trigger /proc/sysrq-trigger ro,…
1027 1000 0:94 /     /dev  rw,nosuid - tmpfs tmpfs rw,size=65536k,mode=755,inode64
```

- **masked directory → read-only tmpfs**: CONFIRMED (three fresh superblocks 0:103/104/105).
- **masked file → bind of the container's own `/dev/null`**: CONFIRMED, and the source
  device `0:94` is independently confirmed to be the container's `/dev` tmpfs (mount 1027).
  The document's derived requirement — **`/dev` must be populated before masking runs** — is
  therefore correct and load-bearing.
- **readonly path → self-bind from the same procfs superblock (0:91) + remount ro**:
  CONFIRMED.

### F.2 §10.3 "4 of Docker's 12 masked paths do not exist on 6.11" — **CONFIRMED; but the companion count is wrong**

Fetched moby v28.3.1 `oci/defaults.go`: the static `maskedPaths` list is exactly the 12 the
document reproduces, plus a dynamically computed
`/sys/devices/system/cpu/cpu%d/thermal_throttle` per possible CPU. Re-probed all 12:

```
/proc/asound   ABSENT     /proc/acpi          PRESENT   /proc/interrupts PRESENT
/proc/kcore    PRESENT    /proc/keys          PRESENT   /proc/latency_stats PRESENT
/proc/timer_list PRESENT  /proc/timer_stats   ABSENT    /proc/sched_debug  ABSENT
/proc/scsi     PRESENT    /sys/firmware       PRESENT
/sys/devices/virtual/powercap                 ABSENT
```

**4 absent — CONFIRMED, and the same four the document names.**

**CORRECTED:** the document says "of the 12 masked paths in the source, only **7** appear in
the live mountinfo." Counting the mountinfo above: acpi, interrupts, kcore, keys,
latency_stats, timer_list, scsi, /sys/firmware = **8**. 12 − 4 = 8, so the document's own
arithmetic is inconsistent with its own figure. Trivial, but it is a stated count.

Additionally, `/sys/devices/system/cpu/cpu0/thermal_throttle` is also **ABSENT** on this
virtio host, so moby's *dynamic* masked paths hit `ENOENT` too. That makes the ENOENT-skip
requirement even less optional than the document argues.

### F.3 §10.3 "skip `ENOENT` and only `ENOENT`" — **CONFIRMED against runc's implementation**

runc v1.2.5 `libcontainer/rootfs_linux.go`:

```go
func maskPath(path string, mountLabel string) error {
	if err := mount("/dev/null", path, "", unix.MS_BIND, ""); err != nil && !errors.Is(err, os.ErrNotExist) {
		if errors.Is(err, unix.ENOTDIR) {
			return mount("tmpfs", path, "tmpfs", unix.MS_RDONLY, label.FormatMountLabel("", mountLabel))
		}
		return err
	}
	return nil
}
```

`os.ErrNotExist` (ENOENT) → return nil. Every other error → returned. **Exactly the
document's rule.** CONFIRMED, and it is what a real runtime does, not just a good idea.

Implementation detail the document gets *stylistically* wrong and which is worth copying
instead: runc does **not** stat the path to decide file-vs-directory. It tries the
`/dev/null` bind unconditionally and uses **`ENOTDIR`** as the discriminator for "this is a
directory, use a tmpfs". One syscall, no type check, no branch. The document's "masked
directory → tmpfs; masked file → /dev/null" describes the outcome and implies a
stat-then-branch design.

### F.4 §10.3's two follow-on requirements — one CONFIRMED, one is a **deviation from runc presented as standard**

- `nosuid,nodev,noexec` on `/proc` and `ro,nosuid,nodev,noexec` on `/sys`: CONFIRMED
  verbatim in the mountinfo above.
- **"the read-only remount must … on ≥ 5.12 preferably [use] `mount_setattr(AT_RECURSIVE)`
  — a non-recursive read-only bind leaves submounts writable, which for `/proc/sys` would be
  a complete bypass."** The *principle* is correct. But runc does **not** do this:

```go
func readonlyPath(path string) error {
	if err := mount(path, path, "", unix.MS_BIND|unix.MS_REC, ""); err != nil { ... }
	var s unix.Statfs_t
	unix.Statfs(path, &s)
	flags := uintptr(s.Flags) & (unix.MS_NOSUID | unix.MS_NODEV | unix.MS_NOEXEC)
	return mount(path, path, "", flags|unix.MS_BIND|unix.MS_REMOUNT|unix.MS_RDONLY, "")
}
```

runc binds **recursively** (`MS_REC`) but remounts read-only **non-recursively**. So under
runc, a submount under `/proc/sys` would be present and writable. The document's `[R]` is
therefore *stronger* than runc, which is fine — but it is presented as describing the
mechanism rather than as a deliberate improvement. Label it as a deviation so it does not
get "simplified" back to runc's behaviour in review.

**One requirement the document omits entirely, visible in the same runc snippet:** the
remount must **re-specify the existing `MS_NOSUID|MS_NODEV|MS_NOEXEC` flags**, read back via
`statfs(2)`. `MS_REMOUNT|MS_BIND` sets the per-mount flags to exactly what is passed; flags
not passed are cleared. A naive `MS_REMOUNT|MS_BIND|MS_RDONLY` on `/proc/sys` would make it
read-only **and simultaneously clear `nosuid`, `nodev` and `noexec`** — a net security
regression introduced by the hardening step. This belongs in §10.3 and in §20's C7.

---

## G. Devices and I/O (brief items 9 and 10)

### G.1 §14.1 "two `cgroup_device` programs per container" — **CONFIRMED across all five containers**

```
$ bpftool cgroup tree /sys/fs/cgroup/system.slice
.../docker-ee883c9c….scope   2294 cgroup_device multi
                             2455 cgroup_device multi  sd_devices
.../docker-301b2be6….scope   2128 cgroup_device multi / 2458 sd_devices
.../docker-88ecb99d….scope   2000 cgroup_device multi / 2456 sd_devices
.../docker-db69f93d….scope    471 cgroup_device multi / 2447 sd_devices
.../docker-e66b1ba9….scope    486 cgroup_device multi / 2442 sd_devices

$ bpftool prog show | grep -A2 cgroup_device
471/486/2000/2128/2294: cgroup_device tag ab4bc4523b7fe6b4  xlated 552B jited 361B
2442…2448:              cgroup_device name sd_devices  (gpl)  xlated 416–744B
```

**CONFIRMED exactly**: two per container, `BPF_F_ALLOW_MULTI` (`multi`), runc's anonymous
552-byte program with the **identical tag `ab4bc4523b7fe6b4`** on every container (same
rules ⇒ same program), plus systemd's named `sd_devices`. The document's causal explanation
— it is a consequence of the systemd driver setting `DeviceAllow=` *and* runc attaching its
own program, i.e. §2.5's fs-driver-inside-systemd-driver again — is correct.

### G.2 §14.1 "~95% of the device cgroup's value for zero dependencies" — **MISCLASSIFIED / UNSUPPORTED**

The recommendation is: skip eBPF device control; instead drop `CAP_MKNOD`, deny
`mknod`/`mknodat` in seccomp, and mount the rootfs `nodev`. Attacking the "95%":

**What the substitute genuinely covers, completely:** a malicious *image* or a compromised
in-container process creating a device node for itself. Node creation needs `mknod(2)`/
`mknodat(2)` (there is no other syscall that makes a device node), so cap-drop + seccomp
closes it twice; and a node baked into the image tar is inert if the rootfs is `nodev`.
That threat is fully covered. Good.

**What it does not cover, and the document does not say:**

1. **Devices the operator passes in.** The device cgroup gates `open()` — it mediates
   *access*, not creation. `--device /dev/sda`, or any bind mount of a path under the host
   `/dev`, produces a live node with no `mknod` anywhere. Our substitute has nothing to say
   about it: the node exists, and `nodev` on the *rootfs* superblock does not apply to a
   different mount. Under Docker, a `--device` still passes through a kernel-enforced
   allowlist; under the proposal, `--device` policy is enforced only by our own userspace
   code at mount time, with **no kernel backstop**. That is the single biggest thing lost
   and it is not 5%.
2. **Access modes.** `--device /dev/sdb:r` (read-only), `--device-cgroup-rule`, and
   per-device `rwm` masks are simply unimplementable. The document acknowledges this
   ("add the eBPF device controller later, when `--device` needs fine-grained rules").
3. **Subsystem independence.** §8.5 argues *for* cap↔seccomp correlation on the grounds
   that "the capability check and the seccomp check are enforced by different kernel
   subsystems; a bug in one is covered by the other." §14.1 then discards the *third*
   subsystem — the device cgroup checks at `open()` time, downstream of both. The two
   remaining layers share a failure mode: anything that restores `CAP_MKNOD` or evades the
   filter defeats both at once. The document applies its own defence-in-depth argument in
   §8.5 and drops it in §14.1 without noticing the inconsistency.

**Also flagged — §14.1's supporting bullet (b) is misleading.** It says the control is
redundant because "`CAP_MKNOD` is a no-op outside the initial user namespace". HOST-A
confirms Docker's default containers are **in** the initial user namespace
(`/proc/2590614/uid_map` = `0 0 4294967295`, `ns/user -> user:[4026531837]`). So that fact
is irrelevant to the default rootful configuration the recommendation is written for. The
document's *next* bullet correctly states the gap ("a root container with `CAP_MKNOD` on a
filesystem without `MS_NODEV` can create `/dev/sda`"), which contradicts bullet (b). Delete
(b) or scope it to rootless.

**Verdict:** the recommendation (skip eBPF for the MVP) is defensible and I would still
take it — the dependency cost is real and the image-side threat is fully covered. But
"~95% of the value" is an unsupported number that hides a categorical gap. Restate as:
*"covers the malicious-image and in-container-`mknod` threat completely; covers **nothing**
about devices the operator explicitly passes in, which must then be policed entirely in our
own code. `--device` is therefore gated on the eBPF controller, not optional-with-it."*

Also worth adding as free hardening the document misses: mount the **`/dev` bind of each
passed-through device** with `MS_NOSUID|MS_NOEXEC`, and refuse `--device` for block devices
by default. Neither needs BPF.

### G.3 §3.6 / §19.1 "`io.weight` is a silent no-op — never implement `--blkio-weight`" — **MISCLASSIFIED, and the mechanism is CORRECTED**

The **observation** is CONFIRMED and re-measured on HOST-A:

```
/sys/block/vda/queue/scheduler   ->  none [mq-deadline]
/sys/block/vdb/queue/scheduler   ->  none [mq-deadline]
/sys/fs/cgroup/io.cost.qos       ->  (empty — no cost model)
/sys/fs/cgroup/io.cost.model     ->  (empty)
container io.weight              ->  default 100
container io.max                 ->  (empty)
```

So on this host writing `io.weight` is accepted and changes no scheduling. Correct.

**CORRECTED — the mechanism.** The document says `io.weight` "requires a scheduler that
implements it — **`bfq` or the `io.cost` controller**". That conflates two *different files*:

- `io.weight` is registered by **blk-iocost** (`CONFIG_BLK_CGROUP_IOCOST`). It exists
  whenever iocost is compiled in — HOST-A has `CONFIG_BLK_CGROUP_IOCOST=y` — and is inert
  until iocost is *enabled* on the device via `io.cost.qos`/`io.cost.model`.
- BFQ's proportional weight in cgroup v2 is a **different file, `io.bfq.weight`**, provided
  by `blkcg_policy_bfq`. BFQ does not read `io.weight`.

HOST-A confirms: the container cgroup contains `io.max io.pressure io.prio.class io.stat
io.weight` and **no `io.bfq.weight`** — because `CONFIG_IOSCHED_BFQ=m` and the module is
not loaded.

**CORRECTED — what runc actually writes.** runc v1.2.5 `libcontainer/cgroups/fs2/io.go`
tries `io.bfq.weight` **first** and falls back to `io.weight` only if it does not exist:

```go
bfq, err = cgroups.OpenFile(dirPath, "io.bfq.weight", os.O_RDWR)
...
if bfq != nil {           // Use BFQ, writes the raw 10..1000 value
    bfq.WriteString(strconv.FormatUint(uint64(r.BlkioWeight), 10))
} else {                  // Fallback: io.weight with the conversion
    v := cgroups.ConvertBlkIOToIOWeightValue(r.BlkioWeight)
    cgroups.WriteFile(dirPath, "io.weight", ...)
}
```

So the §3.0 table row "`--blkio-weight=500` → `io.weight` → `4954`" describes only the
**fallback** path, and it describes it with the wrong number (see §M.1).

**MISCLASSIFIED — the conclusion does not generalize.** "Never implement `--blkio-weight`"
is a single-host observation (mq-deadline + BFQ module unloaded + iocost unconfigured)
promoted to a universal rule. On a host where BFQ is the elevator for the target device —
Fedora and Arch udev rules select `bfq` for rotational devices, and it is one `modprobe bfq`
+ `echo bfq > /sys/block/vda/queue/scheduler` away on **this very box**, where BFQ is
built as a module — `--blkio-weight` works exactly as the user expects, through
`io.bfq.weight`. On an iocost-enabled fleet it works through `io.weight`.

Worse, the blanket "never implement" **contradicts the document's own §5.4 policy**, which
is the best thing in the document: *hard-fail on requested-but-impossible, warn on
not-requested, never silently succeed.* The right recommendation is the §5.4 one applied
here:

> Implement `--blkio-weight`. Probe: does `io.bfq.weight` exist in the container cgroup
> (BFQ loaded), or is iocost enabled for the target device (`io.cost.qos` non-empty)?
> If neither, the flag is inert — **refuse it with an error naming the I/O scheduler**,
> exactly as §5.4 requires for `--memory` with no memory controller.

That is ~15 lines and it is more honest than either "implement blindly" or "never
implement". Downgrade §19.1's "**anti-feature**" tier to "conditional, probe-gated".

The parts of §3.6 that *do* hold: `io.max` is absolute throttling with no scheduler
dependency and is the right thing to implement first; resolving the backing device
major:minor for an overlay rootfs is a genuine footgun. `io.stat` at the container shows
only `253:0`; the **root** cgroup's `io.stat` shows `253:16` and the loop devices too — the
document attributes both to the container, minor.

Also: `253:0` is **vda** (`lsblk`: `vda 253:0 disk`), not "the dm/vda device" — there is no
device-mapper in this stack. Cosmetic.

---

## H. Escape classes (brief item 11)

### H.1 CVE-2019-5736 — **CORRECTED (mechanism), requirement CONFIRMED**

What the CVE is: runc container breakout via `/proc/self/exe`, fixed in runc 1.0-rc7 /
Docker 18.09.2. Affects **both** `docker exec` into a container the attacker can write to
**and** `docker run` of an attacker-controlled image. The document scopes it to
"`docker exec`" only — narrower than the CVE.

**The mechanism as described is garbled in two places:**

1. The document says "the container can open `/proc/self/exe` … `O_WRONLY`, and overwrite
   it". The real sequence has two halves. (a) The attacker plants a **shebang**
   `#!/proc/self/exe` in the file runc is about to execute inside the container; when runc
   execs it, the kernel follows the shebang to `/proc/self/exe`, which is the **host runc
   binary**. (b) A *second* attacker process in the container busy-loops on
   `open("/proc/<that pid>/exe", O_WRONLY)`. That open returns **`ETXTBSY`** while the
   binary is running, and succeeds the moment that runc process exits — at which point the
   attacker writes its payload over the host binary.
2. The document says runc's binary is overwritten "with `#!/proc/self/exe` or arbitrary
   code". The `#!/proc/self/exe` is the *bait* planted inside the container; what is written
   to the host binary is the payload. The two are swapped.

The fix is described **correctly**: `memfd_create` + `F_SEAL_WRITE` (with an earlier
`O_TMPFILE` variant).

**The derived requirement survives the garbling** — "any helper binary we `execve` inside
the container's namespaces must not be the on-disk host binary" is exactly the right
mitigation, and it is what runc shipped. CONFIRMED.

**But design 2 is incomplete and the document presents it as complete.** §11.1 recommends
"nothing of ours ever `execve`s inside the container's namespaces — the child that joins the
namespaces is a `fork`, never an `execve` of the host binary." That is sufficient for
`run` (a fresh PID+mount namespace contains no other process that could open
`/proc/<pid>/exe`). It is **not** sufficient for `mydocker exec`: the forked child that
`setns`es into an *existing* container has `/proc/self/exe` pointing at our host binary for
the whole window between `setns` and `execve`, and the container's existing processes can
see it in their own `/proc`. The only thing protecting the binary in that window is
`ETXTBSY` from our own running daemon — a property that evaporates on daemon restart, and
an attacker can hold the loop open indefinitely.

Two cheap fixes the document does not mention:

- **`prctl(PR_SET_DUMPABLE, 0)` on the child before `setns`.** A non-dumpable process's
  `/proc/<pid>/*` becomes root-owned and requires `PTRACE_MODE_ATTACH` to traverse; a
  container without `CAP_SYS_PTRACE` cannot open `/proc/<pid>/exe`. This is what runc does
  in addition to the memfd.
- Or take design 1 (`memfd` + seal) **for the `exec` path only**, keeping design 2 for
  `run`. The document treats the two designs as alternatives; they are complements.

Add an `[R]`: *"design 2 covers `run`; `exec` additionally needs `PR_SET_DUMPABLE(0)` or a
sealed memfd."*

### H.2 CVE-2022-0492 — **CONFIRMED**

What it is: `cgroup_release_agent_write()` in `kernel/cgroup/cgroup-v1.c` performed **no
capability check** on writes to `release_agent`. Fixed by commit 24f6008564183aa
("cgroup-v1: Require capabilities to set release_agent"), which added:

```c
if ((of->file->f_cred->user_ns != &init_user_ns) || !capable(CAP_SYS_ADMIN))
	return -EPERM;
```

The document's description — *"the kernel failed to verify that the writer of
`release_agent` had `CAP_SYS_ADMIN` **in the initial user namespace**, so an unprivileged
user who could create a userns (thereby getting `CAP_SYS_ADMIN` inside it) and mount a v1
cgroup hierarchy could write it. Fixed by requiring `CAP_SYS_ADMIN` in the init userns and
that the file be opened from the initial namespaces"* — matches the fix commit **exactly**,
including the "opened from the initial namespaces" half (`f_cred->user_ns != &init_user_ns`).
CONFIRMED.

The §10.5 exploit sketch is accurate, including the requirement to mount a v1 controller
that is not already mounted. The §11.2 conclusion — three independent controls (never mount
cgroupfs writable; prefer v2 which has no `release_agent`; `CAP_SYS_ADMIN` out of the
default set, which the seccomp `mount` denial mirrors) — is correct, and the claim that
**cgroup v2 has no `release_agent`** is CONFIRMED (`cgroup-v2.rst` has no such file;
`release_agent`/`notify_on_release` are v1-only). This is a legitimate security argument for
v2-only and the document is right to promote it.

### H.3 CVE-2019-14271 — **CONFIRMED**

What it is: Docker 19.03.x before 19.03.3 — "a loading of nsswitch based config inside
chroot under Glibc results in code injection through the container". The `docker cp` path
used a `docker-tar` helper that `chroot`ed into the **container's** filesystem while
retaining host root, and — being cgo-linked and performing a user/group lookup — `dlopen`ed
`libnss_*.so` from the container's `/lib`. A malicious image supplies a hostile
`libnss_files.so`, executed as host root outside all container confinement.

The document's description matches. CONFIRMED.

The derived requirement — *"no host-privileged process of ours may ever resolve a dynamic
library, a config file, an interpreter, or a `PATH` lookup from container-controlled
filesystem contents"* — is broader than the CVE and correctly so. The Rust-specific
concretisation (`std::net::ToSocketAddrs` and the `users` crate reach glibc NSS; musl-static
or never-chrooting avoids it) is accurate. The recommendation to do file copy through an
`O_PATH`/`openat2` handle rooted at the rootfs rather than entering it is the structural
fix and matches what moby did (`docker cp` now uses a chrootarchive that re-execs a
statically-safe helper).

### H.4 CVE-2024-21626 — **CONFIRMED, with one requirement that is wrong as stated**

What it is: runc ≤ 1.1.11 leaked an internal file descriptor (notably the `/sys/fs/cgroup`
handle) across `execve` into the container. `process.cwd` — settable from a Dockerfile as
`WORKDIR /proc/self/fd/7` — then starts the container process with a cwd in the **host**
filesystem namespace. Also enabled arbitrary host-file overwrite variants ("attack 3a/3b").
The document's description is accurate, including that the fd number is non-deterministic
and that no exploit code is needed. CONFIRMED.

Requirements 1 (`O_CLOEXEC` everywhere) and 2 (`close_range(3, ~0U, 0)` before `execve`) are
correct and are exactly runc's fix. Requirement 3 (`openat2` with
`RESOLVE_BENEATH|RESOLVE_NO_MAGICLINKS`) is correct — `openat2(2)` documents
`RESOLVE_NO_MAGICLINKS` as returning `ELOOP` on a trailing magic link, and `/proc/<pid>/fd/N`
is a magic link.

**Requirement 4 is wrong as written.** *"Verify after the fact: `chdir`, then confirm the
cwd's `st_dev` matches the rootfs's."* The container's cwd legitimately sits on a
**different** `st_dev` in ordinary use: any bind-mounted volume, any `tmpfs`, `/dev/shm`, and
an overlay whose upper/lower differ. `WORKDIR /data` where `/data` is a `-v` mount would be
rejected by this check. The correct post-hoc verification is *"the cwd is beneath the
rootfs in the mount tree"* — e.g. resolve it via `openat2(rootfs_fd, …, RESOLVE_BENEATH)`
and compare the resulting handle, or walk `/proc/self/mountinfo` and confirm the cwd's mount
id is the rootfs mount or a descendant of it. As stated, requirement 4 is a false-positive
generator that an implementer will delete the first time a volume breaks — taking the
backstop with it.

### H.5 CVE-2021-30465 (§11.5) and §11.6 — **CONFIRMED**

CVE-2021-30465 is runc's mount-destination symlink race (concurrent `runc run`s, one
swapping a directory for a symlink while the other resolves it), fixed in runc 1.0.0-rc93.
The document describes it accurately. The requirements (never string-concatenate a container
path into `mount(2)`; `openat2` with `RESOLVE_BENEATH|RESOLVE_NO_SYMLINKS|
RESOLVE_NO_MAGICLINKS`; `filepath-securejoin` + operate-on-fd as the pre-5.6 fallback; do
the mounts inside the container's mount namespace after `pivot_root`) are correct and match
runc's own fix and its `securejoin` dependency. §11.6's magic-link description is correct.
CONFIRMED.

---

## I. Kernel floor, feature probing, and the `fsconfig` tension (brief item 8)

### I.1 §6.1 version attributions — **CONFIRMED except one**

Spot-checked every load-bearing row against the man pages on HOST-A and `cgroup-v2.rst`:
cgroup v2 4.5, cgroup ns 4.6, `nsdelegate` 4.13, `/sys/kernel/cgroup/{delegate,features}`
4.15, `BPF_PROG_TYPE_CGROUP_DEVICE` 4.15, `memory.oom.group` 4.19, PSI 4.20, seccomp
user-notify 5.0, `pidfd_send_signal` 5.1, `cgroup.freeze` / `CLONE_PIDFD` / `open_tree` /
`move_mount` 5.2, `clone3` / `pidfd_open` 5.3, `openat2` / time ns 5.6, `CLONE_INTO_CGROUP`
/ `memory_recursiveprot` 5.7, `close_range` 5.9, overlay-in-userns / `CLOSE_RANGE_CLOEXEC`
5.11, idmapped mounts 5.12, `cgroup.kill` / `cpu.max.burst` 5.14, `cpu.idle` 5.15, overlayfs
idmap / `memory.reclaim` / `memory.peak` 5.19, `cgroup.pressure` / `pids.peak` 6.1 — **all
CONFIRMED**.

**CORRECTED — one row.** The table groups "`CAP_BPF`, `CAP_PERFMON`,
`CAP_CHECKPOINT_RESTORE` | **5.8**". `capabilities(7)` on HOST-A:

```
CAP_BPF (since Linux 5.8)
CAP_PERFMON (since Linux 5.8)
CAP_CHECKPOINT_RESTORE (since Linux 5.9)
```

`CAP_CHECKPOINT_RESTORE` is **5.9**, not 5.8. It does not move the floor (5.10 covers both)
but the table is a reference others will cite. Also confirmed independently:
`/proc/sys/kernel/cap_last_cap` = **40** on 6.11, so 41 capabilities — the document's
arithmetic in §7.6 is right.

### I.2 §6.2 "never version-gate features, probe them" — **CONFIRMED and the strongest recommendation in the document**

The motivating case (RHEL 8's 4.18 having overlay-in-userns years before upstream 5.11, via
a Red Hat backport) is `[C]` MED in the document and the document flags in §23 item 6 that
it could not find the RHEL commit. That honesty is correct and the tag should stay MED —
**UNSUPPORTED — pending host** for the RHEL-specific half (`rpm -q --changelog kernel |
grep -i overlay` on ROCKY settles it). The upstream half (5.11) is CONFIRMED.

The rule itself does not depend on that case being true. Two independent supports the
document does not use, which make it airtight:

- Vendor kernels *remove* things too (the document says this but does not instance it);
  Amazon Linux, Oracle UEK and Azure kernels all ship divergent feature sets at the same
  `uname -r` major.minor.
- `seccomp(2)`'s own x32 caveat (§A.4) is version-dependent (< 5.4 mis-dispatch) in a way no
  probe can detect — so version checks are needed for *bug* avoidance even when feature
  probes suffice for *feature* detection. Worth stating: probe for features, and keep a
  small, explicitly-listed set of version checks for known-broken ranges.

The probe list in §6.2 is correct; each named syscall does fail cleanly with the named
errno.

### I.3 **ADJUDICATION: the 5.10 floor vs `fsconfig(FSCONFIG_SET_STRING, "lowerdir+", …)`**

The tension the brief names is real and resolves cleanly against the storage
recommendation. Determined from kernel source and documentation:

| version | `lowerdir+` in `fs/overlayfs/params.c` | `lowerdir+` in `Documentation/filesystems/overlayfs.rst` |
|---|---|---|
| v6.6 | **absent** | absent |
| v6.7 | **present** | present |
| v6.8 – v6.11 | present | present |

and the kernel's own documentation text (identical from v6.7 through v6.11) reads:

> **Since kernel version v6.8**, directory names containing colons can also be configured as
> lower layer using the "lowerdir+" mount options and the fsconfig syscall from new mount
> api.
> …
> ```
> fsconfig(fs_fd, FSCONFIG_SET_STRING, "lowerdir+", "/l1", 0);
> ```

The parameter table in v6.11 `fs/overlayfs/params.c` confirms it:

```c
fsparam_string_empty("lowerdir",  Opt_lowerdir),
fsparam_string("lowerdir+",       Opt_lowerdir_add),
fsparam_string("datadir+",        Opt_datadir_add),
```

**Adjudication: `lowerdir+` requires kernel ≥ 6.7 (documented as 6.8). It is flatly
incompatible with a 5.10 floor as a *requirement*.** Note this is much later than the new
mount API itself (`fsopen`/`fsconfig`/`fsmount`/`move_mount`, Linux **5.2**) — overlayfs was
not converted to the new mount API until 6.7, so on 5.10 `fsconfig` exists but overlayfs
cannot be configured through it at all.

**Both recommendations survive, because §6.2's own rule resolves the conflict.** The
storage agent's `lowerdir+` is not a floor change; it is a **probe-gated fast path**:

```
try:  fsopen("overlay") ; fsconfig(SET_STRING,"lowerdir+",…) per layer ; fsmount ; move_mount
      -> on ENOSYS (no fsopen, <5.2) or EINVAL/ENOPARAM ("lowerdir+" unknown, <6.7): fall back
fallback: mount(2) with a comma-joined "lowerdir=" string, and beat the 4096-byte
          (PAGE_SIZE, `copy_mount_options`) data limit the way overlay2 already does —
          chdir into the layer directory and use short relative paths.
```

That fallback is not hypothetical: moby's `overlay2` graphdriver does exactly the
`chdir`-and-relative-paths trick precisely because of the one-page `mount(2)` data limit.
So a 5.10 floor costs us **nothing structural** here — it costs one fallback path that
already has a known, shipped implementation.

**What must change in D5:** §6.1's table and §6.3's justification say nothing about
overlayfs and the new mount API. Add a row — *"overlayfs via new mount API (`fsconfig`
`lowerdir+`) | 6.7 | escapes the 4096-byte mount-option limit | comma-joined `lowerdir=` +
relative paths"* — and add it to §6.2's probe list. Without that row, someone reading §6.3's
"5.10 gives us every mechanism this document treats as load-bearing" alongside D3's
`lowerdir+` recommendation will conclude they are compatible. They are, but only through the
fallback, and that has to be written down.

**Second, smaller adjudication in the same area:** §6.3's justification for 5.10 should gain
the `seccomp(2)` argument from §A.4 — kernels before **5.4** mis-dispatch x32 syscall
numbers 512–547, which is a *security* reason for the floor that the document does not
currently cite and that is stronger than any of the convenience reasons it does.

---

## J. The two proposed deviations from Docker (brief item 13)

Both are heading for sign-off, so I priced each on three axes: is the benefit real, is the
cost complete, and is anything else broken.

### J.1 `no_new_privs` ON by default (§7.4) — **benefit REAL but OVERSTATED; cost INCOMPLETE. Adopt, with a corrected rationale.**

**Docker's behaviour is confirmed.** `NoNewPrivs: 0` with `Seccomp: 2, Seccomp_filters: 1`
on both containers I re-measured, so runc installed the filter while holding
`CAP_SYS_ADMIN`. `[C]` confirmed: Docker sets NNP only under
`--security-opt no-new-privileges`.

**Benefit 1 — "a container process that escapes to a setuid-root binary" — OVERSTATED for
the default case.** Docker's default container is uid 0 **in the initial user namespace**
(re-measured: `uid_map = 0 0 4294967295`, `ns/user -> user:[4026531837]`). For such a
process:

- a setuid-**root** binary changes nothing — it is already euid 0;
- a **file-capability** binary grants nothing new, because `capabilities(7)`'s
  "execution of programs by root" rule already gives it `P' = E' = P(bounding)` on *every*
  `execve`, and NNP does **not** disable that rule (it is gated on `SECBIT_NOROOT`, not on
  NNP).

So for the default root container, NNP buys essentially **nothing on the capability axis**.
Its real capability-axis value appears only with `--user`, where NNP stops a setuid-root
binary from restoring uid 0 and stops a file-capability binary from granting a capability.
The document's §7.4 rationale reads as though it applied to the default; it does not.

**Benefit 2 — "makes seccomp installation possible after dropping capabilities, removing a
`CAP_SYS_ADMIN`-shaped branch" — REAL and CONFIRMED.** `seccomp(2)` requires
`CAP_SYS_ADMIN` in the caller's userns **or** NNP; mandating NNP deletes one branch and
lets the filter be installed last (§7.7 step 8). This is the strongest argument and should
be promoted to first place.

**Benefit 3 — unstated in §7.4 but real: LSM.** NNP blocks `execve`-time transitions to a
*more permissive* AppArmor/SELinux profile, which applies to root containers too. Worth
adding.

**Costs the document lists:** `sudo`, `su`, `ping` (as setuid), `passwd`, `newgrp`, "loud".

**Costs the document misses:**

1. **File-capability binaries under `--user`.** The modern hardening idiom is
   `setcap cap_net_bind_service=+ep /usr/sbin/nginx` + a non-root user. NNP makes file
   capabilities inert, so this **breaks**, and it breaks *hardened* images specifically.
   The fix — §7.5's ambient-capability sequence — moves the grant from the image to *our*
   `--cap-add`, which is a **policy change**, not just a compatibility break: an image can
   no longer declare its own least-privilege needs. That deserves a sentence.
2. **AppArmor profile transitions.** §12.1 flags this as `[F]` MED and §23 item 10 flags the
   collision, but §7.4's cost list does not mention it at all. The two sections should
   cross-reference: if NNP and our AppArmor plan collide, the deviation is the thing that
   has to give.
3. **Irreversibility and inheritance.** NNP cannot be unset, is inherited across `fork` and
   `execve`, and therefore applies to every `mydocker exec` into that container for its
   whole life. There is no per-exec override. `--no-new-privileges=false` is a *create-time*
   decision only.
4. **Nested rootless tooling.** `newuidmap`/`newgidmap` are setuid/file-capability helpers;
   podman-in-our-container would fail. Niche, but it is the case people file bugs about.

**Verdict: adopt the deviation**, on benefits 2 and 3 plus the `--user` half of benefit 1,
and rewrite the rationale — as written it justifies the change with an argument that does
not hold for the default configuration. Also add §21 item 11 (the breakage inventory) as a
**blocking** experiment, not an optional one: this deviation is the one whose cost the
document has actually not measured.

### J.2 Dropping `CAP_MKNOD` and `CAP_NET_RAW` (§7.6, §14.1) — **`CAP_MKNOD`: adopt, and it is mandatory not optional. `CAP_NET_RAW`: adopt, cost is both overstated and understated.**

#### `CAP_MKNOD`

**Benefit REAL and re-measured.** The container's `/dev` is mounted `rw,nosuid` — **not
`nodev`** (mountinfo line `1027 1000 0:94 / /dev rw,nosuid - tmpfs tmpfs …`), exactly as the
document says, and the container is in the initial user namespace, so `CAP_MKNOD` is fully
functional there. `vfs_mknod()` gates char/block node creation on `capable(CAP_MKNOD)` —
i.e. `ns_capable(&init_user_ns, …)` — so dropping it from the bounding set closes node
creation completely and permanently.

**Reclassification, not a correction:** because §14.1 also declines the eBPF device
controller, dropping `CAP_MKNOD` is not "hardening (deviation)" as §19.2 tiers it — it is
**required**, the load-bearing compensation for having no device cgroup. If someone later
restores `CAP_MKNOD` for compatibility, the device story collapses silently. Tier it
**required**, and gate `--cap-add MKNOD` the way §7.6 gates `--cap-add SYS_ADMIN`. The
document does not say this and it is the obvious way the decision gets undone.

**Cost is smaller than a reader would fear, and the document does not say so.** `CAP_MKNOD`
gates **only** char and block special files. `mknod`/`mknodat` for regular files, FIFOs and
UNIX sockets need no capability at all (`vfs_mknod` checks the cap only for `S_IFCHR`/
`S_IFBLK`), so `mkfifo`, socket creation and `mknod path p` all keep working. State this —
it converts a scary-looking deviation into a nearly free one.

**Cost the document misses:** images that create device nodes at runtime — `/dev/fuse` for
fuse-overlayfs-in-container, `/dev/loop*` for loop mounts, `/dev/net/tun` for VPN
containers, LXD/Docker-in-Docker. Each of these is a real published workaround pattern
(`--cap-add MKNOD --device /dev/fuse`). With the eBPF controller absent, the only answer we
can offer is `--device`, and per §G.2 `--device` has no kernel backstop under this design.
That closes a loop the document leaves open.

#### `CAP_NET_RAW`

**Benefit REAL.** `capabilities(7)`: `CAP_NET_RAW` permits "use of RAW and PACKET sockets"
and "bind to any address for transparent proxying". ARP/DNS spoofing within the container
network is the documented consequence, and CIS Docker Benchmark 5.3 does recommend
restricting the default capability set.

**The stated cost is OVERSTATED — and there is a one-line fix the document does not name.**
Modern `ping` (iputils ≥ 20200821, busybox) prefers `socket(AF_INET, SOCK_DGRAM,
IPPROTO_ICMP)` — a "ping socket" — which needs **no capability**, only membership of a gid
inside `net.ipv4.ping_group_range`. That sysctl is **per-network-namespace**. I read it
through the live containers:

```
$ cat /proc/2590614/root/proc/sys/net/ipv4/ping_group_range   ->  1  0
$ cat /proc/82271/root/proc/sys/net/ipv4/ping_group_range     ->  1  0
$ cat /proc/sys/net/ipv4/ping_group_range        (host)       ->  1  0
```

`1 0` is an empty range (start > end): ping sockets are disabled, so on *this* configuration
ping genuinely does need `CAP_NET_RAW` — the document's stated cost is accurate as far as it
goes. **But we control that sysctl.** Setting `net.ipv4.ping_group_range = 0 2147483647` in
the container's netns (one OCI `linux.sysctl` entry, written by us in the container's
namespaces) makes unprivileged ping work **without** `CAP_NET_RAW`. That turns the entire
stated cost — "one confusing error the first time someone pings" — into zero. This should be
part of the deviation, not a footnote: *"drop `CAP_NET_RAW` **and** default
`net.ipv4.ping_group_range` to `0 2147483647`."*

**Costs the document misses, which are the real ones:**

1. **`AF_PACKET` breaks.** DHCP clients (`dhclient`, `udhcpc`), `tcpdump`, `nmap`,
   `arping`, and raw-socket `traceroute` all need `CAP_NET_RAW`. Irrelevant for
   Docker-managed bridge networking (we configure the netns), **relevant** for macvlan/ipvlan
   and `--net=host` containers that run their own DHCP.
2. **Transparent proxying breaks.** `capabilities(7)` lists "bind to any address for
   transparent proxying" under `CAP_NET_RAW`. `IP_TRANSPARENT` sockets — Envoy, HAProxy and
   any sidecar in transparent-intercept mode — need it. This is the least obvious and most
   likely to surface as a mystery in a service-mesh deployment. The document does not
   mention it at all.
3. `SO_BINDTODEVICE` on some paths.

**Verdict: adopt both drops.** `CAP_MKNOD` should be re-tiered **required**. `CAP_NET_RAW`
should ship with the `ping_group_range` default, and the deviation note should list
`AF_PACKET`/DHCP and `IP_TRANSPARENT` as the actual costs instead of `ping`.

**One thing neither deviation addresses, and it is the bigger fish:** the document itself
observes (§9.2) that Docker's container root **is** host root, and §19.2 tiers the user
namespace as "hardening (the big one)". Two capability drops are a rounding error next to
that. The deviations are worth taking, but the sign-off note should say plainly that they do
not change the security *class* of the runtime — only a user namespace does.

---

## K. LOW-confidence claims (brief item 14)

### K.1 §3.2 PSI for `stats` — **facts CONFIRMED, tag appropriate**

PSI 4.20 and per-cgroup PSI in the same release: CONFIRMED. `some` = at least one task
stalled, `full` = all non-idle tasks stalled: CONFIRMED against
`Documentation/accounting/psi.rst`. `cgroup.pressure` (6.1) disabling per-cgroup accounting:
CONFIRMED. The `[R]` LOW/MED ("strictly better than CPU % for 'is this container starved',
but a `stats`-quality feature, not MVP") is a design opinion correctly tagged LOW. **No
change needed** — this is the document handling uncertainty correctly.

### K.2 §4.7 dying cgroups — **observation CONFIRMED and strengthened; one CORRECTION**

Re-read `/sys/fs/cgroup/cgroup.stat` during this session:

```
nr_descendants        65
nr_dying_descendants  952        (the document recorded 881)
```

Same live count, **952 dying vs the document's 881** — the pool grew by 71 between the two
sessions on an otherwise unchanged host. That is good independent evidence that the
accumulation is real and ongoing, and it strengthens the document's point that "`rmdir` ≠
freed". CONFIRMED.

**CORRECTED:** the document writes *"There is a per-hierarchy cap (`cgroup.max.descendants`,
default `max`) and historically there were kernel bugs where large numbers of dying cgroups
caused measurable slowdown."* `cgroup.max.descendants` limits **live** descendants only;
dying cgroups are not counted against it and there is **no** limit on them. Joining the two
in one sentence implies a bound that does not exist. The slowdown half is substantively
right (the memcg zombie problem, mitigated but not eliminated by objcg reparenting from
5.3 onward) and `[R]` LOW is the right tag for "not worth engineering around".

### K.3 §10.4 `hidepid` — **conclusion CONFIRMED, reasoning MISCLASSIFIED, and a relevant option is missing**

The conclusion ("skip it") is right, and for a reason stronger than the one given: the
container's `/proc` is its own mount, `hidepid` is a mount option only `CAP_SYS_ADMIN` can
change, and the threat it addresses (cross-*user* process visibility) is not our security
boundary — ours is container↔host.

**MISCLASSIFIED reasoning:** the argument leans on *"inside a container everything usually
runs as the same uid, so `hidepid` hides nothing."* That is an empirical generalisation
doing the work of a security argument, and it is false for exactly the configuration the
document elsewhere recommends (`--user`, plus §7.5's non-root + ambient-capability pattern,
plus any image with a master/worker uid split). The conclusion survives; the reason should
be replaced with the mount-option/threat-model one.

**Omission that matters more than `hidepid` itself:** `proc(5)` on HOST-A documents
**`subset=pid` (since Linux 5.8)** — *"Show only the specified subset of procfs, hiding all
top level files and directories in the procfs that are not related to tasks."* That is a
single mount option that removes `/proc/kcore`, `/proc/keys`, `/proc/sched_debug`,
`/proc/timer_list`, `/proc/sysrq-trigger` and the whole `/proc/sys` tree at once — i.e. most
of §10.2's masking table, enforced by the filesystem rather than by a pile of bind mounts.

It is **not** a replacement (it also removes `/proc/meminfo`, `/proc/cpuinfo`, `/proc/stat`,
`/proc/mounts`, which essentially every workload reads, so a general-purpose container
cannot use it), but a §10 section about `/proc` hardening that discusses `hidepid` and omits
`subset=pid` has covered the less relevant of the two. Add it with the "why not by default,
and why it is right for a locked-down profile" analysis.

---

## L. Inherited ROCKY claims (brief item 15)

`old-host` remains unreachable (`old-host.example.edu` → NXDOMAIN), so every ROCKY-dependent claim is
**UNSUPPORTED — pending host** in the strict sense. The question the brief asks is whether
the document *handled* that honestly. Audit of every ROCKY reference:

| line | claim | tag | verdict |
|---|---|---|---|
| §5.2 | v1 legacy, numbered `/proc/self/cgroup` lines, no cgroup2 mount | `[O]` ROCKY (D1), MED | **properly demoted** ✓ |
| §5.4 | Slurm owns the hierarchy; session in `user.slice/…` for pids+memory, root for cpu; nothing delegated | `[O]` ROCKY (D1), MED, "(not re-verified this session)" | **properly demoted** ✓ |
| §6.2 | overlayfs-in-userns works at 4.18 | `[O]` ROCKY (D1), MED | **properly demoted** ✓ |
| §9.3 | no `/etc/subuid`/`subgid` entries; user cannot add them | `[O]` ROCKY (D1), MED | **properly demoted** ✓ |
| §9.3 | `chown`/`setuid` to an unmapped uid → **`EINVAL`** not `EPERM` | `[F]` **MED**, with an explicit "MED rather than HIGH because I could not re-run it on ROCKY" note | **properly demoted** ✓ |
| §12.2 | "ROCKY has SELinux disabled" | `[O]` — **no `(D1)` marker, no MED** | **silent promotion** ✗ |
| §14.3 | "D1 measured `EPERM` on ROCKY with a full 41-capability effective set inside a userns" | inside an `[F]` **HIGH** paragraph | **inherited evidence propping up an `[F]` HIGH** ✗ |
| §16.1 | `mknod` "always fails — `EPERM` … (D1 measured on ROCKY)" | untagged table cell under `[R]` HIGH | same ✗ |

**None of them silently became `[F]` with a fabricated observation** — the document did not
invent ROCKY output, and its §0.1 caveat is honest. Two lapses, both minor:

1. §12.2's SELinux claim is an `[O]` with no provenance marker and no demotion, in a
   section whose *conclusion* ("do nothing for SELinux") rests partly on "we have no
   SELinux test host at all". The conclusion is fine either way (HOST-A is AppArmor, which
   I verified: `docker-default (enforce)`, 140 profiles loaded, 43 in enforce mode), but
   the ROCKY half should carry `(D1)`.
2. §14.3 and §16.1 use an unverified ROCKY measurement as support for `[F]` HIGH / `[R]`
   HIGH statements. **The underlying fact is independently true and does not need ROCKY**:
   `vfs_mknod()` gates `S_IFCHR`/`S_IFBLK` creation on `capable(CAP_MKNOD)`, which is
   `ns_capable(&init_user_ns, CAP_MKNOD)` — so device-node creation requires `CAP_MKNOD` in
   the **initial** user namespace, unconditionally, by kernel source. Cite that instead of
   an unreachable host's measurement, and the `[F]` HIGH becomes justified.

### L.1 The `EINVAL` vs `EPERM` claims can be settled **without** ROCKY

The document demotes these to MED and lists them in §23 item 1 as needing the host. That is
conservative and unnecessary — both are decidable from kernel source, which I did:

- `chown(path, 100, -1)` with uid 100 unmapped: `chown_common()` does
  `uid = make_kuid(current_user_ns(), user)`, then `if ((user != (uid_t)-1) && !uid_valid(uid))
  return -EINVAL;`. **`EINVAL` — CONFIRMED.**
- `setuid(65534)` with 65534 unmapped: `__sys_setuid()` does `kuid = make_kuid(ns, uid)`,
  then `if (!uid_valid(kuid)) return -EINVAL;`. **`EINVAL` — CONFIRMED.**

So §9.3's `[F]` MED can be promoted to `[F]` HIGH on a **source** citation (`fs/open.c`,
`kernel/sys.c`), and §23 item 1's first two bullets can be closed without waiting for the
host. The operational point the document draws from it — that `EINVAL` correctly says "that
identity does not exist here" while `EPERM` would wrongly suggest "get more privilege" — is
right and worth keeping in error messages.

The remaining ROCKY items genuinely need the host, with the commands the document already
gives:

- **`newuidmap` failure text with no `/etc/subuid` entry**: `newuidmap $$ 0 1000 1` as the
  unprivileged user.
- **cgroup v1 write errno on a Slurm-owned hierarchy**: `mkdir /sys/fs/cgroup/memory/test`
  → `EACCES` or `EPERM`?
- **The RHEL overlay-in-userns backport** (§23 item 6):
  `rpm -q --changelog kernel | grep -i overlay`, or grep the RHEL kernel source for
  `FS_USERNS_MOUNT` on overlayfs.

---

## M. Other checkable numbers and source claims

### M.1 §3.0 `--blkio-weight=500` → `4954` — **CORRECTED to `4950` (and the wrong file)**

runc v1.2.5 `ConvertBlkIOToIOWeightValue`:

```go
return 1 + (uint64(blkIoWeight)-10)*9999/990
```

`1 + (500-10)*9999/990 = 1 + 4899510/990 = 1 + 4949 = **4950**`. (Checked the endpoints too:
10 → 1, 1000 → 10000, so the mapping is exactly `[10,1000] → [1,10000]`.) The document's
`4954` is wrong. And per §G.3 the file is `io.bfq.weight` (raw `500`) whenever BFQ is
loaded; `io.weight` is only the fallback.

By contrast `--cpu-shares=512` → `cpu.weight` is **correct**: runc's
`1 + ((512-2)*9999)/262142 = 1 + 19 = 20`, matching the document's "lands near 20".
`ConvertMemorySwapToCgroupV2Value` is quoted **verbatim correctly** (the document elides
only the `memory < 0` sanity case).

### M.2 §3.0 / §3.1 `--oom-kill-disable` "errors in v2" — **REFUTED**

moby v28.3.1 `daemon/daemon_unix.go:verifyPlatformContainerResources()`:

```go
if resources.OomKillDisable != nil && !sysInfo.OomKillDisable {
	if *resources.OomKillDisable {
		warnings = append(warnings, "Your kernel does not support OomKillDisable. OomKillDisable discarded.")
	}
	resources.OomKillDisable = nil
}
```

On cgroup v2 `sysInfo.OomKillDisable` is `false`, so the flag is **warn-and-discard**, not an
error. The string the document quotes — "OOM kill disable is not supported" — does not
appear in moby v28.3.1. Same for `--memory-swappiness`, which the document calls "**silently
dropped**": it is dropped with a **client-visible warning** ("Your kernel does not support
memory swappiness capabilities or the cgroup is not mounted. Memory swappiness discarded."),
so "silently" is also wrong. `--kernel-memory` is likewise warn-and-discard.

**This flips the framing of §3.1's recommendation.** The document says *"do not implement the
flag; if we accept it for CLI compatibility, **reject it with a clear error**, do not
silently ignore it. Silent ignoring of a *safety* flag is the worst option."* I **endorse
the recommendation** — it is the §5.4 policy applied correctly, and hard-failing on a
requested-but-impossible safety flag is right. But it is a **deviation from Docker**, not
compatibility with it, and the document presents it as the latter. Add it to the deviations
list alongside `no_new_privs` and the capability drops.

### M.3 §2.1 / §2.3 / §7.6 / §10.2 source quotations — **CONFIRMED verbatim**

- `cgroupFilesToChown()` including the three-file fallback: verbatim ✓
- `genV2ResourcesProperties`'s `if r.PidsLimit > 0 || r.PidsLimit == -1` TasksMax gate: verbatim ✓
- moby `oci/caps/defaults.go` 14 capabilities: verbatim ✓ (fetched and diffed)
- moby `oci/defaults.go` `maskedPaths` (12) and `readonlyPaths` (5): verbatim ✓, plus the
  dynamic `/sys/devices/system/cpu/cpu%d/thermal_throttle` the document mentions ✓
- `addCpuQuota`'s 1%-rounding comment and formula: verbatim ✓
- `AllowedDevices` + cyphar's "questionable (if not outright wrong)" comment: verbatim ✓
- `UnifiedManager.Set` calling `m.fsMgr.Set(r)`: verbatim ✓

### M.4 §14.2 / §14.3 device set — **CONFIRMED by observation**

Read the live container's `/dev` through `/proc/2590614/root/dev`:

```
full 1,7   null 1,3   random 1,8   tty 5,0   urandom 1,9   zero 1,5
ptmx -> pts/ptmx      fd -> /proc/self/fd    stdin|stdout|stderr -> /proc/self/fd/{0,1,2}
pts/  shm/  mqueue/   core -> /proc/kcore
```

**`/dev/ptmx` is a symlink to `pts/ptmx`, not a `mknod`'d 5:2 node — CONFIRMED**, exactly as
§14.3 requires. The minimum-device table matches (`/dev/console` is absent because these
containers are not `-it`, consistent with the document's own note that console is a bind of
the pty slave). Note `/dev/core -> /proc/kcore`, which is inert here only because
`/proc/kcore` is masked to `/dev/null` — a nice illustration that masking and device setup
are coupled, which the document's "populate `/dev` before masking" ordering already implies.

`/dev` is `rw,nosuid` (no `nodev`, no `noexec`) and the **rootfs overlay is
`rw,relatime` with neither `nodev` nor `nosuid`** — both CONFIRMED, and `nouserxattr` on the
overlay is CONFIRMED, supporting §9.4's point that `userxattr` is the rootless-only option.

### M.5 §12.1 AppArmor — **CONFIRMED by observation**

`/proc/2590614/attr/current` = `docker-default (enforce)`; `aa-status` reports
`140 profiles are loaded. 43 profiles are in enforce mode.`; `/etc/apparmor.d/docker*` does
not exist, confirming the profile is generated by dockerd and loaded directly. All match.

### M.6 §9.2 user namespace — **CONFIRMED by observation**

`/proc/2590614/uid_map` and `gid_map` both `0 0 4294967295`; `ns/user -> user:[4026531837]`
(the initial user namespace); `apparmor_restrict_unprivileged_userns` = 1;
`unprivileged_userns_clone` = 1; `max_user_namespaces` = 63826; `/etc/subuid` and
`/etc/subgid` both `deploy:100000:65536`. Every value matches the document.

**"Container root *is* host root on the default Docker configuration" — CONFIRMED**, and it
is the most important sentence in the document.

### M.7 §3.2 "no memory or CPU limit anywhere on this host" — **CONFIRMED, and no claim was over-stated**

I re-ran the scan (`find /sys/fs/cgroup -name memory.max` / `cpu.max`, comparing against
`max`): **no cgroup on the host carries a non-`max` memory or CPU limit.** The document is
therefore correct that its read-side OOM/throttling claims could not be observed.

Checked whether any of those read-side claims were nonetheless *stated as though observed*:
they were not. §3.1's `memory.max` vs `memory.high` semantics are `[F]` from `cgroup-v2.rst`;
§3.2's `memory.events` field meanings are `[F]`; the `[O]` tags are attached only to the
all-zeros readings and to `nr_periods 0`, which are genuine observations of the *absence* of
limits and are labelled as such. **The document is honest here** — this was the specific
trap the brief asked me to check for, and it is not present.

---

## N. Summary

### N.1 REFUTED

| § | claim | evidence |
|---|---|---|
| 8.5 | `ptrace`, `process_vm_readv`, `process_vm_writev` are "denied" by Docker's default profile because their gating caps are absent | moby v28.3.1 `default.json` allows them via `includes: {"minKernel":"4.8"}` — capability-independent, matches on every modern kernel. Seccomp is **not** a second layer for `ptrace`; only `ptrace_may_access()` + Yama + AppArmor's peer rule are. HOST-A has `kernel.yama.ptrace_scope = 1`, which mitigates but is host-configurable and defaults to 0 on RHEL/Fedora |
| 3.0, 3.1 | `--oom-kill-disable` "errors in v2"; Docker "errors out with *OOM kill disable is not supported*" | moby v28.3.1 `daemon_unix.go` warn-and-discards it (`"Your kernel does not support OomKillDisable. OomKillDisable discarded."`). The quoted string does not exist in the source |

### N.2 CORRECTED (ordered by consequence)

1. **§7.7 listing + §20 C9–C13 — capability ordering is inverted.** Both put `setuid`
   before `PR_CAPBSET_DROP`. `setuid` from 0 clears `CAP_SETPCAP` from the **effective**
   set (and `PR_SET_KEEPCAPS` preserves only *permitted*), so every `PR_CAPBSET_DROP` then
   fails `EPERM` and the container runs with the **full 41-capability bounding set**. The
   section's own prose says the opposite ("Follow runc: bounding drop → capset → uid
   change…"). Fix both listings.
2. **§11.4 requirement 4** — "confirm the cwd's `st_dev` matches the rootfs's" rejects every
   legitimate `WORKDIR` on a volume/tmpfs. Replace with a beneath-the-rootfs mount-tree check.
3. **§3.6 / §3.0** — `io.weight` is **iocost's** file; BFQ's is `io.bfq.weight`, and runc
   writes `io.bfq.weight` first, falling back to `io.weight` only when BFQ is absent. The
   fallback value for `--blkio-weight=500` is **4950**, not 4954.
4. **§11.1 CVE-2019-5736 mechanism** — the `#!/proc/self/exe` shebang is the *bait* inside
   the container, not what is written to the host binary; the write is done by a second
   process via `/proc/<pid>/exe` after `ETXTBSY` clears; the CVE covers `docker run` with a
   malicious image, not only `docker exec`. Also: design 2 ("never exec our binary in
   container ns") does **not** cover `mydocker exec` — add `PR_SET_DUMPABLE(0)` or a sealed
   memfd for that path.
5. **§7.2** — "root gets its bounding set back **whatever** we set P and E to" omits
   `SECBIT_NOROOT`, which disables exactly that. And rule (1) triggers on **real or
   effective** uid 0 while rule (2) needs **effective** uid 0.
6. **§8.3 rule 1** — "every i386 number is denied by construction" is false; i386 numbers
   *collide* with x86_64 numbers. Only the x32 half is structural. The i386 denial comes
   entirely from rule 2 (check `arch`), so rule 1 must not be stated as independent.
7. **§3.0 `--memory-swappiness`** — "silently dropped" is wrong; it is dropped with a
   client-visible warning.
8. **§10.3** — "only **7** [masked paths] appear in the live mountinfo": it is **8**
   (12 − 4 absent). Re-counted from mountinfo.
9. **§7.3** — `webapp-redis-1` does **not** run `USER redis`. `Config.User` is empty on
   all five containers; the uid change is `su-exec` in `redis:7-alpine`'s entrypoint. The
   measurement is evidence about the kernel's uid-change rule, not about runtime behaviour.
10. **§8.5** — `syslog` is gated on `CAP_SYS_ADMIN` **or `CAP_SYSLOG`**; `reboot` on
    `CAP_SYS_BOOT`. Both still denied under the default 14; the attribution is wrong.
11. **§6.1** — `CAP_CHECKPOINT_RESTORE` is **5.9**, not 5.8 (`CAP_BPF`/`CAP_PERFMON` are 5.8).
12. **§4.7** — `cgroup.max.descendants` caps **live** descendants only; there is no limit on
    dying ones. The sentence implies a bound that does not exist.
13. **§4.2** — the quoted `"not supported in threaded cgroups"` is a paraphrase in quote
    marks; the kernel doc's actual sentence names the errno (`EOPNOTSUPP`).
14. **§3.6** — `253:0` is `vda` (`lsblk`), not "the dm/vda device"; there is no
    device-mapper in this stack. `253:16` appears in the **root** cgroup's `io.stat`, not the
    container's.

### N.3 MISCLASSIFIED

| § | issue |
|---|---|
| 3.6, 19.1 | "**never implement `--blkio-weight`**" / tier "anti-feature" — a single-host observation (mq-deadline + BFQ module unloaded + iocost unconfigured) generalized into a universal rule, contradicting the document's own §5.4 "hard-fail on requested-but-impossible" policy. On a BFQ or iocost host the flag works. BFQ is `CONFIG_IOSCHED_BFQ=m` on this very box — one `modprobe` away. Correct recommendation: implement it, probe `io.bfq.weight`/`io.cost.qos`, refuse loudly when inert |
| 14.1 | "**~95% of the device cgroup's value**" — an unsupported number hiding a *categorical* gap: the device cgroup gates `open()`, so `--device` and any bind of a host device node are completely unmediated by the proposed substitute. Also drops the third enforcement subsystem while §8.5 argues *for* subsystem independence |
| 14.1 bullet (b) | "`CAP_MKNOD` is a no-op outside the initial user namespace" — irrelevant to the default rootful container, which **is** in the initial userns (re-measured). Its own next bullet contradicts it |
| 7.2, 7.3 | `CapPrm = CapEff = CapBnd` on the root container is presented as confirming what runc set. The kernel's root-exec rule would produce it regardless — the measurement cannot distinguish the two |
| 10.4 | `hidepid` dismissed because "inside a container everything usually runs as the same uid" — an empirical generalization doing a security argument's work, and false under `--user`. The conclusion (skip it) is right for a different reason |
| 1.3 | the `EBUSY` errno is `[F]` **HIGH** while §23 item 3 admits it was inferred, not read |
| 12.2 | "ROCKY has SELinux disabled" is an `[O]` with no `(D1)` provenance marker and no demotion, unlike every other ROCKY claim |

### N.4 UNSUPPORTED — pending host (`old-host`, NXDOMAIN)

- **`newuidmap` failure text** with no `/etc/subuid` entry → `newuidmap $$ 0 1000 1` as the
  unprivileged user.
- **cgroup v1 write errno** on a Slurm-owned hierarchy → `mkdir /sys/fs/cgroup/memory/test`;
  record `EACCES` vs `EPERM`.
- **RHEL overlay-in-userns backport** (§6.2, §23 item 6) →
  `rpm -q --changelog kernel | grep -i overlay`.

The `EINVAL` claims (§9.3) do **not** need the host — see §L.1; they are decidable from
`fs/open.c:chown_common()` and `kernel/sys.c:__sys_setuid()` and are **CONFIRMED**.

### N.5 Open questions the document can close now, with no experiment

- **§23 item 2 / §21 item 6** (`cgroup.kill` errno in a threaded cgroup): the kernel document
  the section already cites states it — **`EOPNOTSUPP`** (= `ENOTSUP`, 95). Close it.
- **§23 item 1, bullets 1–2** (`EINVAL` for `chown`/`setuid` to an unmapped uid): decidable
  from kernel source, CONFIRMED above. Promote §9.3 to `[F]` HIGH on a source citation.

### N.6 Counts and measurements independently confirmed

| claim | verdict |
|---|---|
| seccomp profile: `SCMP_ACT_ERRNO`, `defaultErrnoRet: 1`, 13 KB (13063 B), 31 groups, **426** distinct names, **361** unconditional | all CONFIRMED |
| `clone3` → `SCMP_ACT_ERRNO`, `errnoRet: 38` (ENOSYS), `excludes.caps: [CAP_SYS_ADMIN]` | CONFIRMED verbatim |
| `socket` AF_VSOCK (`value 40`, `SCMP_CMP_NE`) and `clone` mask `2114060288` (`0x7E020000`) | CONFIRMED verbatim |
| `0xa80425fb` = exactly 14 caps, decoded bit by bit without `capsh` | CONFIRMED; matches `oci/caps/defaults.go` |
| 41 capabilities total (`cap_last_cap` = 40), 27 dropped | CONFIRMED |
| `pids.max = 19147` on **two** containers; `DefaultTasksMax = 19147`; `threads-max = 127653`; 19147/127653 = 15.0% | CONFIRMED |
| `/sys/kernel/cgroup/delegate` lists **5** files | CONFIRMED |
| 12 masked paths, **4** absent on 6.11 (`asound`, `timer_stats`, `sched_debug`, `powercap`) — plus the dynamic `thermal_throttle` paths, also absent | CONFIRMED |
| masked dir → ro tmpfs; masked file → bind of the container's own `/dev/null` (`0:94`, verified to be the container `/dev` tmpfs); readonly path → self-bind from procfs `0:91` + remount ro | CONFIRMED |
| runc skips `ENOENT` **and only** `ENOENT` when masking | CONFIRMED in `rootfs_linux.go` |
| **two** `cgroup_device` eBPF programs per container (runc's 552 B `tag ab4bc4523b7fe6b4` + `sd_devices`), `BPF_F_ALLOW_MULTI`, on **all five** containers | CONFIRMED |
| `io.weight` inert: `mq-deadline` on vda/vdb, `io.cost.qos`/`io.cost.model` empty, no `io.bfq.weight` file | CONFIRMED |
| `UnifiedManager.Set` calls `m.fsMgr.Set(r)` after D-Bus (runc v1.2.5) | CONFIRMED verbatim |
| no-internal-process rule, including the "restriction doesn't get in the way if `subtree_control` is empty" paragraph | CONFIRMED verbatim in `cgroup-v2.rst` v6.11 |
| `pid_namespaces(7)`: ancestor can signal ns-init only if it has a handler; SIGKILL/SIGSTOP excepted — **D1 measured the same and the two documents agree** | CONFIRMED |
| container in the **initial** user namespace, identity `uid_map`, AppArmor `docker-default (enforce)` | CONFIRMED |
| **no memory or CPU limit anywhere on the host**, and no read-side claim was stated as though observed | CONFIRMED — the document is honest here |

### N.7 The two proposed deviations — verdict

**`no_new_privs` ON: adopt, rewrite the rationale.** The seccomp-branch-deletion benefit and
the LSM-transition benefit are real. The headline benefit ("closes the setuid-root escape")
is **overstated for the default root container** — root already gets `P' = E' = P(bounding)`
on every `execve` and NNP does not change that; the benefit is real only under `--user`.
Missing costs: file-capability binaries under `--user` (the modern hardening idiom breaks,
and the fix moves the grant from the image to our `--cap-add` — a policy change), AppArmor
transition interaction (flagged in §12.1 but not priced here), and irreversibility across
every later `exec`. §21 item 11 should be **blocking**, not optional.

**Drop `CAP_MKNOD` + `CAP_NET_RAW`: adopt both, with three changes.** (1) `CAP_MKNOD` is
**required**, not "hardening (deviation)" — it is the load-bearing compensation for skipping
the device cgroup, and `--cap-add MKNOD` must be gated like `--cap-add SYS_ADMIN`. (2) State
that `CAP_MKNOD` gates only char/block nodes — `mkfifo`, sockets and regular files are
unaffected, which makes the cost far smaller than it reads. (3) `CAP_NET_RAW`'s stated cost
(ping) is **removable**: set `net.ipv4.ping_group_range = 0 2147483647` in the container
netns — one sysctl — and modern ping works with no capability. I verified the container's
current value is `1 0` (disabled), so today the cost is real; with the sysctl it is zero.
The **actual** costs, unmentioned: `AF_PACKET` (DHCP clients, `tcpdump`, `arping`) and
`IP_TRANSPARENT` binding (Envoy/HAProxy transparent mode).

Sign-off note both deviations deserve: they do not change the runtime's security **class**.
The document's own §9.2 finding — container root **is** host root by default — dominates
both. Only a user namespace changes the class.

### N.8 5.10 floor vs `fsconfig(FSCONFIG_SET_STRING, "lowerdir+", …)` — adjudicated

`lowerdir+` first appears in `fs/overlayfs/params.c` at **v6.7** (absent at v6.6); the
kernel's own `overlayfs.rst` says *"Since kernel version v6.8"*. Either way it is **far above
a 5.10 floor** — and much later than the new mount API itself (5.2), because overlayfs was
not converted to `fs_context` until 6.7.

**Both recommendations stand, and §6.2's own rule is what reconciles them.** `lowerdir+` is a
**probe-gated fast path**, not a floor change: try `fsopen`/`fsconfig("lowerdir+")`, fall back
on `ENOSYS`/`EINVAL`/`ENOPARAM` to `mount(2)` with a comma-joined `lowerdir=` and the
`chdir`-into-the-layer-dir + relative-paths trick that moby's own `overlay2` driver already
uses to beat the same 4096-byte (`PAGE_SIZE`, `copy_mount_options`) limit. So the 5.10 floor
costs one fallback path that has a shipped reference implementation.

**Required edit to D5:** §6.1's table has no overlayfs/new-mount-API row and §6.2's probe
list does not include it. Add both, or a reader pairing §6.3 ("5.10 gives us every mechanism
this document treats as load-bearing") with D3's `lowerdir+` recommendation will conclude
they are compatible without the fallback. Also add to §6.3's justification the one
*security* argument for the floor the document does not use: kernels **before 5.4**
mis-dispatch x32 syscall numbers 512–547 (`seccomp(2)`), so 5.10 buys correctness of the
seccomp filter itself, not just convenience.
