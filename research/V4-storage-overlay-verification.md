# V4 — Adversarial verification: layer extraction, OverlayFS & storage layout

**Source under review:** `research/D3-images-storage.md`, §6 (extraction hazards), §8 (OverlayFS
mechanics), §9 (overlay2 / snapshotter), §10 (alternatives), §11 (progression), plus §13.3 (`tar`
crate) and the §15 open questions that belong to those sections.
**Not in scope (another verifier):** §1–§4, §7, §12 registry/spec/media-type material.

**Verifier's stance:** falsify. Nothing below is taken from the source document on trust. Every
`[O]` claim was re-run independently; every `[F]` claim was checked against kernel source or the
kernel's own documentation; every `[C]` claim was checked against the upstream repository.

**Method / environment**
- Host `host-a` = `HOST-A`, Ubuntu 24.10, **kernel 6.11.0-29-generic**, x86_64,
  4 cores, 16 GB. Docker 28.3.1, 5 running containers, native overlay. **Production.**
- All experiments run as `unshare -m` (private mount namespace), everything under
  `/tmp/v4-*`, cleaned up and verified clean at the end. No `docker` mutation of any kind.
- `old-host` re-checked for reachability at the top of this session (result in §V4-U).

**Verdict vocabulary:** CONFIRMED / CORRECTED / REFUTED / OUTDATED / UNSUPPORTED / MISCLASSIFIED.

---
## V4-1 — THE HEADLINE CLAIM (§8.5, §9.2): `fsconfig`/`lowerdir+` escapes the 4096-byte limit

### V4-1.1 `lowerdir+` really does escape the limit — **CONFIRMED**, with three ceilings D3 missed

I re-ran the experiment independently (`fsopen`→N×`fsconfig(FSCONFIG_SET_STRING,"lowerdir+")`→
`FSCONFIG_CMD_CREATE`→`fsmount`→`move_mount`, raw syscalls via `ctypes`, private mount namespace,
all dirs under `/tmp`, host 6.11.0-29):

```
N=150 total_path_bytes= 3000 -> OK visible=150
N=300 total_path_bytes= 6000 -> OK visible=300
N=499 total_path_bytes= 9980 -> OK visible=499
N=500 total_path_bytes=10000 -> OK visible=500
N=501 total_path_bytes=10020 -> FAIL at the 501st lowerdir+ call: EINVAL
N=510 total_path_bytes=10200 -> FAIL at the 501st lowerdir+ call: EINVAL
```

The core claim reproduces. `lowerdir+` is not subject to the one-page `mount(2)` `data` limit,
because `mount(2)`'s limit comes from `copy_mount_options()` in `fs/namespace.c`, which `fsconfig`
never calls. **CONFIRMED.**

But the D3 text says the consequence is "no 128-layer cap" and §15.5 lists the ceiling as an
open question ("I verified 500 lower layers … I did not find the ceiling"). Three hard ceilings
exist and D3 states none of them:

**(a) `[CORRECTED]` There IS a hard layer cap, and it is exactly 500.**
`fs/overlayfs/params.h`: `#define OVL_MAX_STACK 500`. Enforced in `ovl_parse_layer()`:
`if (ctx->nr == OVL_MAX_STACK) return invalfc(fc, "too many lower directories, limit is %d", …)`.
D3's "500 verified to work" landed **exactly on the constant** and read it as an unexplored floor.
The 501st `lowerdir+` returns `EINVAL`. Same constant is present in v5.10 (`fs/overlayfs/super.c`),
so this is not new and it applies to the `mount(2)` path too. **§15.5 is now closed: the answer is
500, and it is a compile-time constant, not a length artefact.**
→ D3 §9.2's "impose no 128-layer cap" should read "raise the cap from 128 to the kernel's 500".

**(b) `[CORRECTED]` Each `lowerdir+` value is capped at 255 bytes.**
`fs/fsopen.c`, `FSCONFIG_SET_STRING`: `param.string = strndup_user(_value, 256);` — a value of
256 bytes or more returns `EINVAL`. Measured:
```
lowerdir+ path len=253 -> OK      lowerdir+ path len=256 -> FAIL EINVAL
lowerdir+ path len=254 -> OK      lowerdir+ path len=257 -> FAIL EINVAL
lowerdir+ path len=255 -> OK      lowerdir+ path len=260 -> FAIL EINVAL
```
Harmless for a digest-named store (`/var/lib/x/snapshots/<64hex>/fs` ≈ 100 B) but it is a real
limit that must be asserted, and it is invisible in D3.

**(c) `[CORRECTED — this one is a trap]` The *non-append* form via `fsconfig` is capped at 255 too,
i.e. far worse than `mount(2)`.** Measured:
```
fsconfig(SET_STRING, "lowerdir", <20 colon-joined paths, 399 bytes>) -> EINVAL
fsconfig(SET_STRING, "lowerdir", <2 colon-joined paths,   39 bytes>) -> OK
```
So "use the new mount API" is **not** the escape hatch; "use `lowerdir+`, one path per call" is.
A fallback that reaches for `fsopen`+`lowerdir` on a kernel that has the new mount API but not
`lowerdir+` (i.e. 6.5/6.6) would fail at *255* bytes — 16× worse than `mount(2)`. Write this down.

### V4-1.2 `[O]` **`/proc/mounts` for such a mount is a 15 KB single line** — undocumented cost

D3 recommends this mount path but never says what the mount then looks like. Measured for the
500-layer mount:
```
/proc/self/mountinfo line = 15,085 bytes
/proc/mounts         line = 15,067 bytes
options render as: ro,lowerdir+=/tmp/v4/D/d00000000,lowerdir+=/tmp/v4/D/d00000001,...
```
Two consequences D3 should carry: (i) the options are emitted as **repeated `lowerdir+=` keys**,
not a single `lowerdir=`, so anything that parses `/proc/mounts` looking for `lowerdir=` (including
D3's own §8.4 methodology, and plenty of monitoring agents) will not find it; (ii) a 15 KB mount
line per container is a real cost for anything that reads `/proc/mounts` in a loop.

### V4-1.3 `[CORRECTED / OUTDATED]` `lowerdir+` landed in **v6.7**, not v6.8

D3 §8.5: "since **Linux 6.8**, the new mount API accepts lower layers one at a time". The kernel's
own `overlayfs.rst` also says v6.8 — but the code disagrees with the prose:
`fs/overlayfs/params.c` at tag **v6.7** already contains `fsparam_string("lowerdir+",
Opt_lowerdir_add)`, `Opt_lowerdir_add`, and the `seq_show_option(m, "lowerdir+", …)` renderer;
tag **v6.6** contains none of it. So the feature is present from **v6.7**; the documentation
sentence is one release conservative. Low practical impact (both are far above any plausible
floor) but D3 cites the version as `[F] HIGH` and it is wrong by one release.
Kernel-fd form (`FSCONFIG_SET_FD` with `lowerdir+`) is a separate, later feature: the kernel doc
says **v6.13** for specifying layers as file descriptors. D3 §8.5 asserts `FSCONFIG_SET_FD` with
`[F] HIGH` and no version — on a 6.11 host that call would fail. **UNSUPPORTED as stated; the
fd form is v6.13+, i.e. not available on D3's own test host.**

### V4-1.4 The kernel-floor tension — adjudicated

- `openat2` + `RESOLVE_IN_ROOT` (§6): **5.6**
- unprivileged overlay in a userns + `userxattr` (§8.9): **5.11**
- idmapped mounts (§6.8): **5.12**
- `lowerdir+` (§8.5/§9.2, the headline): **6.7**

A 5.10 floor recommended elsewhere in the corpus is **compatible with `openat2` and nothing else
on this list**. The `lowerdir+` recommendation therefore carries a **6.7 floor**, which excludes
Debian 12 (6.1), RHEL 9 (5.14), Ubuntu 22.04 (5.15) and Ubuntu 24.04 (6.8 — just clears it).
That is not a rounding error; it is most of the installed base.

**Adjudication:** the two are reconcilable, and D3 already half-says so with its `[R] MED`
`mount(2)` fallback — but D3 mis-prices the fallback as "~20 lines … it just caps usable layer
depth". That is wrong, per V4-2 below: the `mount(2)` fallback does **not** cap depth, it
**silently truncates**. Corrected recommendation:

> Floor at 5.10 for the engine. Use `lowerdir+` when `fsopen("overlay")` succeeds *and* a probe
> `fsconfig(fd,SET_STRING,"lowerdir+",…)` returns 0 (a runtime feature probe, not a version
> check — Ubuntu/RHEL backport aggressively). Otherwise use `mount(2)` **with a hard
> pre-flight refusal above 4,095 bytes**, and at that point you need short names — which means
> either a symlink farm or a `chdir`-based relative-path mount, i.e. exactly the Docker
> subsystem §9.2 wants to delete. **The deletion is conditional on a 6.7 floor.**

D3's claim that this is "the largest single simplification available to us versus Docker's design"
is **MISCLASSIFIED** — it is a recommendation contingent on a kernel floor that another part of the
corpus does not grant, presented as an unconditional finding.

### V4-1.5 The `maxDepth=128` / `idLength=26` derivation — **CONFIRMED** (arithmetic), see V4-6 for source

`(26 + len("l") + 1) * 128 = 28 * 128 = 3584 = 4096 - 512`. The identity holds exactly.

---
## V4-2 — §8.5 / §15.2: over-length `mount(2)` does **not** fail. It silently truncates.

This is the most important correction in this document, and it inverts D3's second and third
"things to take from this".

### V4-2.1 `[REFUTED]` "160 lowerdirs / 4,229 B → FAIL `ELOOP`" is not what the kernel does

**Mechanism, from kernel source (v6.11, `fs/namespace.c`):**

```c
static void *copy_mount_options(const void __user * data)
{
	copy = kmalloc(PAGE_SIZE, GFP_KERNEL);
	left = copy_from_user(copy, data, PAGE_SIZE);   /* copies AT MOST one page  */
	...                                             /* over-length is NOT an error */
}
...
static int path_mount(...)
{
	if (data_page)
		((char *)data_page)[PAGE_SIZE - 1] = 0;    /* forcible NUL at byte 4095 */
```

`copy_mount_options()` never reports "too long". It copies the first 4096 bytes and `path_mount()`
NUL-terminates at index 4095. **The option string is silently truncated to 4,095 bytes and the
truncated string is then parsed as if the caller had written it.** Whatever errno you get — or
don't get — is decided by what the *chopped* string happens to mean.

**Boundary, measured (190 lower layers + one final layer with a 40-char name that no prefix of
can exist, total length tuned one byte at a time):**

```
bytes=4090..4095 -> OK   all 190 lowers + final marker visible
bytes=4096       -> FAIL ENOENT
```

**4,095 bytes is the exact usable maximum**, corroborated by moby's own guard
(`daemon/graphdriver/overlay2/overlay.go`): `if len(mountData) > pageSize-1 { … }`.
D3's bracket (3,969 OK / 4,229 FAIL) is consistent but never located the boundary; the number an
implementation must check is **4095**, not 4096.

### V4-2.2 `[REFUTED / MISCLASSIFIED]` `ELOOP` is not the characteristic errno — it is the minority case

D3 §8.5 point 2: "`ELOOP` is a spectacularly misleading errno for 'your option string is too
long'." That generalizes one directory layout to Linux. `ELOOP` here does not mean "too long" at
all: it is `ovl_lookup_trap_inode()` → `"Caught in a trap of overlapping layers"`
(`fs/overlayfs/namei.c`) — i.e. the truncated tail happened to name a directory that is an
**ancestor** of a surviving layer.

I swept layer-path width from 6 to 45 bytes, holding the option string at ~4,400 bytes, so that
the truncation point lands at a different offset inside the last path each time:

```
TALLY over 40 layouts:   ENOENT 33   ELOOP 7   (0 silent, in this flat layout)
```

and again over 34 cut-point shifts in a **docker-shaped `<id>/diff` layout**:

```
TALLY over 34 layouts:   ENOENT 23   ELOOP 5   SILENT-TRUNCATION 6
```

`ENOENT` dominates. `ELOOP` appears when the cut lands on a directory separator; silent success
appears when it lands on an existing non-ancestor directory. **The errno is a property of your
directory naming, not of the kernel's length handling.** D3's observation was correct *for its own
scratch layout* and was generalized.

### V4-2.3 `[REFUTED]` §15.2's "unreproducible" silent truncation — **reproduced, repeatedly, and it is not an anomaly**

D3 §15.2 records one early run where `mount(8)` at ~8.6 KB appeared to succeed exposing ~141 of
300 layers, says careful re-runs "failed cleanly with `ELOOP` every time", and declines to claim
it. **The observation was real.** It reproduces on demand:

```
[nested <id>/diff layout] bytes=4408 -> MOUNT SUCCEEDED, visible=185 of 200
                                        (truncated tail = '/tmp/v4/A/000185')
cutshift= 1 bytes=4410 -> OK visible=131/142   <<< SILENT TRUNCATION
cutshift= 3 bytes=4430 -> OK visible=123/134   <<< SILENT TRUNCATION
cutshift=13 bytes=4437 -> OK visible= 95/103   <<< SILENT TRUNCATION
cutshift=14 bytes=4408 -> OK visible= 92/100   <<< SILENT TRUNCATION
cutshift=31 bytes=4400 -> OK visible= 67/72    <<< SILENT TRUNCATION
cutshift=32 bytes=4410 -> OK visible= 65/71    <<< SILENT TRUNCATION
```

**6 of 34 (18%) of docker-shaped layouts silently mount a wrong filesystem.** The rule is exact:
silent truncation occurs **iff the truncated tail names an existing directory that is not an
ancestor of a surviving layer**. In a `<store>/<id>/diff` layout — Docker's shape, and the shape
D3 §7.4/§11.4 proposes for us — the cut landing anywhere that leaves `<store>/<id>` produces
exactly that, because `<store>/<id>` is a real directory. It is *deterministic per store layout*,
which is precisely why one run reproduces and another does not: D3's re-runs changed the paths.

### V4-2.4 `[NEW — worse than anything D3 records]` a read-write overlay silently becomes read-only

Because truncation eats the **end** of the string, `lowerdir=…,upperdir=…,workdir=…` loses its
`upperdir` and `workdir` entirely. Overlay then legally mounts read-only (`upperdir=`/`workdir=`
are optional, §8.1). Measured:

```
asked for: 139 lower layers + upperdir + workdir   (option string 4,357 bytes)
got:       MOUNTED, ST_RDONLY=True, 131 of 139 layers visible, write -> EROFS
```

A container engine that does this hands the runtime a rootfs that is missing eight layers and
cannot be written to. Note D3 §8.5 explicitly claims "identical results whether the string is
`lowerdir=…,upperdir=…,workdir=…` or `upperdir=…,workdir=…,lowerdir=…`" — the *errno* may look
alike, but the **failure semantics are not identical**: option order decides which options are
destroyed, and putting `lowerdir` last is strictly safer.

### V4-2.5 Consequences for the recommendation

D3 §8.5 `[R]` MED calls the `mount(2)` fallback "~20 lines … it just caps usable layer depth".
**CORRECTED — it does not cap anything; unchecked, it silently produces a wrong filesystem.**
The length check is therefore not a nicety, it is the *only* thing standing between the fallback
and silent image corruption. Restated:

```rust
// MANDATORY on the mount(2) path, not optional:
if opts.len() > 4095 { return Err(OverlayOptionsTooLong { len: opts.len() }); }
// and put lowerdir LAST so a bug here cannot silently drop upperdir/workdir
```

D3 §8.5 point 3's `[R]` HIGH ("we check the length ourselves before calling mount") is right and
is hereby upgraded from "so the error message is nicer" to "so the filesystem is correct".

### V4-2.6 `[CORRECTED]` `mountFrom` is not a fork-exec

D3 §9.2: "`mountFrom` **forks a helper** that `chdir`s into `/var/lib/docker/overlay2`", quoting
moby's comment "requires a fork exec to chroot". The comment is stale; the current implementation
(`daemon/graphdriver/overlay2/mount.go`) is:

```go
func mountFrom(dir, device, target, mType string, flags uintptr, label string) error {
	go func() {
		runtime.LockOSThread()          // thread is never returned to the pool
		unix.Unshare(unix.CLONE_FS)     // private cwd for THIS thread only
		unix.Chdir(dir)
		chErr <- unix.Mount(device, target, mType, flags, label)
	}()
```

No fork, no exec, no chroot — `unshare(CLONE_FS)` on a pinned OS thread. In Rust the equivalent is
a dedicated `std::thread` doing `unshare(CLONE_FS)` + `fchdir(store_fd)` + `mount`, roughly 15
lines. This **weakens** D3 §9.2's "largest single simplification" framing further: the expensive-
sounding component of the thing being deleted is not expensive.

---
## V4-3 — §6: `openat2(RESOLVE_IN_ROOT|RESOLVE_NO_MAGICLINKS)` as "the whole answer"

### V4-3.1 What holds — **CONFIRMED**

`RESOLVE_IN_ROOT` landed in **v5.6** (`include/uapi/linux/openat2.h`: absent at v5.5, present at
v5.6) — D3's `[F]` HIGH is correct, and 5.6 is *below* the 5.10 floor recommended elsewhere in the
corpus, so there is **no tension here**. Re-tested on the host against a fresh root dir:

```
'..'                                              -> /tmp/v4d/root            INSIDE (clamped)
'../../../../etc'                                 -> ENOENT (clamped to <root>/etc)
symlink abs_escape -> /etc            (CVE entry 1) -> ENOENT
'abs_escape/passwd'                   (CVE entry 2) -> ENOENT   <-- the CVE-2019-1002101 /
                                                                    RUSTSEC-2026-0148 shape, dead
symlink rel_escape -> ../../../../etc              -> ENOENT
'/proc/self/root'  (NO_MAGICLINKS on)              -> ENOENT
```

The two-entry symlink-composition escape and the TOCTOU variant are genuinely killed: resolution
is one atomic kernel operation, there is no check-then-use window. **CONFIRMED.**

### V4-3.2 `[REFUTED]` "the whole answer" — four things it does not do

**(a) `RESOLVE_IN_ROOT` clamps silently; it never reports the attack.** `../../etc/passwd` becomes
`<root>/etc/passwd` and is *created there*. Containment is preserved, image correctness is not:
a hostile layer can overwrite `<root>/etc/passwd` by naming an entry `../../etc/passwd`, and
nothing errors. D3 §6.1's reject-don't-sanitize rule is therefore **load-bearing, not
belt-and-braces**, and D3 §6.2's "structurally eliminated" for absolute paths overstates it.
`[R]` **`RESOLVE_BENEATH` is the better primitive for an extractor** and D3 never mentions it —
measured, the same path returns **`EXDEV`** instead of being clamped, so the attack becomes an
error rather than a silent rewrite. Use `RESOLVE_BENEATH|RESOLVE_NO_MAGICLINKS|RESOLVE_NO_XDEV`
for resolution of *existing* components, and reject `..` lexically as well.

**(b) `[NEW]` It does not stop crossing a mount point inside the root.** Measured — bind/tmpfs
mount created inside the extraction root, then resolved:

```
mount point inside root, no RESOLVE_NO_XDEV   -> opened, st_dev 71 != root st_dev 40 : CROSSED FS
same with RESOLVE_NO_XDEV added               -> EXDEV
```

D3's `SafeRoot` recommendation names only `RESOLVE_IN_ROOT | RESOLVE_NO_MAGICLINKS`.
`RESOLVE_NO_XDEV` is missing and should be added: the extraction root lives under
`/var/lib/<engine>` where an operator (or a concurrent container with a shared mount propagation
bug) can plant a mount, and D3 §7.4's "everything on one filesystem" invariant then silently
breaks without detection.

**(c) `[NEW — the important one]` The guarantee is destroyed by mixing in one non-`openat2` call.**
D3 §6.14 step 5 resolves the *parent* with `openat2`, then steps 7–11 use ordinary `*at` syscalls
on the basename. That is only safe if the basename is a strictly single component and the syscall
does not follow symlinks. Measured, resolving the parent correctly and then using a plain
`openat(2)` with a two-component remainder:

```
parent via openat2, basename via plain openat  -> /etc/hosts   *** ESCAPED ***
```

There is **no `openat2` form of `mkdirat`, `symlinkat`, `linkat`, `mknodat`, `unlinkat`,
`fchownat`, `fchmodat`, `utimensat` or `setxattr`** — they all take `dirfd + name` and resolve it
with ordinary rules. In particular **`fchmodat` on Linux has no working `AT_SYMLINK_NOFOLLOW`**, so
step 10 applied to a name that an earlier entry made a symlink *is literally RUSTSEC-2026-0067*.
D3's checklist survives this only because step 6 unlinks an existing name before creating — i.e.
the safety comes from `openat2` **plus** single-component basenames **plus** unlink-before-create,
and if any one of the three is dropped the hole reopens. D3 presents it as one structural decision
that "kills most of the list at once". `[R]` state the three-part invariant explicitly and assert
`!name.contains('/')` in the `SafeRoot` API's type, not in a comment.

**(d) It says nothing about what you do with the fd.** setuid bits, device nodes, xattrs, uid/gid
and hardlink *targets* are all still yours to police. D3 does cover these in §6.4–§6.8; the
objection is only to the framing in the §6 preamble.

### V4-3.3 §6.7 `security.capability` — **CONFIRMED**, mechanism verified

- `capabilities(7)` mechanism is as described. Verified on the host (tmpfs, real root): writing
  a well-formed `struct vfs_cap_data` succeeds and is preserved verbatim —
  `setxattr OK, 20 bytes back, magic_etc=0x02000001 (revision 2, EFFECTIVE set)`. A v2 capability
  written by host root **is** honoured in every namespace; that is exactly the escalation D3
  describes. Note the kernel validates the blob: an arbitrary value (`b"0x01"`) is rejected
  `EINVAL`, so a malformed `SCHILY.xattr.security.capability` record fails loudly rather than
  silently — a small mercy D3 does not mention.
- `VFS_CAP_REVISION_3` (the namespaced form with a `rootid`): absent at **v4.13**, present at
  **v4.14**. D3's "since Linux 4.14" — **CONFIRMED.**
- `trusted.*` requires `CAP_SYS_ADMIN`: confirmed indirectly — as real root, `trusted.overlay.opaque`
  set fine; the D3 concern (a layer forging `trusted.overlay.opaque`) is real because *we* are root
  during extraction. **The allowlist recommendation stands and I agree it is MVP-mandatory.**
- Minor `[CORRECTED]`: D3 says "Reject `security.selinux`, `security.ima`, `security.evm`". On this
  host `security.selinux` **is settable** (no SELinux policy loaded) while `security.ima` returns
  `EINVAL`. Rejecting is still right; but do not expect the kernel to reject them for you — it did
  not for `security.selinux`.

### V4-3.4 §6.9 the 5,000,000 entry cap — **MISCLASSIFIED (recommendation dressed as a limit)**

D3 labels this `[R]` LOW and admits it is a guess, which is honest. But the number is not derived
from anything: a tar of 5,000,000 zero-byte entries at 512 B/header is a **2.4 GB** archive that
would already be stopped by the `descriptor.size` cap and by the 10 GiB uncompressed cap. The
binding constraint is inode exhaustion on the store filesystem, which is a *filesystem* property
(ext4 default ~1 inode per 16 KB), not a tar property. `[R]` replace the magic number with a check
against `statfs(store).f_ffree` at extraction start, or drop the limit — it is currently a
number that cannot be violated without violating two other limits first.

### V4-3.5 §6.10 `PATH_MAX` bonus — **CONFIRMED**, and it is why the 255-byte `lowerdir+` cap matters

D3 §6.10 `[R]` MED is right that `openat2` from a dirfd sidesteps total-`PATH_MAX`. Note the
symmetric fact this file adds: on the **mount** side you are hard-capped at **255 bytes per
`lowerdir+` value** (V4-1.1b), so the store root path plus digest plus `/fs` must stay under 255.

---
## V4-4 — §6.3 / §13.3: the `tar` crate advisories and the "parser only" recommendation

### V4-4.1 RUSTSEC-2026-0067 — **CONFIRMED, verbatim**

Fetched from `rustsec/advisory-db/crates/tar/RUSTSEC-2026-0067.md`:

```toml
id = "RUSTSEC-2026-0067"; package = "tar"; date = "2026-03-19"
aliases = ["CVE-2026-33056", "GHSA-j4xf-2g29-59ph"]
[affected.functions]
"tar::Entry::unpack"    = ["< 0.4.45"]
"tar::Entry::unpack_in" = ["< 0.4.45"]
[versions] patched = [">= 0.4.45"]
```
Title: "`unpack_in` can chmod arbitrary directories by following symlinks". D3's quotation of the
body is **word-for-word correct**. Affected `<= 0.4.44`, fixed `0.4.45` — correct.
CVSS is 4.0 `AV:N/AC:L/AT:N/PR:N/UI:A/VC:N/VI:L/VA:N/…` (i.e. **low integrity impact, requires user
interaction** — a modest score, not a critical). D3 does not overstate it; noting it for accuracy.

Crate metadata also checked against crates.io: `tar` **0.4.46**, updated **2026-05-18**,
`recent_downloads = 51,265,108`, `repository = https://github.com/composefs/tar-rs`. Every figure
in D3 §13.3 — **CONFIRMED.**

### V4-4.2 `[REFUTED]` "the parsing is boring, well-tested, and not where the danger lives"

D3 §13.3's central argument for the split is that `tar`'s *reader* is safe and only the *applier*
is dangerous. **There is a second advisory, filed the same day against the same versions, that D3
does not mention and that contradicts this**: `rustsec/advisory-db/crates/tar/RUSTSEC-2026-0068.md`

> **RUSTSEC-2026-0068 / CVE-2026-33055 / GHSA-gchp-q4r4-x4ff**, `tar < 0.4.45`, 2026-03-19 —
> "tar-rs incorrectly ignores PAX size headers if header size is nonzero … Any discrepancy in how
> tar parsers honor file size can be used to create archives that appear differently when unpacked
> by different archivers. In this case, the tar-rs (Rust tar) crate is an **outlier** in checking
> for the header size — other tar parsers (including e.g. Go `archive/tar`) unconditionally use
> the PAX size override."

That is a **parser** bug, of the entry-smuggling / extractor-differential class, in the exact
component D3 declares safe. The remediation is unchanged (`tar >= 0.4.45`, which D3 already says),
but the *reasoning* must be corrected: using `tar` as a parser is a choice that still needs
defence in depth. `[R]` add to the extractor: after each entry, assert that the bytes actually
consumed equal the size the header claimed, and reject any entry where a PAX `size` record and the
ustar header size disagree — rather than trusting whichever the crate picked.

### V4-4.3 `[CORRECTED]` CVE attribution for "TARmageddon"

D3 §6.3/§13.3: "`async-tar` (**CVE-2025-62518**, 'TARmageddon') and `astral-tokio-tar`
(CVE-2025-59825)".

- `rustsec/advisory-db/crates/**`: **`async-tar` has no advisory directory at all.** The
  TARmageddon-family advisory in RustSec is **RUSTSEC-2025-0111, against `tokio-tar`**.
  `astral-tokio-tar` carries RUSTSEC-2025-0110 (plus four 2026 ones).
- **CVE-2025-62518** is filed against **`astral-tokio-tar`** (GHSA-j5gw-2vrg-8fgx, "astral-tokio-tar
  Vulnerable to PAX Header Desynchronization", `<= 0.5.5`, patched **0.5.6**), *not* `async-tar`.
  The advisory itself says the class "was disclosed to multiple Rust tar parsers, all derived from
  the original `async-tar` fork of `tar-rs`" — that lineage is presumably what D3 meant, but the
  crate/CVE pairing as written is wrong.
- **CVE-2025-59825** is `astral-tokio-tar` `<= 0.5.3`, patched **0.5.4** — D3's quotation of the
  paired-symlink `allow_external_symlinks` bypass is **CONFIRMED**.

### V4-4.4 `[CORRECTED]` §6.12: global headers are not the TARmageddon mechanism

D3 §6.12 on `TypeXGlobalHeader`: "never let a global header mutate defaults for later entries
(**that mechanism is exactly what 'TARmageddon'-class bugs exploit**)." Per CVE-2025-62518's own
text the mechanism is *"incorrect stream position advancement when PAX headers override size
information"* — a **per-entry `x` header size desync**, not a `g` global header. Ignoring `g` is
correct hygiene and should stay; it does not mitigate the bug class D3 names it for. The mitigation
that does is in V4-4.2.

---
## V4-5 — §8: OverlayFS mechanics against `Documentation/filesystems/overlayfs.rst` (v6.11)

### V4-5.1 §8.3 lowerdir ordering — **CONFIRMED three independent ways**

1. **Kernel doc, v6.11 line 347**, verbatim and identical to D3's quote: *"The specified lower
   directories will be stacked beginning from the rightmost one and going left. In the above
   example lower1 will be the top, lower2 the middle and lower3 the bottom layer."*
2. **My own two-layer experiment** (private mount ns, `/tmp`):
   `lowerdir=lowA:lowB → f.txt = 'A-version'`; `lowerdir=lowB:lowA → f.txt = 'B-version'`.
3. **Live production container, walked independently of D3's table.** I started from
   `layerdb/mounts/<cid>/parent` (the top image layer's chain ID) and followed `parent` links down
   to the base, printing each layer's `diff` and `cache-id`, then compared with the running
   container's `lowerdir` resolved through `l/`:

```
 layerdb walk (top -> base)              live mount lowerdir (left -> right)
 chain=daae7f3b80ff diff=803a8e1dc20f -> a5f79ae06f34   <- init layer (no diff_id)
   cache-id=1ab184527cb3                 1ab184527cb3   == diff_ids[8]  (TOP)
 chain=72293fb3b69d cache=547cda7a9ff9   547cda7a9ff9   == diff_ids[7]
 chain=72937144df87 cache=5bab11c42954   5bab11c42954
 chain=1007ac3ce997 cache=c88211795b80   c88211795b80
 chain=ac995c361ecd cache=b99b39b26a44   b99b39b26a44
 chain=809b468f82a9 cache=b45d72d839b4   b45d72d839b4
 chain=7e3a0e5ee76d cache=c56b584a7442   c56b584a7442
 chain=5828c9b15438 cache=b2f31d872d08   b2f31d872d08
 chain=4f35986d820c cache=42534ff23a4c   42534ff23a4c   == diff_ids[0]  (BASE)
```

Exact match, top-first, with the `-init` layer prepended above the image's topmost layer.
**`lowerdir` == `reverse(diff_ids)`. CONFIRMED.** D3 §8.3 and §9.3 are right, including the init
layer's position. Also confirmed on the same host: 106 layer dirs under `overlay2/` and 106 `l/`
symlinks, of which **14 have no `lower` file** (base layers) — matches D3 §9.1.

### V4-5.2 §8.1 workdir rules — one **CORRECTED**, two additions

- Doc, v6.11 line 126: *"The 'workdir' needs to be **an empty directory** on the same filesystem as
  upperdir."* D3's quote drops "empty" from the sentence and states it separately — fine.
- `[CORRECTED]` **the kernel does not enforce it.** Measured: a workdir containing a stray file
  mounted successfully; afterwards `workdir` held `['junk', 'work']`. So "must be empty" is a
  contract you must enforce yourself; a leftover file from a crashed run will not be caught.
- `[NEW]` `workdir` *inside* `upperdir` → **EINVAL**; `workdir == upperdir` → **EINVAL**. Both are
  enforced. Worth knowing because the obvious layout `snapshots/<id>/{fs,work}` is fine but
  `snapshots/<id>/fs/work` is not.

### V4-5.3 §8.1 `[REFUTED as a guarantee]` `EBUSY` on a shared upperdir/workdir

D3 quotes the doc as a rule: *"Using an upper layer path and/or a workdir path that are already
used by another overlay mount is **not allowed and may fail with EBUSY**."* Measured at 6.11:

```
two overlay mounts, identical upperdir AND workdir -> mount1=OK  mount2=OK
dmesg: overlayfs: workdir is in-use as upperdir/workdir of another mount,
       accessing files from both mounts will result in undefined behavior.
```

The kernel **warns in `dmesg` and mounts anyway**. The doc's own next sentence is the operative
one: *"If files are accessed from two overlayfs mounts which share or overlap the upper layer
and/or workdir path the behavior of the overlay is undefined."* For a container engine this is a
`[R]` **HIGH** item D3 does not have: **you must keep your own mount bookkeeping** (an exclusive
lock or refcount per snapshot). Starting the same container twice will not be stopped by the
kernel; it will silently corrupt the writable layer.

### V4-5.4 `[NEW — hidden dependency in §11.4 step 6]` a read-only overlay needs **≥ 2** lower layers

```
lowerdir=<one layer>,  no upperdir   -> FAIL EINVAL
   dmesg: overlayfs: at least 2 lowerdir are needed while upperdir nonexistent
lowerdir=<two layers>, no upperdir   -> OK
lowerdir=<one layer> + upper + work  -> OK
lowerdir=<same layer twice>          -> FAIL ELOOP  (dmesg: conflicting lowerdir path)
```

D3 §11.4 step 6's acceptance criterion is "OverlayFS read-only mount of a committed stack
(`lowerdir` only, no upper)". **That is impossible for a single-layer image** — and `alpine`, the
image D3 uses as its running example throughout, has exactly one layer. Duplicating the layer to
make two entries does not work either (`conflicting lowerdir path`, `ELOOP`). The single-layer
case must be handled by adding an `upperdir`+`workdir`, or by bind-mounting the diff directly.
Note also this is a **third** legitimate source of `ELOOP` that has nothing to do with option
length (V4-2.2).

### V4-5.5 §8.6 whiteout representation — **CONFIRMED**, kernel-produced

Doc line 148: *"A whiteout is created as a character device with 0/0 device number or as a
zero-size regular file with the xattr `trusted.overlay.whiteout`."* Reproduced:

```
rm merged/keep.txt        -> upper/keep.txt  type=chardev  mode=0000  rdev=0:0
rm -rf + mkdir merged/adir -> trusted.overlay.opaque = b'y'
```
The `opaque="x"` marker for directories holding *alternative* whiteouts, and the
`overlay.overlay.` escape prefix, are both in the doc as D3 states. **CONFIRMED.**
The alternative-whiteout sentence D3 quotes in §8.9 is verbatim at line 547.

### V4-5.6 `[REFUTED]` §8.8: "tmpfs — no `trusted.*` xattr support"

D3 §8.8's list of what does *not* work as `upperdir` includes "tmpfs (no `trusted.*` xattr
support — but tmpfs *does* work with `userxattr` on modern kernels)". **This is backwards.**

- Measured on the host's tmpfs: `setxattr trusted.overlay.opaque` → **OK**;
  `setxattr user.overlay.opaque` → **OK**.
- Full overlay with **tmpfs `upperdir` + `workdir`, default (`trusted.overlay.*`) namespace,
  no `userxattr`** → **mounts, and produces a correct char 0:0 whiteout on delete.**
- What actually arrived late in tmpfs is **`user.*`** xattr support: `XATTR_USER_PREFIX` is absent
  from `mm/shmem.c` at **v6.5** and present at **v6.6**. tmpfs has carried `trusted.*` and
  `security.*` via `simple_xattr` for many years.
- The kernel doc (line 101-104) states the requirement correctly: the upper *"must support the
  creation of `trusted.*` **and/or** `user.*` extended attributes, and must provide valid d_type in
  readdir responses, so NFS is not suitable."*

Practical consequence for this corpus: **`/tmp` on the host is tmpfs** (`df -T /tmp` →
`tmpfs`), so every `[O] HOST-A` overlay experiment in D3 §8 — including the whiteout
demonstration in §8.6 — was run on a filesystem §8.8 declares unusable. The results happen to be
valid; the §8.8 entry is not.

### V4-5.7 §8.7 optional features — versions verified, one mechanism **CORRECTED**

Landing versions, established by presence/absence in `fs/overlayfs/` at consecutive tags:

| Feature | First present | Absent at | D3 states |
|---|---|---|---|
| `metacopy` | **v4.19** | v4.18 | no version |
| `volatile` | **v5.10** | v4.19 | no version |
| `userxattr` | **v5.11** | v5.10 | v5.11 ✅ |
| overlay `FS_USERNS_MOUNT` (unpriv. in userns) | **v5.11** | v5.10 | v5.11 ✅ |
| `lowerdir+` / `datadir+` | **v6.7** | v6.6 | v6.8 ❌ (V4-1.3) |
| layers as file descriptors (`FSCONFIG_SET_FD`) | **v6.13** (doc) | 6.11 | asserted, unavailable on the test host |
| `RESOLVE_IN_ROOT` (`openat2`) | **v5.6** | v5.5 | v5.6 ✅ |
| `VFS_CAP_REVISION_3` (namespaced fcaps) | **v4.14** | v4.13 | v4.14 ✅ |

All six optional mount options are accepted at 6.11 (`metacopy=on`, `redirect_dir=on`, `index=on`,
`xino=on`, `userxattr`, `nfs_export=on` all mounted). D3's `[R]` "use none of them for MVP" is
sound and I agree with it; the `metacopy` "expect the unexpected" and `index` `ESTALE` reasoning
are both in the doc as quoted.

`[CORRECTED]` **`volatile`**: D3 says the marker *"forces a fresh upper on next mount"*. It does
not. Measured:

```
mount -o volatile          -> OK; leaves workdir/work/incompat/volatile/dirty
remount WITHOUT volatile   -> FAIL EINVAL
dmesg: overlayfs: overlay with incompat feature 'volatile' cannot be mounted
```
Kernel doc line 764: *"overlay checks for this directory and **refuses to mount** if present."*
Nothing is reset for you — the caller must delete the workdir. D3's `[R]` LOW ("useful for
throwaway build steps") survives; the mechanism sentence is wrong and would produce a stuck
container on first use.

### V4-5.8 §8.4 the live Docker option set — **CONFIRMED**, with one clarification

Re-read from `/proc/mounts` on the live host: 5 overlay mounts, `lowerdir` field lengths
**671–887 bytes** (8–12 layers), `rw,relatime,…,nouserxattr`, absolute paths through `l/`.
`docker info`: `overlay2`, `Backing Filesystem: extfs`, `Supports d_type: true`,
`Using metacopy: false`, `Native Overlay Diff: true`, `userxattr: false`. Docker 28.3.1.
`/sys/module/overlay/parameters/`: `index=N metacopy=N redirect_dir=N xino_auto=Y
redirect_always_follow=Y redirect_max=256 nfs_export=N check_copy_up=N`. All as D3 reports.
`/var/lib/docker` and `/var/lib/docker/overlay2` are `drwx--x--- root:root` — §6.6 confirmed.

Clarification on D3 §8.7's "Docker passes `index=off` explicitly": `index=off` does **not** appear
in `/proc/mounts` (overlayfs does not render default-valued options). It is nonetheless true —
`overlay.go` sets `indexOff = "index=off,"` whenever `/sys/module/overlay/parameters/index` exists,
which it does here. D3's claim stands; the evidence for it is the source, not the mount table.

---
## V4-6 — §9: overlay2, the `l/` arithmetic, and the snapshotter model

### V4-6.1 §9.2 the moby constants — **CONFIRMED verbatim**

`moby/daemon/graphdriver/overlay2/overlay.go`, current master, reproduced exactly as D3 quotes it:
`linkDir = "l"`, `maxDepth = 128`, `idLength = 26`, and the comment carrying
`((idLength + len(linkDir) + 1) * maxDepth) <= (pageSize - 512)`. The arithmetic
`28 * 128 = 3584 = 4096 - 512` holds exactly. The cap is enforced twice — in overlay2's
`getLowerDirs` (`if len(lowers) > maxDepth { return "", errors.New("max depth exceeded") }`) and
again in the graphdriver-agnostic layer store (`ErrMaxDepthExceeded`). The page-size fallback is
`if len(mountData) > pageSize-1` → i.e. **4095**, independently corroborating V4-2.1.

### V4-6.2 `[CORRECTED]` §9.2's comparison arithmetic is off, and it hides a better point

D3: "A full layer path is `/var/lib/docker/overlay2/<64 hex>/diff` = 94 characters, +1 for the
colon = 95 bytes per layer. `4096 / 95 ≈ 43` layers. The indirection buys **128 layers instead of
~43** … `mountFrom` … 28 bytes per layer instead of **51**."

Counted against the live host: `/var/lib/docker/overlay2/l/YXMKWDGGR4WAVV2US7SKMQ6WRH` is
**53** characters (27-char prefix + 26-char id), so **54** bytes per layer with the colon, not 51.
94+1 = 95 for the raw form is right.

The sharper point D3 misses: `28 × 128 = 3584` is the budget for the **relative** form used only
by `mountFrom`. Docker's *primary* path emits **absolute** `l/` paths at 54 B each, and the option
string also carries `index=off,` + `upperdir=`(~103 B) + `workdir=`(~102 B). Usable budget
≈ 4095 − 9 − 103 − 102 − 10 ≈ 3,871 B ⇒ **≈ 71 layers**. So **Docker cannot reach its own
`maxDepth = 128` without falling back to `mountFrom`**; the constant is solved for the fallback,
not the common path. That strengthens D3's "delete this subsystem" argument on the merits even as
V4-1.4 weakens it on the kernel floor.

### V4-6.3 §9.3 the `-init` layer — **CONFIRMED** independently

`layerdb/mounts/<cid>/`: `mount-id: a5f79ae06f34…`, `init-id: a5f79ae06f34…-init`,
`parent: sha256:…`. The live mount's leftmost (topmost) `lowerdir` entry resolves to the `-init`
directory, sitting above the image's topmost layer and below `upperdir`. Exactly as D3 describes.
D3's `[R]` MED to skip the init layer and use bind mounts instead is sound and I have no objection.

### V4-6.4 `[UNSUPPORTED]` §11.5: "Docker carries `cache-id` **because** it cannot rename the overlay2 directories"

The mechanism is real — `layerdb/sha256/<chainID>/cache-id` maps a content-addressed chain ID to a
random graphdriver directory name (`cacheID: stringid.GenerateRandomID()` in
`daemon/internal/layer/layer_store.go`). The **causal claim is not supported**, on three counts:

1. **It is not an overlay2 feature.** `cache-id` lives in the graphdriver-*agnostic* layer store and
   is used identically by aufs, btrfs, zfs, vfs and (formerly) devicemapper. For devicemapper the
   graph "directory" is a thin block device and for zfs it is a dataset — "cannot rename the
   directory" is not even a meaningful statement there, so it cannot be the general reason.
2. **The actual reason is visible in the source**: `func (ls *layerStore) RegisterByGraphID(graphID
   string, parent ChainID, diffID DiffID, tarDataFile string, size int64)` in
   `daemon/internal/layer/migration.go` — the Docker 1.10 content-addressability migration had to
   **adopt pre-existing graphdriver directories, whose names were already random v1 ids, under
   newly computed chain IDs, without moving any data.** The indirection is a migration artefact
   that was never removed, not a filesystem constraint.
3. To the extent a rename *is* hard for overlay2, that is self-inflicted: each child layer's
   `lower` file embeds `l/<link>` names, so renaming one directory means rewriting every
   descendant's `lower` file. D3's own recommendation deletes `l/` and `lower` — which removes the
   obstacle it cites as the reason to fear the indirection.

**The recommendation itself is fine** (`layers/` by `diff_id`, `snapshots/` by chain ID is the
right keying, and getting it right up front is cheaper than migrating). It just does not follow
from the stated premise, and the premise as written would mislead a reader into thinking a kernel
or filesystem limitation is at work. **MISCLASSIFIED: inference presented as an observation.**

### V4-6.5 §11.5's "expensive to change" list is missing an item

D3 lists three lock-ins (keying, one-filesystem, `tar-split`). A fourth belongs there, and D3
itself supplies the evidence in §8.6: **the xattr namespace a layer was extracted with**. A layer
whose whiteouts/opaque markers are `trusted.overlay.*` cannot be used under a `userxattr` mount and
vice versa; switching later means **re-extracting the entire store**, exactly like `tar-split`.
D3 says "record it in `info.json`", which is necessary but does not make it cheap to change.
Related: D3 §6.8 already concedes that under a user namespace "the store must be keyed by
`chainID + idmap`, not chainID alone" — which contradicts §11.5's flat `diff_id` keying unless
idmapped mounts (5.12+) are adopted. The two sections should be reconciled explicitly.

### V4-6.6 §9.4 snapshotter model — **CONFIRMED as description**, and the three "good ideas" are sound

The `graphdriver` vs `snapshotter` table is an accurate description of containerd's design, and the
three adopted ideas (Active/Committed as a type, return a `Mount` description rather than a path,
`Prepare`/`Commit` with a temporary key) are good engineering judgement. These are `[R]` items
correctly labelled as such. One reinforcement from this verification: **idea 2 is not optional** —
because `lowerdir+` mounts must be built with `fsopen`/`fsconfig` and then `move_mount`ed into the
target namespace, the storage layer physically cannot "return a mounted path" without mounting in
the wrong namespace. The `Mount` description must carry the *list* of lower layers, not a
pre-joined `lowerdir=` string, or the whole `lowerdir+` benefit is thrown away at the interface.

---
## V4-7 — §10: alternatives

- **§10.5 devicemapper "removed in Docker Engine v25" — CONFIRMED.** `daemon/graphdriver/devmapper`
  returns HTTP 200 at moby tag `v24.0.0` and **404** at `v25.0.0`; the removal PR is moby#43637
  "Remove deprecated devicemapper storage driver", merged 2023-05-18.
- **§10.6 "the `tar` crate is now maintained under `github.com/composefs/tar-rs`" — CONFIRMED**
  from crates.io metadata (`repository = https://github.com/composefs/tar-rs`).
- **§10.1 plain extracted directory / `native` — no objection**, and V4-5.4 makes it *more*
  attractive: it is the only backend that works for a single-layer image without special-casing.
- **§10.4 btrfs/zfs dismissal — MISCLASSIFIED (mild).** It is labelled `[R]` LOW so the framing is
  honest, but "irrelevant to us" is a project decision, not a finding; the supporting facts (zfs
  out-of-tree, btrfs snapshot-count scaling) are correct and uncontested. Left as recommendation.
- **§10.3 `fuse-overlayfs` 2–5× metadata slowdown** — `[C]` MED at best; no citation is given and I
  did not measure it. **UNSUPPORTED as a number**; the qualitative claim (userspace round-trips are
  slower) is uncontroversial. It changes no recommendation.
- **§10.7 lazy pulling out of scope — no objection.**

---
## V4-8 — §11: the recommended progression

**Overall: the ordering is right and I would keep it.** The two corrections (split the registry
client from the extractor; make "directory rootfs" a permanent `native` backend) are both good and
I found nothing against them. The insertion of OCI image-layout import before OverlayFS does what
D3 says: after step 4, steps 5–8 need no network. Confirmed by inspection of the dependency graph —
no step *requires* a later one. Three refinements:

1. **`[NEW]` Step 6's acceptance criterion is unachievable for a single-layer image** (V4-5.4:
   "at least 2 lowerdir are needed while upperdir nonexistent"). Since `alpine` — the image used as
   the running example in §2 and as step 3's success criterion — is single-layer, step 6 will fail
   on the first thing anyone tries. Restate step 6 as "read-only mount of a ≥2-layer committed
   stack; single-layer stacks fall back to `native` or to a bind mount of the diff".
2. **`[NEW]` Step 7 needs a mount-exclusivity mechanism** that D3 does not list anywhere. Per
   V4-5.3 the kernel will let two containers share an `upperdir`/`workdir` with only a `dmesg`
   warning and undefined behaviour. That belongs in step 7's done-when, e.g. "starting the same
   snapshot twice returns an error, not a second mount".
3. **`[R]` Move the option-length guard into step 6, not step 8.** Per V4-2, an unguarded
   `mount(2)` fallback silently produces a wrong filesystem. The check is three lines and it is the
   difference between "fails loudly on kernels < 6.7" and "silently corrupts on kernels < 6.7".

---
## V4-9 — §15 open questions: what is now closed

| # | D3's status | Verdict |
|---|---|---|
| 15.2 silent truncation at ~8.6 KB | "could not reproduce", `[O]` LOW | **REFUTED — it reproduces.** Not an anomaly: it is the kernel's documented-in-source truncation behaviour, occurring in **6 of 34 (18%)** docker-shaped layouts, deterministic per layout. See V4-2.3. |
| 15.5 kernel max overlay stacking depth | "did not find the ceiling" | **CLOSED: exactly 500** (`#define OVL_MAX_STACK 500`, `fs/overlayfs/params.h`; present unchanged since ≤ v5.10). The 501st `lowerdir+` returns `EINVAL`. D3's "500 verified to work" landed on the constant itself. See V4-1.1a. |
| 15.1 `old-host` unprivileged overlay at 4.18 | not verified, host down | **UNSUPPORTED — pending host.** Re-checked this session: `ssh old-host` → `Could not resolve hostname old-host.example.edu`; independent `nslookup old-host.example.edu` → **NXDOMAIN**. D3's account is unchanged and unverifiable from here. Settling commands below. |
| 15.1 NFS `upperdir` errno | not verified, host down | **UNSUPPORTED — pending host.** The kernel doc *does* support the reasoning (v6.11 line 101-104: the upper "must support the creation of `trusted.*` and/or `user.*` extended attributes, and must provide valid d_type in readdir responses, **so NFS is not suitable**"), but the errno is still unobserved. Settling commands below. |
| 15.6 `tar-split` | decided-by-default | Not re-litigated; agree it is a real accept-or-not decision. Add the xattr-namespace lock-in beside it (V4-6.5). |

**Exact commands that would settle the `old-host` items**, for whoever gets a shell there:

```sh
# 1. unprivileged overlay at 4.18 — which mechanism?
grep -i overlay /proc/filesystems
cat /sys/module/overlay/parameters/* 2>/dev/null
unshare -Ur -m sh -c 'mkdir -p /tmp/o/{l,u,w,m};
    mount -t overlay overlay -o lowerdir=/tmp/o/l,upperdir=/tmp/o/u,workdir=/tmp/o/w /tmp/o/m; echo rc=$?'
unshare -Ur -m sh -c 'mkdir -p /tmp/o2/{l,u,w,m};
    mount -t overlay overlay -o userxattr,lowerdir=/tmp/o2/l,upperdir=/tmp/o2/u,workdir=/tmp/o2/w /tmp/o2/m; echo rc=$?'
rpm -q --changelog kernel 2>/dev/null | grep -i 'overlay.*\(userns\|user namespace\)' | head

# 2. NFS as upperdir — capture errno AND dmesg
mkdir -p /tmp/nl "$HOME/ovl-u" "$HOME/ovl-w" /tmp/nm
strace -f -e trace=mount mount -t overlay overlay \
    -o lowerdir=/tmp/nl,upperdir=$HOME/ovl-u,workdir=$HOME/ovl-w /tmp/nm ; dmesg | tail -5
```

---
## V4-10 — Scoreboard

### CONFIRMED (23)

1. `fsconfig`+`lowerdir+` mounts 500 lower layers / ~10–12 KB of paths, far past `mount(2)`'s page
   limit (re-run independently, raw syscalls). §8.5
2. moby's `linkDir`/`maxDepth = 128`/`idLength = 26` block and the `((idLength+len(linkDir)+1)*
   maxDepth) <= (pageSize-512)` identity → `28*128 = 3584 = 4096-512`. §9.2
3. `lowerdir` == `reverse(diff_ids)`, leftmost is topmost — kernel doc v6.11 line 347 + my own
   two-layer experiment + an independent `layerdb` walk on a live production container. §8.3
4. Docker's `-init` layer sits above the image's topmost layer and below `upperdir`. §9.3
5. overlay2 on-disk shape (`committed`/`diff`/`link`/`lower`/`work`, base layers have no `lower`;
   106 layer dirs and 106 `l/` symlinks; 14 base layers). §9.1
6. Live Docker option set, `/sys/module/overlay/parameters/*`, `docker info` fields. §8.4
7. Docker does pass `index=off` (from source; invisible in `/proc/mounts`). §8.7
8. `openat2(RESOLVE_IN_ROOT|RESOLVE_NO_MAGICLINKS)` kills `..`, absolute paths, absolute-symlink
   escape, the two-entry CVE-2019-1002101 / RUSTSEC-2026-0148 shape, and magiclinks, with no
   check-then-use window. §6
9. `RESOLVE_IN_ROOT` added in Linux **5.6** (absent v5.5, present v5.6). §6
10. `VFS_CAP_REVISION_3` (namespaced file caps) added in **4.14**. §6.7
11. `security.capability` mechanism and its escalation potential; well-formed v2 blob written by
    host root is preserved verbatim and applies in all namespaces. §6.7
12. `userxattr` and overlayfs `FS_USERNS_MOUNT` both first appear at **5.11**. §8.9
13. `metacopy` first appears at **4.19**; `volatile` at **5.10** (versions D3 omitted). §8.7
14. RUSTSEC-2026-0067 / CVE-2026-33056 exists and says exactly what D3 quotes; `tar <= 0.4.44`
    affected, `0.4.45` fixed; affected functions `Entry::unpack`, `Entry::unpack_in`. §13.3
15. `tar` 0.4.46, updated 2026-05-18, 51,265,108 recent downloads, repo `composefs/tar-rs`. §13.3
16. RUSTSEC-2026-0148 / CVE-2026-46703 (`boxlite`, CVSS 10.0) exists; `SafeRoot` fix in 0.9.0. §6.3
17. CVE-2025-59825 (`astral-tokio-tar` ≤0.5.3, fixed 0.5.4) and the paired-symlink
    `allow_external_symlinks` bypass quotation. §6.3
18. Kernel-produced whiteout is a char device 0:0 mode 0000; opaque dir is
    `trusted.overlay.opaque="y"`; `"x"` marks dirs holding alternative whiteouts. §8.6
19. Upper filesystem must supply `d_type` and `trusted.*` and/or `user.*` xattrs; "so NFS is not
    suitable" is the kernel doc's own words. §8.8
20. devicemapper removed in Docker Engine **v25** (present at moby v24.0.0, absent at v25.0.0). §10.5
21. `workdir` must be on the same filesystem as `upperdir`; omitting `upperdir`/`workdir` gives a
    read-only overlay; lower layers may be freely shared between mounts. §8.1
22. containerd snapshotter vs graphdriver description, and the three ideas worth adopting. §9.4
23. The §11 progression contains no backward dependency — every step's prerequisites precede it,
    and image-layout import at step 4 does make steps 5–8 offline. §11

### REFUTED (6)

| # | Claim | §  | Evidence |
|---|---|---|---|
| R1 | over-length `mount(2)` "FAILs with `ELOOP`" | 8.5 | `copy_mount_options()` truncates to one page; `path_mount()` NULs byte 4095. Boundary measured at **4095 OK / 4096 FAIL**. V4-2.1 |
| R2 | the silent-truncation observation "could not be reproduced" | 15.2 | Reproduced on demand; **6 of 34 (18%)** docker-shaped layouts mount successfully with layers missing. V4-2.3 |
| R3 | a shared `upperdir`/`workdir` is "not allowed and may fail with `EBUSY`" (relied on as a rule) | 8.1 | Two mounts with identical upper+work both returned **OK**; only a `dmesg` warning. V4-5.3 |
| R4 | tmpfs has "no `trusted.*` xattr support" and needs `userxattr` | 8.8 | tmpfs accepts `trusted.overlay.*` and works as `upperdir` with the default namespace. `user.*` is the late arrival (**tmpfs v6.6**). V4-5.6 |
| R5 | `openat2(RESOLVE_IN_ROOT)` is "the whole answer" / makes escape "impossible" | 6, 6.3 | `openat2`-resolved parent + one plain `openat` on a two-component basename → **escaped to `/etc/hosts`**. No `RESOLVE_NO_XDEV` ⇒ mount points inside the root are crossed. `..` is clamped silently, never reported. V4-3.2 |
| R6 | "the parsing is boring, well-tested, and not where the danger lives" | 13.3 | **RUSTSEC-2026-0068 / CVE-2026-33055**, same crate, same versions, same day: tar-rs ignores PAX size headers when the base header size is nonzero — a parser-level entry-smuggling bug. V4-4.2 |

### CORRECTED (13)

C1 §15.5/§9.2 the layer ceiling is **exactly 500** (`OVL_MAX_STACK`), not unknown and not absent ·
C2 §8.5 each `lowerdir+` value is capped at **255 bytes** (`strndup_user(_value, 256)`) ·
C3 §8.5 `fsconfig(SET_STRING,"lowerdir", …)` — the *non-append* form — is capped at 255 too, i.e.
**16× worse than `mount(2)`**; only `lowerdir+` escapes ·
C4 §8.5 `lowerdir+` landed in **v6.7**, not v6.8 (code at tag v6.7; absent v6.6) ·
C5 §8.5 the `FSCONFIG_SET_FD` layer form is **v6.13**, unavailable on the 6.11 test host ·
C6 §8.5 the usable option-string maximum is **4095**, not 4096 ·
C7 §9.2 `mountFrom` is **not a fork-exec** — `unshare(CLONE_FS)` on a `LockOSThread`ed goroutine ·
C8 §9.2 an absolute `l/` entry is **54 B** not 51, and Docker **cannot reach `maxDepth=128`** on the
absolute path (~71 layers) without falling back to `mountFrom` ·
C9 §8.1 "workdir must be empty" is documented but **not enforced** at 6.11 ·
C10 §8.7 `volatile`'s marker makes the next mount **fail `EINVAL`**; it does not "force a fresh
upper" · C11 §6.7 `security.selinux` was **settable** here — do not expect the kernel to reject the
namespaces you mean to reject · C12 §6.3/§13.3 **CVE-2025-62518 is `astral-tokio-tar`**, not
`async-tar`; `async-tar` has no RustSec advisory, the family advisory is **RUSTSEC-2025-0111**
against `tokio-tar` · C13 §6.12 the TARmageddon mechanism is **per-entry PAX size desync**, not
`g` global headers.

### MISCLASSIFIED (5)

M1 §9.2 "the largest single simplification available to us" — a recommendation contingent on a
**6.7 kernel floor** that another part of the corpus does not grant, stated unconditionally ·
M2 §11.5 "Docker carries `cache-id` *because* it cannot rename overlay2 directories" — inference
presented as observation; the source says migration (`RegisterByGraphID`) and the mechanism is
graphdriver-agnostic · M3 §8.5 "`ELOOP` is a spectacularly misleading errno" — a single-layout
observation generalized to Linux; `ENOENT` is the majority outcome and `ELOOP` genuinely means
overlapping layers · M4 §6.9 the 5,000,000-entry cap — an unfounded number that cannot bind before
two other limits do · M5 §10.4 btrfs/zfs "irrelevant to us" — a project decision (correctly
labelled `[R]` LOW, noted for completeness).

### UNSUPPORTED (3)

U1 §15.1 unprivileged overlay at RHEL-8 4.18 — **pending host** (`old-host` re-checked this session:
NXDOMAIN). Settling commands in V4-9 · U2 §15.1 the NFS-`upperdir` errno — **pending host**; the
kernel doc supports the reasoning, the errno remains unobserved · U3 §10.3 `fuse-overlayfs`
"2–5× on metadata-heavy workloads" — no citation, not measured; changes no recommendation.

### NEW — hazards D3 does not record (6)

N1 **A read-write overlay silently becomes read-only** when truncation eats `upperdir`/`workdir`:
asked for 139 layers + upper + work, got a read-only mount with 131 layers and `EROFS` on write ·
N2 **A read-only overlay needs ≥2 lower layers** ("at least 2 lowerdir are needed while upperdir
nonexistent"), so §11.4 step 6 is unachievable for a single-layer image such as `alpine`;
duplicating the layer gives `ELOOP` "conflicting lowerdir path" · N3 `workdir` inside `upperdir`,
or equal to it, is `EINVAL` (enforced) · N4 a 500-layer `lowerdir+` mount renders a **15 KB single
line** in `/proc/mounts`, as repeated `lowerdir+=` keys that no `lowerdir=`-seeking parser will
find · N5 **mount exclusivity must be enforced in our own code** (V4-5.3) · N6 the **xattr
namespace a layer was extracted with** belongs on §11.5's "expensive to change" list.

---
## V4-11 — Host cleanliness attestation

Verified after the final experiment:

```
/tmp/v4*                          -> No such file or directory   (all scratch removed)
mounts under /tmp                 -> only the system tmpfs
overlay mounts in /proc/mounts    -> 5, all under /var/lib/docker/   (0 foreign)
/var/lib/docker/overlay2 entries  -> 107   (unchanged from session start)
docker ps                         -> aikserver-{web,nginx,frontend,certbot,redis} all Up
uptime                            -> 189 days, load 0.02
```

All experiments ran under `unshare -m --propagation private`; nothing under `/var/lib/docker` was
written, no `docker` mutating subcommand was issued, and nothing touched :80/:443.

**One note on the previous agent's standard.** `/tmp` on this host still contains six files left by
the earlier D3 session: `ovl611.rst`, `ovlparams611.c`, `ovlsuper611.c`, `ubparams.c`,
`v3_accept_test.py`, `e.3088863` (all dated Sep 4). They are harmless and I deliberately did not
delete files I did not create on a production host — but "left the host clean" is not quite
accurate, and I flag it so the next session does not inherit a false baseline. I also did **not**
use those files as evidence: every kernel-source fact above was fetched fresh from
`raw.githubusercontent.com/torvalds/linux` at an explicit tag.
