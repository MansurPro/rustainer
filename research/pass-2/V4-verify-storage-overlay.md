# V4 — adversarial verification: layered storage, OverlayFS, tar extraction

Scope: `D3-images-storage.md` **§4–§11, §13–§15**. (§0–§3 and §12 are V3's; D5 is V6's —
I read only D5 §7 and §17, for the cross-file kernel-floor dispute.)

Verifier: V4. Date: **2026-10-23**. Research under test written **2026-09-04**.

---

## Method and evidence base

**Primary sources read (pinned, recoverable):**

| What | Pinned reference |
|---|---|
| `copy_mount_options()` / `path_mount()` | `torvalds/linux@v6.11/fs/namespace.c#L3669-L3701`, `#L3736` |
| `OVL_MAX_STACK` | `torvalds/linux@v6.11/fs/overlayfs/params.h#L20` |
| overlayfs mount parameters | `torvalds/linux@v6.11/fs/overlayfs/params.c#L143-L145`, `#L305-L326`, `#L466-L471` |
| `vfs_mknod()` whiteout exemption | `torvalds/linux@v6.11/fs/namei.c#L4063-L4093` |
| `fsconfig(2)` value copy | `torvalds/linux@v6.11/fs/fsopen.c#L417`, `#L430` |
| legacy fs_context shim | `torvalds/linux@v6.11/fs/fs_context.c#L576-L629` |
| overlayfs documentation | `torvalds/linux@v6.11/Documentation/filesystems/overlayfs.rst`, and the same file at `v6.7`/`v6.8` for the version claim |
| `lowerdir+` commit | `torvalds/linux@24e16e385f22` "ovl: add support for appending lowerdirs one by one" (2023-10-30), merged `7f851936a0ca` (tag `ovl-update-6.7`) |

**Empirical work — `host-a` (`HOST-A`, Ubuntu 24.10, kernel 6.11.0-29-generic,
util-linux/libmount 2.40.2).** Everything ran **inside
`unshare --user --map-root-user --mount --propagation private`**, rooted under `/tmp`
(tmpfs) and `/var/tmp` (ext4), with three exceptions that created and deleted files only:
a `setpriv --reuid=65534` `mknod` test, reads of `/proc/mounts` and
`/sys/module/overlay/parameters/`, and `stat -f`. Nothing under `/var/lib/docker`, `/etc`,
`/sys/fs/cgroup` or any unit file was written. No container or image was created, started,
stopped or removed. **All scratch directories were removed and their absence verified**
(`ls -d /tmp/v4p /tmp/v4m /tmp/v4q /tmp/v4s /tmp/v4b.* /var/tmp/v4b.*` → "No such file or
directory" after each run).

Three probes, all raw-syscall (`ctypes` on `libc.syscall`), because `mount(8)` cannot answer
the question being asked:
1. `v4probe.py` — `mount(2)` option-string length sweep; silent-truncation boundary probe;
   `lowerdir+=` through the *classic* options string; genuine
   `fsopen`/`fsconfig`/`fsmount`/`move_mount`; `OVL_MAX_STACK` search; colon-in-path; `mknod`.
2. `v4probe2.sh` — truly unprivileged `mknod` (uid 65534, `CapEff=0`, **initial** userns) on
   tmpfs and ext4; `mount(8)` with `-o lowerdir+=` under all three `LIBMOUNT_FORCE_MOUNT2`
   modes with `strace`; `userxattr` auto-enable; overlay module params; live Docker mount flags.
3. `v4probe3.py` — `fsconfig(2)` string-value length caps.

**macOS local machine**: no kernel claim verified locally; source reading and web only.

**`old-host` (Rocky 8.8 / 4.18): not used.** Every 4.18 claim I touch is marked
UNVERIFIABLE-HERE with the exact command, or adjudicated from source. (Time budget went to
the §8.3/§9.2 mount-API question, which was the brief's stated priority.)

---

## Part 1 — The mount-option page limit and whether `fsconfig` escapes it — **the resumed finding, now settled**

This is the question the previous attempt was killed mid-sentence on. It is settled in full.

### 1.1 The 4095-byte `mount(2)` option limit — **CONFIRMED**, and the mechanism is now cited

D3 §8.3 `[F]` HIGH: *"`copy_mount_options()` copies at most one page and NUL-terminates, so
the total option string is capped at 4095 bytes and — critically — truncation is silent."*

**Verdict: CONFIRMED.** The claim is right; the attribution is one function off, which is
worth fixing because it is the line a future reader will go looking for.

`copy_mount_options()` (`torvalds/linux@v6.11/fs/namespace.c#L3669`) does **not** itself
NUL-terminate. It `kmalloc(PAGE_SIZE)`s, `copy_from_user(copy, data, PAGE_SIZE)`s, falls back
to a byte loop, and returns `-EFAULT` only if *nothing* was copyable. The truncation is one
frame up, in `path_mount()` (`#L3736`):

```c
	/* Basic sanity checks */
	if (data_page)
		((char *)data_page)[PAGE_SIZE - 1] = 0;
```

One unconditional store of `'\0'` at byte 4095. No length is checked, nothing is compared,
no error is produced. **Bytes 0–4094 survive; everything from byte 4095 on is gone, silently.**

`[O]` **host-a 6.11.0-29, userns+mntns, HIGH** — measured by building a `lowerdir=` string
of an exact byte length and mounting:

```
  valid lowerdir string of  4093 B -> OK
  valid lowerdir string of  4094 B -> OK
  valid lowerdir string of  4095 B -> OK
  valid lowerdir string of  4096 B -> ENOENT
  valid lowerdir string of  4200 B -> ENOENT
```

The boundary is exactly 4095. Not `E2BIG`, not `EINVAL`, not `ENAMETOOLONG` — **`ENOENT`**,
because the last path in the list was cut in half and the kernel then looked up a directory
that does not exist. D3's "you get a mount failure about a path that does not exist" is
exactly right, and D3's ROCKY observation ("special device overlay does not exist") is the
same errno rendered by an older `mount(8)`.

### 1.2 The truncation is *silent*, proven directly — **CONFIRMED**

An errno at the boundary is weak evidence for silence; a path can be invalid for many
reasons. The decisive test appends a **deliberately invalid option** after N valid bytes. If
the kernel sees it, the mount fails `EINVAL`; if the mount *succeeds*, the tail was discarded
without complaint.

`[O]` **host-a 6.11.0-29 HIGH**:

```
  valid  3000 B + ',ZZZZZZZZ'  -> EINVAL            (bogus option reached the parser)
  valid  4086 B + ',ZZZZZZZZ'  -> EINVAL            (ends exactly at 4095; still fully seen)
  valid  4090 B + ',ZZZZZZZZ'  -> EINVAL            (",ZZZ" survives inside the window)
  valid  4094 B + ',ZZZZZZZZ'  -> MOUNTED           (only the ',' fits; tail gone)
  valid  4095 B + ',ZZZZZZZZ'  -> MOUNTED           (entire tail discarded, no error)
  valid  4096 B + ',ZZZZZZZZ'  -> ENOENT            (truncation landed mid-path)
```

A `mount(2)` that was handed 4104 bytes of options, of which 9 bytes were garbage,
**returned 0**. That is silent truncation, demonstrated rather than inferred.
D3's `[R]` HIGH "compute the option length and refuse before calling `mount(2)`" is
**vindicated and should be treated as mandatory, not advisory**.

### 1.3 `OVL_MAX_STACK = 500` — **CORRECTED upward in confidence, from `[C]` LOW to `[F]` HIGH**

D3 §8.3 and open question §15.2: *"the value is widely reported as 500, but I did not locate
the `#define` … LOW confidence on the number."*

It is in `fs/overlayfs/params.h`, not `ovl_entry.h` or `overlayfs.h` — which is why D3 missed it:

```c
/* torvalds/linux@v6.11/fs/overlayfs/params.h#L20 */
#define OVL_MAX_STACK 500
```

`[O]` **host-a HIGH** — and the kernel says so itself. Driving it past the limit through
`fsconfig` and then `read(2)`ing the `fs_context` fd (the new mount API returns real error
strings, which is a reason to prefer it independent of the length question):

```
  480 lowers -> OK
  500 lowers -> OK
  501 lowers -> fsconfig lowerdir+ #501 EINVAL  ctx:['e overlay: too many lower directories, limit is 500']
```

**D3 §15 open question 2 is closed.** `[F]` HIGH, 500, and it is a per-`lowerdir+`-call check
(`params.c#L305-L307`) as well as a post-split check on `lowerdir=` (`#L466-L471`).

### 1.4 The overlay2 arithmetic — **CONFIRMED, and it reconstructs the real constant exactly**

D3 §9.2: `(26 + 1 + 1) * 128 = 3584` and `4096 - 512 = 3584`, from moby's own comment in
`daemon/graphdriver/overlay2/overlay.go`.

The arithmetic is correct and the quoted comment matches moby's source. Two refinements:

- The buffer is reserved against **4096**, not 4095. moby budgets against `pageSize`, the
  kernel gives 4095 usable. moby's 512-byte SELinux-label reserve absorbs the off-by-one many
  times over, so the derivation stands.
- `(idLength + len(linkDir) + 1)` = `26 + 1 + 1` = 28 is **`len("l/XXXXXXXXXXXXXXXXXXXXXXXXXX") + 1`**
  — 26 random chars, `len("l")`, one `/`… and the `+1` is the `:` separator. D3 reads the `+1`
  as the separator and the other `+1` as the slash; either reading gives 28. Confirmed.
- D3's "94 bytes/layer would cap you at ~43 layers" checks out: `4095/94 = 43.5`.

So overlay2's two magic numbers (26 and 128) really are a joint solution to the 4095-byte
wall. **CONFIRMED.**

### 1.5 The decisive question: does the new mount API escape it? — **YES for genuine `fsconfig(2)`, NO for `mount(8)`/`mount(2)`**

This is the finding the brief asked for, and the two cases diverge completely.

**Case A — `mount(8)` / `mount(2)` with `-o lowerdir+=…`: the limit still applies.**

`[O]` **host-a 6.11.0-29 HIGH** — raw `mount(2)` with the options string built from
repeated `lowerdir+=` pairs:

```
   60 x lowerdir+= ( 1559 B) -> OK   (60 layers visible in /proc/self/mounts)
  150 x lowerdir+= ( 3899 B) -> OK   (150 layers visible)
  300 x lowerdir+= ( 7799 B) -> ENOENT
```

`[O]` **host-a HIGH** — and `mount(8)` behaves identically, because **util-linux 2.40.2
does not use the new mount API for this at all.** `strace -e trace=mount,fsopen,fsconfig,fsmount,move_mount,open_tree`
over `mount -t overlay -o 'lowerdir+=…'`, under every value of `LIBMOUNT_FORCE_MOUNT2`:

```
mode=auto   mounted=yes  syscalls: 1 mount(
mode=never  mounted=yes  syscalls: 1 mount(
mode=always mounted=yes  syscalls: 1 mount(
```

One `mount(2)`, zero `fsopen`. And at 300 layers (7389-byte option string) all three modes
fail with the same `wrong fs type, bad option, bad superblock`.

**This settles the prior verifier's cliffhanger: `-o lowerdir+=` buys you the
colon-escaping and the "append" semantics, and buys you *nothing at all* against the page
limit.** `lowerdir+=` in a `mount(8)` command line is syntactic sugar over the same single
4095-byte page. Any benchmark, blog post or design note that claims "use `lowerdir+=` to get
past the mount option limit" is wrong unless it means raw `fsconfig(2)`.

**Case B — genuine `fsopen` + repeated `fsconfig(FSCONFIG_SET_STRING, "lowerdir+", …)`: the
limit is gone.**

`[O]` **host-a 6.11.0-29 HIGH** — `fsopen("overlay")`, N × `fsconfig(…, "lowerdir+", <one path>)`,
`fsconfig(…, FSCONFIG_CMD_CREATE)`, `fsmount`, `move_mount(…, MOVE_MOUNT_F_EMPTY_PATH)`, then
`readdir` of the result:

```
    2 lowerdir+ calls (=     32 B of paths) -> OK   files visible through mount: 2
  128 lowerdir+ calls (=   2048 B of paths) -> OK   files visible through mount: 128
  200 lowerdir+ calls (=   3200 B of paths) -> OK   files visible through mount: 200
  300 lowerdir+ calls (=   4800 B of paths) -> OK   files visible through mount: 300
  300 lowers + upperdir + workdir           -> OK   files visible: 300   (read-write overlay)
```

**300 layers, 4800 bytes of lower paths, every single layer's file readable through the
mount, read-write with upper and work.** The 4095-byte page does not exist on this path.
D3 §8.3's and §9.2's central `[R]` HIGH — *"we do not need the short-link hack, because we
should use `fsopen`/`fsconfig` with `lowerdir+`"* — is **CONFIRMED by direct measurement.**

`[O]` **host-a HIGH** — and the second claimed benefit holds too. A lower directory
literally named `has:colon`:

```
  fsconfig lowerdir+ with ':' in path -> MOUNTED, marker visible: True
  classic mount(2) lowerdir= with ':' -> ENOENT
```

### 1.6 **What D3 missed: `fsconfig` does not remove the limit, it replaces it with two narrower ones**

This is the correction that matters for implementation, and it is new.

**(a) Each `fsconfig(FSCONFIG_SET_STRING)` value is capped at 255 bytes.**
`torvalds/linux@v6.11/fs/fsopen.c#L430`: `param.string = strndup_user(_value, 256);`
(and `#L417` the same for the key).

`[O]` **host-a 6.11.0-29 HIGH** — one lower directory whose absolute path is exactly N bytes:

```
  len= 200 -> accepted      len= 254 -> accepted
  len= 250 -> accepted      len= 255 -> accepted
                            len= 256 -> EINVAL
```

`PATH_MAX` is 4096; `fsconfig` gives you **255**. A store path of
`<root>/layers/userxattr/<64-hex>/diff` is ~90 bytes plus the root, so a root deeper than
~160 bytes breaks every mount, with an `EINVAL` that names nothing.

**(b) There is no escape hatch.** `FSCONFIG_SET_PATH` (which uses `getname_flags`, i.e.
`PATH_MAX`) is rejected by overlayfs:

```
  FSCONFIG_SET_PATH lowerdir+ -> EINVAL  ["e overlay: Bad value for 'lowerdir+'"]
```

`lowerdir+` is declared `fsparam_string` (`params.c#L144`), and `fs_param_is_string` accepts
only `fs_value_is_string`. So 255 bytes per lower path is a hard ceiling.

**(c) The *aggregate* `lowerdir=` form is far worse under `fsconfig` than under `mount(2)`.**

```
  fsconfig lowerdir= string of  43 B -> accepted
  fsconfig lowerdir= string of 439 B -> EINVAL
```

Same 255-byte cap. So on the new API you get 4095 bytes → 255 bytes if you keep the old
colon-joined style. **`lowerdir+` is not an optimisation on the new API; it is the only
usable form.** An implementation that "ports to fsconfig" by moving its existing
colon-joined string into one `FSCONFIG_SET_STRING` call makes things dramatically worse and
will pass its two-layer test.

**(d) The honest summary of the three ceilings:**

| path | per-path limit | aggregate limit | layer ceiling | failure mode |
|---|---|---|---|---|
| `mount(2)`, `lowerdir=a:b:c` | `PATH_MAX` (4096) | **4095 B total options** | ~43 (Docker's raw paths) … ~145 (28 B short links) | **silent truncation → `ENOENT`** |
| `mount(2)`, `-o lowerdir+=` repeated | `PATH_MAX` | **4095 B total options** | same | **silent truncation → `ENOENT`** |
| `fsconfig`, `lowerdir` (aggregate) | **255 B for the whole string** | 255 B | ~3 | loud `EINVAL` |
| **`fsconfig`, `lowerdir+` repeated** | **255 B per path** | none | **`OVL_MAX_STACK` = 500** | loud `EINVAL` + a readable message on the context fd |

### 1.7 Kernel floors — **one CORRECTION to a `[F]` HIGH**

| feature | D3 says | actually | source |
|---|---|---|---|
| `fsopen`/`fsconfig`/`fsmount`/`move_mount`/`open_tree` | (D5 §7.1 table) 5.2 | **5.2** — CONFIRMED | David Howells' mount-API series, v5.2-rc1 |
| overlayfs converted to the new mount API (`fs_context`) | *not mentioned* | **6.5** — `fs/overlayfs/super.c` first references `init_fs_context` at `v6.5` (0 at `v6.4`) | measured across tags |
| `lowerdir+` / `datadir+` | **"v6.8"** (quoting `overlayfs.rst`) | **v6.7** | code present in `torvalds/linux@v6.7/fs/overlayfs/params.c#L148`; commit `24e16e385f22` (2023-10-30) merged in `ovl-update-6.7` |

**CORRECTED — `lowerdir+` is a 6.7 feature, not a 6.8 feature.** D3 quoted
`overlayfs.rst` faithfully; **the kernel's own documentation is wrong**. The sentence "Since
kernel version v6.8, directory names containing colons can also be configured … using the
`lowerdir+` mount options" is already present *in the v6.7 tree itself*, alongside the
working code — it was written while the series was still aimed at the 6.8 merge window and
was never corrected when it landed a cycle early. Verified by diffing `params.c` across
`v6.6` (0 occurrences of `lowerdir+`) / `v6.7` (5) / `v6.8` (5).

**The 6.5 line is the one D3 actually needed and does not have.** Below 6.5, `fsopen("overlay")`
does not fail — it silently falls back to the **legacy fs_context shim**, which reassembles
your `fsconfig` calls into exactly one comma-separated options page
(`fs/fs_context.c#L576` `legacy_parse_param`), enforcing
`if (size + len + 2 > PAGE_SIZE) return invalf(fc, "VFS: Legacy: Cumulative options too large");`.
So on 5.2–6.4 the new mount API gives you the **same ~4094-byte ceiling** — but, to its
credit, a loud error instead of truncation. On 6.5–6.6 you get native overlay parsing but no
`lowerdir+`, so you are still passing one colon-joined string and still capped — now at
**255 bytes** (§1.6c), which is *worse* than `mount(2)`.

**Practical floor for "mount an image with an arbitrary number of layers without a symlink
farm": Linux 6.7.** Not 6.8, and nothing earlier is usable for it.

### 1.8 **Verdict on the question the brief said to answer explicitly**

> *Does this decide whether our engine can mount an image with an arbitrary number of layers
> without inventing overlay2's symlink-farm workaround?*

**Yes, on kernel ≥ 6.7, and no below it.**

- **≥ 6.7**: `fsopen` + N × `fsconfig(FSCONFIG_SET_STRING, "lowerdir+", path)` + `fsmount` +
  `move_mount` mounts **up to `OVL_MAX_STACK` = 500 layers** with no aggregate length limit —
  measured at 300 layers / 4800 bytes of paths, read-write, on 6.11. The only constraint is
  **255 bytes per lower path**, which our store layout controls. The symlink farm is
  unnecessary. D3 §9.2's `[R]` HIGH stands, with the 500-layer ceiling and the 255-byte
  per-path rule added.
- **< 6.7** (which includes D3's and D5's own 5.10/5.15 floor discussion, and RHEL 8's 4.18):
  there is no escape. Every route — `mount(2)` with `lowerdir=`, `mount(2)` with
  `lowerdir+=`, `mount(8)` in any mode, `fsconfig` via the legacy shim — passes through one
  4095-byte page. D3's fallback `[R]` MED (relative paths + `chdir` into the store root,
  moby's `mountFrom` trick) is therefore **not optional on the stated 5.15 floor — it is the
  only path that works there**, and it is what makes 500-ish layers reachable without a
  symlink farm: `chdir(<store root>)` then `lowerdir=<chainid>/diff:…` costs 70 bytes/layer
  → ~58 layers, or 21 bytes/layer with a 16-hex truncated name → ~195 layers.

**`[R]` for the specification session — three rules, in order:**
1. **Probe, do not version-check** (and this is D5 §7.2's rule, which D3 does not restate):
   try `fsopen("overlay")` + one `fsconfig(…,"lowerdir+",…)` at startup and classify. `EINVAL`
   on `lowerdir+` means pre-6.7 *or* the legacy shim; both mean "use the `mount(2)` path".
2. On the `mount(2)` path, **compute the exact option byte count and refuse above 4095 with a
   message naming the layer count**, before the syscall. §1.2 shows what happens otherwise.
3. On the `fsconfig` path, **validate every lower path at ≤ 255 bytes** and refuse above 500
   layers, both before the first syscall. Both ceilings are loud when hit, but the message is
   `EINVAL` on an unnamed path unless you read the `fs_context` fd — so **always `read(2)` the
   context fd on failure and surface the string**; it is the single best error-reporting
   reason to prefer the new API, independent of length.

---

## Part 2 — §8 OverlayFS mechanics against the kernel's own documentation and source

All kernel-doc quotes below are from `torvalds/linux@v6.11/Documentation/filesystems/overlayfs.rst`
and were re-read line by line; all empirical results are `[O]` **host-a 6.11.0-29**, in a
private user+mount namespace under `/tmp`.

### 2.1 `lowerdir` ordering: leftmost wins — **CONFIRMED**

Doc, `overlayfs.rst#L347-349`, verbatim: *"The specified lower directories will be stacked
beginning from the rightmost one and going left. In the above example lower1 will be the
top, lower2 the middle and lower3 the bottom layer."* D3 §8.2 quotes this correctly.

`[O]` **host-a 6.11.0-29 HIGH** — re-measured independently of D3:

```
  lowerdir=oa:ob  f = FIRST     (oa/f contained "FIRST")
  lowerdir=ob:oa  f = SECOND
```

**CONFIRMED.** D3 §8.2's `[R]` HIGH ("build the option string by reversing the manifest layer
order") stands. The same ordering rule applies to repeated `lowerdir+` under `fsconfig`: the
first `fsconfig` call is the topmost layer — `[O]` verified in Part 1 by reading all 300
layers' files through the mount in the right order.

### 2.2 `upperdir` / `workdir` constraints — **CORRECTED in two places, both tighter than D3 says**

D3 §8.1 `[F]` HIGH: *"`workdir=` — an empty scratch directory 'on the same filesystem as
upperdir'"*, and §8.6 `[R]` HIGH: *"require upperdir and workdir on the same filesystem and
check it ourselves (compare `statfs.f_fsid`/`st_dev`)"*.

**CORRECTION 1 — the kernel requires the same *mount*, not the same filesystem.**
`torvalds/linux@v6.11/fs/overlayfs/super.c#L789-793`:

```c
	if (upperpath->mnt != workpath->mnt) {
		pr_err("workdir and upperdir must reside under the same mount\n");
		return err;
	}
```

`[O]` **host-a HIGH** — demonstrated: upperdir at `/tmp/v4r/u`, workdir at `/tmp/v4r/w`
— same tmpfs, same inode device, **different `vfsmount` via a `mount --bind`** — **FAILS**.

```
  same fs but DIFFERENT MOUNT (bind) upper vs work  -> FAIL
```

**So D3's recommended check is wrong and will pass a configuration the kernel rejects.**
`st_dev` and `statfs.f_fsid` are identical across a bind mount. The correct check is the
mount id: compare the `mnt_id` from `statx(STATX_MNT_ID)` (Linux 5.8+), or the mount-id field
of `/proc/self/mountinfo` for each path. Cheap, and it is what the kernel actually tests.

**CORRECTION 2 — "workdir must be empty" is documentation, not enforcement.**
`overlayfs.rst#L126`: *"The 'workdir' needs to be an empty directory on the same filesystem
as upperdir."* `[O]` **host-a HIGH** — a workdir pre-populated with two files and a
directory **mounts fine**; overlay simply creates its own `work/` subdirectory alongside the
junk:

```
  workdir NOT EMPTY (3 pre-existing entries)  -> MOUNTED
    workdir contents afterwards: d junk junk2 work
```

D3 states the emptiness rule as a `[F]`. It is a doc-level SHOULD; the kernel does not check
it. Practical consequence for us: **do not rely on `mount(2)` to catch a reused workdir** —
a stale `work/` from a crashed run is exactly the state we will meet, and see §2.5 for why
that is not harmless.

**CONFIRMED — workdir must not be a subdirectory of upperdir or vice versa.**
`super.c#L428-440` (`/* Workdir should not be subdir of upperdir and vice versa */`,
implemented with `lock_rename()` returning a trap dentry) and `#L794-797`
`pr_err("workdir and upperdir must be separate subtrees\n")`.
`[O]` all four variants rejected:

```
  workdir INSIDE upperdir          -> FAIL
  upperdir INSIDE workdir          -> FAIL
  workdir == upperdir              -> FAIL
  (and the error is the useless "wrong fs type, bad option, bad superblock")
```

D3 §8.6's "check it ourselves rather than letting `mount(2)` fail, because the kernel's error
is uninformative" is **CONFIRMED and strengthened**: all five distinct upper/work misuses
produce the *same* opaque `EINVAL`, and the real reason is only in `dmesg`.

**CONFIRMED — the same-filesystem rationale.** D3 §8.1's explanation (copy-up and whiteout
creation are made atomic by creating in `work/work/` and `rename`ing into place; `rename` is
only atomic within a filesystem) matches `super.c`'s `ovl_create_temp` /
`ovl_cleanup_and_whiteout` flow and the doc. No correction.

### 2.3 Which filesystems may be upper — **CONFIRMED (doc), overlay-on-overlay measured**

`overlayfs.rst#L98-106`, verbatim: *"A wide range of filesystems supported by Linux can be
the lower filesystem… The lower filesystem can even be another overlayfs. The upper
filesystem will normally be writable and if it is it must support the creation of trusted.\*
and/or user.\* extended attributes, and must provide valid d_type in readdir responses, so
**NFS is not suitable**."* And `#L106-107`: *"A read-only overlay of two read-only
filesystems may use any filesystem type."*

D3 §8.6 quotes this correctly. **CONFIRMED.**

`[O]` **host-a 6.11.0-29 HIGH** — the overlay-on-overlay asymmetry, which D3 asserts
("overlay-on-overlay needs care") without measuring:

```
  overlay as LOWER of another overlay  -> MOUNTED
  overlay as UPPER of another overlay  -> FAIL
```

So the doc's "the lower filesystem can even be another overlayfs" is a real, one-directional
permission. Note the nested-lower case is also what `…overlay.opaque="x"` (the "whiteout
base" marker D3 §5.2 quotes) exists for.

**UNVERIFIABLE-HERE** — D3's `[O]` ROCKY claim that an NFS `upperdir` returns **`EACCES`**
(not `EINVAL`/`EXDEV`). No NFS mount exists on `host-a`, and I will not create one on a
production host. Settle with: `ssh old-host` and
`strace -e trace=mount mount -t overlay overlay -o lowerdir=/tmp/l,upperdir=$HOME/u,workdir=$HOME/w /tmp/m`
(`$HOME` is NFS there). D3's own trace output is credible and I found nothing contradicting it.

### 2.4 Duplicate and overlapping lower directories — **CONFIRMED, and broader than D3 says**

D3 §8.6 `[O]`: duplicate lowerdirs give `ELOOP`.

`[O]` **host-a 6.11.0-29 HIGH** — reproduced on a second kernel, and the rule covers
three distinct cases, all `ELOOP`:

```
  duplicate lowerdir (oa:oa)            -> ELOOP "Too many levels of symbolic links"
  lowerdir == upperdir                  -> ELOOP
  nested lowerdirs (oa:oa/nested)       -> ELOOP
```

Mechanism: `ovl_setup_trap()` (`super.c#L442`) installs a "trap" inode on every layer root,
and a second use of the same dentry hits it. The doc states the policy at `#L490-499`:
*"Lower layers may be shared among several overlay mounts … Using an upper layer path and/or
a workdir path that are already used by another overlay mount is not allowed and may fail
with EBUSY. Using partially overlapping paths is not allowed."*

**Strengthens D3 §8.6's `[R]` MED to `[R]` HIGH**: dedupe the lower list, *and* reject any
lower that is an ancestor or descendant of another lower or of the upper. With chain-id-named
directories under one store root this cannot happen by accident — but it *can* happen
deliberately, from a crafted image whose two layers produce the same `diff_id` (the "empty
layer" case D3 itself names), so it must be a check, not an assumption.

### 2.5 Feature options — **CORRECTED: four of them are unusable in exactly the mode we want**

D3 §8.5 `[O]` ROCKY HIGH: *"all of `metacopy=on`, `redirect_dir=on`, `index=on`, `xino=on`,
`volatile`, `nfs_export=on`, `userxattr` are accepted by the 4.18 RHEL kernel. So option
acceptance is not a useful kernel-version probe."*

That observation is from a **root, initial-userns** mount. It does not transfer to the
rootless mode D3's own `[R]` HIGH mandates. `[O]` **host-a 6.11.0-29 HIGH** — the same
sweep *inside a user namespace* (where 6.11 auto-selects `userxattr`), fresh upper+work per test:

| option | result in a userns overlay on 6.11 |
|---|---|
| `userxattr` | **MOUNTED** — `…,redirect_dir=nofollow,uuid=on,userxattr` |
| `nouserxattr` | **MOUNTED** — `…,redirect_dir=nofollow,uuid=null,nouserxattr` |
| `metacopy=on` | **REJECTED** |
| `redirect_dir=on` | **REJECTED** |
| `redirect_dir=off` | **REJECTED** |
| `redirect_dir=nofollow` | MOUNTED (this is the forced value) |
| `index=on` / `index=off` | MOUNTED |
| `xino=on` / `xino=off` | MOUNTED |
| `nfs_export=on` | MOUNTED |
| `volatile` | MOUNTED (see the poisoning finding below) |
| `verity=require` | REJECTED (lower is tmpfs; no fs-verity) |
| `index=off,nfs_export=on` | REJECTED (documented conflict) |
| `metacopy=on,nfs_export=on` | REJECTED |
| `metacopy=on,redirect_dir=nofollow` | REJECTED |

The mechanism is explicit in `torvalds/linux@v6.11/fs/overlayfs/params.c#L886-902`:

```c
	/* Resolve userxattr -> !redirect && !metacopy && !verity dependency */
	if (config->userxattr) {
		if (set.redirect && config->redirect_mode != OVL_REDIRECT_NOFOLLOW) {
			pr_err("conflicting options: userxattr,redirect_dir=%s\n", …);
			return -EINVAL;
		}
		if (config->metacopy && set.metacopy) {
			pr_err("conflicting options: userxattr,metacopy=on\n");
			return -EINVAL;
		}
		if (config->verity_mode) { … return -EINVAL; }
	}
```

**This is a useful finding for us and it is not in D3.** `metacopy`, `redirect_dir` and
`verity` are *structurally* unavailable in rootless mode, not merely inadvisable. D3 §15 open
question 8 ("whether `metacopy=on` is safe for us and how much it saves") is therefore **half
closed**: in rootless mode the question does not arise, the kernel refuses. It remains open
for root mode only. And note `redirect_dir=off` is rejected too — under `userxattr` the only
accepted explicit value is the one the kernel already chose.

**CONFIRMED and sharpened — the `metacopy` security warning.** `overlayfs.rst#L382-387`,
verbatim: *"Do not use metacopy=on with untrusted upper/lower directories. Otherwise it is
possible that an attacker can create a handcrafted file with appropriate REDIRECT and
METACOPY xattrs, and gain access to file on lower pointed by REDIRECT. This should not be
possible on local system as setting 'trusted.' xattrs will require CAP_SYS_ADMIN. But it
should be possible for untrusted layers like from a pen drive."*

Read the second half carefully, because it changes the answer to D3 §15's question 8:
**the doc's own reasoning for why this is usually safe is "a layer cannot set
`trusted.overlay.*` without CAP_SYS_ADMIN" — and that reasoning collapses precisely under
`userxattr`,** where the overlay reads `user.overlay.*`, which *any* unprivileged process,
and therefore any extracted layer, can set. That is exactly why the kernel hard-rejects the
combination above. Our threat model (attacker-supplied image content in the lower layers) is
the pen-drive case, not the local-filesystem case.

**`[R]` HIGH (strengthening D3's `[R]` HIGH "pass none of these"):** the unsafe-with-untrusted-
lowers set is **`metacopy=on`, `redirect_dir=on`, `redirect_dir=follow`**, and the reason is
a layer that ships its own `overlay.redirect` / `overlay.metacopy` xattr. §8.4's `[R]`
("never let a layer set an `overlay.*` xattr directly") is the matching extractor-side rule
and is **CONFIRMED as load-bearing, not hygiene**. `index`, `xino` and `nfs_export` are
correctness/compat options with no equivalent lower-layer attack surface.

**NEW `[O]` — `volatile` permanently poisons its workdir.** `overlayfs.rst#L765` describes
the marker file; the operational consequence is not stated in D3, which recommends `volatile`
for the extraction staging mount (§8.5 `[R]` MED).

```
  volatile mount                                  -> MOUNTED
  workdir contents: work/incompat/volatile/dirty
  re-mount NON-volatile, same workdir             -> FAIL (workdir poisoned)
```

Combined with §2.2's finding that a non-empty workdir is accepted, this is a real foot-gun:
after any `volatile` mount, every later ordinary mount on that workdir fails with the same
opaque `EINVAL`, forever, until `work/incompat/` is removed. **`[R]` HIGH — if we use
`volatile` at all, use a fresh workdir every time and `rm -rf` it afterwards; never reuse a
workdir across modes.** D3's `[R]` MED is fine in intent but needs this rule attached.

### 2.6 `userxattr` — **CONFIRMED, including the 6.11 auto-enable, with one clarification**

D3 §5.5 `[O]` HOST-A HIGH: mounting overlay inside `unshare -Urm` *without* passing
`userxattr` yields a mount that reports `userxattr` and writes `user.overlay.*`.

`[O]` **host-a 6.11.0-29 HIGH — independently reproduced:**

```
overlay /tmp/v4m/x/mrg overlay rw,relatime,lowerdir=…,upperdir=…,workdir=…,
        redirect_dir=nofollow,uuid=on,userxattr 0 0
    rm -rf of a lower dir through the mount: OK
    upperdir afterwards:  c--------- 2 root root 0, 0 …  d      <-- char 0:0 whiteout
```

**CONFIRMED on all three counts:** `userxattr` is auto-selected, the xattr namespace switches
to `user.overlay.*`, and deleting a lower-only directory through the mount materialises a
**character device 0:0** in the upper — which independently confirms §5.2's whiteout
representation (see Part 3).

**Clarification D3 does not have: the auto-enable is a default, not a forcing.** `[O]` —
`nouserxattr` is accepted inside the same user namespace and the mount then reports
`uuid=null,nouserxattr`. So the kernel will happily give a rootless caller a
`trusted.overlay.*` overlay that cannot actually write its own xattrs. D3's `[R]` HIGH
("always pass `userxattr` explicitly when not CAP_SYS_ADMIN-in-init-userns, never when we
are") is **CONFIRMED as necessary** — the default is only a default, and `nouserxattr` is a
reachable, silently-broken configuration.

`[O]` **host-a HIGH** — the converse, read-only from `/proc/mounts`: live Docker container
overlays on this host report **`nouserxattr`**, confirming D3 §9.1 and the root-mode
`trusted.overlay.*` half of the partitioning argument. Host module defaults, read-only from
`/sys/module/overlay/parameters/`: `metacopy=N index=N nfs_export=N redirect_dir=N
redirect_always_follow=Y redirect_max=256 xino_auto=Y check_copy_up=N` — matching D3 §8.5's
`[O]` exactly.

### 2.7 **NEW CORRECTION — a read-only overlay needs TWO lower layers**

D3 §8.1 `[F]` HIGH: *"A read-only overlay can be mounted with `lowerdir=` alone and no
upper/work."* D3's supporting `[O]` used **two** lower dirs (`lowerdir=$B/lo2:$B/lo`), so the
single-layer case was never tested.

`[O]` **host-a 6.11.0-29 HIGH**:

```
  single lowerdir, no upper -> FAIL
  two lowerdirs, no upper   -> MOUNTED  ro,relatime,lowerdir=…/oa:…/ob,redirect_dir=nofollow,userxattr
```

`torvalds/linux@v6.11/fs/overlayfs/super.c#L1123-1126`:

```c
	if (!ofs->config.upperdir && ctx->nr == 1) {
		pr_err("at least 2 lowerdir are needed while upperdir nonexistent\n");
		return ERR_PTR(-EINVAL);
	}
```

**This bites us on the very first image we will test.** `alpine` has exactly **one** layer
(D3 §4.1's own `[O]`). A `--read-only` container on a single-layer image, mounted the obvious
way, fails with the generic "wrong fs type, bad option, bad superblock". Also note the
`[O]` above shows a no-upper mount reports `ro` — D3's §8.1 `[O]` ROCKY claim about that is
reproduced.

**`[R]` HIGH — for a read-only container, either (a) always supply an `upperdir`+`workdir` on
a `tmpfs` and mount the overlay `ro` at the VFS level (`MS_RDONLY` / `fsmount` with
`MOUNT_ATTR_RDONLY`), or (b) pad the lower list with one empty directory when the image has a
single layer.** (a) is better: it keeps one code path, gives a real place for copy-up to fail
loudly, and matches what the runtime spec's `readonly` rootfs means anyway.

### 2.8 Section-8 items checked and found correct, needing no correction

- `workdir` rationale (atomic rename within one filesystem) — correct.
- `lowerdir` list is colon-separated, `upperdir`/`workdir` omittable — correct.
- the `…overlay.*` xattr table in §8.4 — every entry matches
  `torvalds/linux@v6.11/fs/overlayfs/overlayfs.h`'s `OVL_XATTR_*` enumeration; `opaque="x"`
  as the overlay-on-overlay "whiteout base" is `overlayfs.rst`'s wording, correct.
- `nfs_export` requires `index=on` — correct and enforced (`params.c#L833-853`).
- `xino` folds the layer fsid into the high bits of `st_ino` and falls back silently on
  overflow — correct per the doc's `#L49` discussion.
- `volatile` skips sync/fsync and is not crash-survivable — correct, and §2.5 adds the
  poisoning consequence.

---

## Part 3 — §5 whiteouts: tar convention → kernel representation

Spec source: `opencontainers/image-spec@v1.1.1/layer.md`. **v1.1.1 (2025-03-03) is still the
latest release as of today** — no drift since D3 was written.

### 3.1 The tar-side convention IS normative — **CONFIRMED**, with one quote that does not exist

Verified verbatim against `image-spec@v1.1.1/layer.md`:

| D3 §5.1 quote | spec line | verdict |
|---|---|---|
| "A whiteout file is an empty file with a special filename that signifies a path should be deleted." | `#L247` | **CONFIRMED** |
| "A whiteout filename consists of the prefix `.wh.` plus the basename of the path to be deleted." | `#L248` | **CONFIRMED** |
| "Once a whiteout is applied, the whiteout itself MUST also be hidden." | `#L250` | **CONFIRMED** |
| "Whiteout files MUST only apply to resources in lower/parent layers." | `#L251` | **CONFIRMED** |
| "An opaque whiteout entry is a file with the name `.wh..wh..opq` indicating that all siblings are hidden in the lower layer" / "hides _all_ children of the `bin/`…" | `#L279`, `#L301` | **CONFIRMED** |
| "…applied first, before creating the new version of `a/b`, **regardless of the ordering**" | `#L336` ("…regardless of the ordering in which the whiteout file was encountered") | **CONFIRMED** |
| "Implementations SHOULD generate layers using _explicit whiteout_ files, but MUST accept both." | `#L315` | **CONFIRMED** |
| "it is not possible to create a filesystem which has a file or directory with a name beginning with `.wh.`" | `#L249` | **CONFIRMED** |
| **"A `.wh.` file, without a basename to delete, is invalid and implementations SHOULD return an error when encountering such an entry."** | — | **CORRECTED: this sentence is not in image-spec `layer.md` at v1.0.2, v1.1.0 or v1.1.1.** |

The missing quote does not change the recommendation — rejecting a bare `.wh.` entry is
obviously right — but it should be re-tagged from `[F]` (spec SHOULD) to `[R]` (our policy),
because a conformance argument cannot be built on it.

**Three spec rules D3 omits, all of which simplify §6:**
- `#L22` — *"Layer Changesets … **MUST NOT** include duplicate entries for file paths in the
  resulting tar archive."* D3 §6.10's table resolves duplicates with "apply in stream order".
  **The spec lets us reject them outright**, which is strictly safer: a duplicate entry is the
  shape of a validate-then-replace attack. `[R]` HIGH — track seen paths per layer and refuse
  on a duplicate, citing this MUST NOT.
- `#L62` — *"Sparse files SHOULD NOT be used because they lack consistent support across tar
  implementations."* D3 §6.10 says "support decoding". We may simply **reject** GNU/PAX
  sparse entries and be conformant. That deletes the single most error-prone decoder in the
  tar format (three incompatible `GNU.sparse.*` encodings) from our attack surface.
- `#L54`/`#L56` — *"User Name (`uname`) should be ignored on platforms that support User ID"*
  (and the same for `gname`). **CONFIRMS** D3 §6.6's "PAX `uname`/`gname` must be ignored" as
  a spec position, not just our policy.

One place where our planned behaviour is **deliberately non-conformant and D3 should say so**:
`layer.md#L40-46` lists `block devices` and `character devices` among the **valid file types**
in a layer. D3 §6.3's `[R]` HIGH ("refuse to create device nodes from layer content, always")
is therefore a *policy deviation from the spec*, not an implementation of it. The
recommendation is still right; the justification "no legitimate image needs a device node"
should be stated as our judgement, and the OCI conformance list (§12, V3's) should carry the
deviation.

### 3.2 The OverlayFS-side representation — **CONFIRMED, and observed directly**

`overlayfs.rst` (v6.11) states both forms, and D3 §5.2 quotes them correctly: a whiteout is
*"created as a character device with 0/0 device number or as a zero-size regular file with the
xattr `trusted.overlay.whiteout`"*; an opaque directory is marked by *"setting the xattr
`trusted.overlay.opaque` to `'y'`"*; with `userxattr` the prefix becomes `user.overlay.`;
`opaque="x"` is the overlay-on-overlay whiteout-base marker.

`[O]` **host-a 6.11.0-29 HIGH** — observed as a side effect of Part 2's `userxattr` test.
`rm -rf` of a lower-only directory through a userns overlay produced, in the upper dir:

```
    c--------- 2 root root 0, 0 Oct 24 00:47 d
```

A character device, major 0, minor 0, mode 000. **The representation is confirmed by
construction, not only by documentation.** D3 §5.3's translation table is correct.

### 3.3 Unprivileged `mknod c 0 0` — **CONFIRMED, mechanism now read from source, and it generalises further than D3 claims**

D3 §5.4 `[O]` HIGH (behaviour) + `[F]` **MED** (mechanism, explicitly not read from source),
and D3 §15 open question 3. D5 §17 records the same `[O]`.

**Mechanism — now `[F]` HIGH.** `torvalds/linux@v6.11/fs/namei.c#L4063-4074`:

```c
int vfs_mknod(struct mnt_idmap *idmap, struct inode *dir,
	      struct dentry *dentry, umode_t mode, dev_t dev)
{
	bool is_whiteout = S_ISCHR(mode) && dev == WHITEOUT_DEV;
	int error = may_create(idmap, dir, dentry);

	if (error)
		return error;

	if ((S_ISCHR(mode) || S_ISBLK(mode)) && !is_whiteout &&
	    !capable(CAP_MKNOD))
		return -EPERM;
```

D3's guess was exactly right. Three things this adds:

1. **It is generic VFS code, not filesystem-specific.** D3's open question 3 worried the
   mechanism "could be narrower than I think (e.g. filesystem-specific)". It is not: the
   check lives in `vfs_mknod`, above `dir->i_op->mknod`. Any filesystem that implements
   `mknod` at all inherits the exemption.
2. **The capability check is `capable()`, not `ns_capable()`** — i.e. against the **initial**
   user namespace. That is why a *userns-root* process still gets `EPERM` for `c 1 1`: userns
   root has `CAP_MKNOD` in its own namespace and it does not help. The whiteout exemption is
   the only way a non-init-userns process creates a char device, ever.
3. `may_create()` still runs first, so ordinary write permission on the parent directory is
   still required. It is an exemption from `CAP_MKNOD`, not from DAC.

`[O]` **host-a 6.11.0-29 HIGH — re-measured in the hardest configuration D3 did not test:
a genuinely unprivileged uid in the INITIAL user namespace with an empty effective and
permitted capability set** (`setpriv --reuid=65534 --regid=65534 --clear-groups --inh-caps=-all`,
confirmed `CapPrm: 0000000000000000`, `CapEff: 0000000000000000`), on **two different
filesystems**:

```
-- /tmp (tmpfs) --              -- /var/tmp (ext4) --
mknod c 0 0 -> OK               mknod c 0 0 -> OK
mknod c 1 1 -> EPERM            mknod c 1 1 -> EPERM
mknod b 0 0 -> EPERM            mknod b 0 0 -> EPERM
mkfifo      -> OK               mkfifo      -> OK
  crw-r--r-- 1 nobody nogroup 0, 0  (on both)
```

And `[O]` separately, as **userns-root** (mapped uid 0 in a private userns): identical —
`c 0 0` OK, `c 1 1` EPERM, `b 0 0` EPERM.

**Verdict: CONFIRMED, upgraded to `[F]` HIGH, and D3 §15 open question 3 is closed.**
D3's `[R]` HIGH — *"a rootless extractor can create overlay whiteouts natively; do not reach
for `fuse-overlayfs` merely because of whiteouts"* — **stands and is now source-backed.**
D5 §17's row "`mknod` a device node → fails loudly (EPERM)" is correct for *real* devices and
should carry the whiteout exception explicitly, since it reads as a blanket statement.

Residual gap: xfs and btrfs upperdirs are untested (the droplet has only tmpfs and ext4).
Given the check is in `vfs_mknod`, the risk is negligible, but see "Still unverifiable".

### 3.4 The opaque xattr half — **CONFIRMED**

D3 §5.4: `trusted.overlay.opaque` needs `CAP_SYS_ADMIN` in the **initial** userns
(`xattr(7)`); `user.overlay.opaque` succeeds unprivileged. I did not re-run this (D3 measured
it on both hosts and the `xattr(7)` statement is unambiguous), but Part 2's `[O]` — a userns
overlay writing `user.overlay.*` and a root-mode Docker overlay reporting `nouserxattr` —
corroborates the asymmetry from both ends. **CONFIRMED**, and with it D3 §5.5's `[R]` HIGH
"partition the layer cache by xattr mode", which is the load-bearing consequence.

### 3.5 **NEW — a 2026 CVE lands squarely on §5's recommended opaque-whiteout algorithm**

**CVE-2026-95838**, published **2026-10-12** (five weeks *after* D3 was written):
*"containerd: Unpack DoS via repeated opaque whiteouts … containerd's layer unpack handler
repeatedly traverses directories for certain opaque whiteout configurations. A malicious
image layer can trigger repeated directory traversals while opaque whiteouts are processed."*
Fixed in containerd 1.7.35, 2.0.12, 2.2.8, 2.3.5.

D3 §5.1's `[R]` HIGH prescribes exactly the vulnerable shape:

> *"when you meet an opaque marker for directory `D`, record `D` in an 'opaque set' and, at
> layer end, remove from the upper dir any entry under `D` that came from a lower layer only."*

A layer containing *k* opaque markers at nested paths makes that an O(k × tree) sweep, and
the tree is attacker-chosen. **`[R]` HIGH — amend §5.1: in the plain-tree backend, collapse
the opaque set to its minimal set of non-nested prefixes before sweeping, sweep each prefix
exactly once, and bound the total number of opaque markers per layer (the §6.8 entry-count
cap already gives you the hook).** This costs nothing and is the difference between
containerd 2.3.4 and 2.3.5.

Note the asymmetry D3 already identified is now also a *security* argument:
**the OverlayFS-native backend is immune** — setting one xattr per marker is O(k) with no
traversal at all, and the kernel does the merge. D3 §5.3's `[R]` HIGH ("do the translation at
extraction time into the layer's own diff directory") gains a CVE-shaped justification.

---

## Part 4 — §6 extraction hazards (the lost sub-investigation, rebuilt as verification)

Every CVE D3 cites was checked against its primary record (OSV / RustSec advisory-db source).
**All ten exist and D3's characterisations are accurate.** The findings below are therefore
mostly *additions* — five relevant advisories published in 2025–2026 that D3 does not have,
three of which change a recommendation.

### 4.1 CVE audit — what D3 cites, verified

| ID | D3's characterisation | verdict |
|---|---|---|
| **CVE-2014-6407** | Docker ≤1.3.1, path traversal + hardlink/symlink escape in the archive extractor | **CONFIRMED** — OSV: *"Docker before 1.3.2 allows remote attackers to write to arbitrary files and execute arbitrary code via a (1) symlink or (2) hard link attack in an image archive in a (a) pull or (b) load operation."* (D3's "≤1.3.1" vs "before 1.3.2" are the same range) |
| **CVE-2014-9356** | absolute-symlink path traversal in archive extraction | **CONFIRMED** — *"Path traversal vulnerability in Docker before 1.3.3 … via a full pathname in a symlink in an (1) image or (2) build in a Dockerfile."* |
| **CVE-2021-32760** | containerd ≤1.4.7 / 1.5.0–1.5.3, chmod outside the unpack target, "set extended bits like setuid, setgid, and sticky" | **CONFIRMED** — OSV text matches almost word for word; fixed 1.4.8 / 1.5.4 |
| **CVE-2018-15664** | `docker cp` TOCTOU: path resolved, then a component swapped for a symlink | **CONFIRMED** |
| **CVE-2022-23648** | containerd CRI, symlink in an image's `Volumes` reads arbitrary host files | **CONFIRMED** — OSV summary: *"Insecure handling of image volumes in containerd CRI plugin"* |
| **CVE-2017-14992** | moby gzip bomb / decompression DoS | **CONFIRMED** — *"Lack of content verification in Docker-CE … allows a remote attacker to cause a Denial of Service via a crafted image layer payload, aka gzip bombing."* |
| **CVE-2023-25153** | containerd OCI import, unbounded read of certain files | **CONFIRMED** — summary is literally *"containerd OCI image importer memory exhaustion"*; fixed 1.6.18 / 1.5.18 |
| **CVE-2021-41190** | manifest/index media-type ambiguity | **CONFIRMED** |
| **RUSTSEC-2021-0080** | tar < 0.4.36, "Links in archive can create arbitrary directories" | **CONFIRMED** — `rustsec/advisory-db/crates/tar/RUSTSEC-2021-0080.md`, alias CVE-2021-38511 |
| **RUSTSEC-2026-0067** | tar ≤ 0.4.44, `unpack_in` chmods arbitrary directories because `unpack_dir` used symlink-following `fs::metadata()` | **CONFIRMED** — `crates/tar/RUSTSEC-2026-0067.md`, dated 2026-03-19, aliases **CVE-2026-33056** / GHSA-j4xf-2g29-59ph, affected functions `tar::Entry::unpack` and `tar::Entry::unpack_in` `< 0.4.45`. D3's wording is near-verbatim correct |
| **RUSTSEC-2026-0148** | boxlite < 0.9.0, CVSS 10.0, OCI layer symlink escape, fixed with a `SafeRoot` over `openat2(RESOLVE_IN_ROOT)` | **CONFIRMED** — `crates/boxlite/RUSTSEC-2026-0148.md`, 2026-05-16, aliases **CVE-2026-46703** / GHSA-f396-4rp4-7v2j, `CVSS:3.1/AV:N/AC:L/PR:N/UI:N/S:C/C:H/I:H/A:H` = **10.0**, fix text matches D3's exactly |

### 4.2 **NEW — CVE-2026-17106: Docker's own extractor, August 2026, is the textbook version of this bug**

Published **2026-08-18**, two weeks before D3 was written, and absent from it:

> **"Tar extraction in `moby/go-archive` can write outside the destination directory via link
> following."** *"The tar extraction routines in moby/go-archive (`Unpack`, `UnpackLayer`,
> `Untar`/`UntarUncompressed`, and the `ApplyLayer` helpers) do not confine filesystem
> operations to the destination directory. The extractor decides where each archive entry
> lands using **lexical string checks** and then performs the filesystem operation on a path
> that is **resolved by the OS**, so links introduced by the archive can be followed out of
> the destination directory."*

This is the single best citation in the corpus for D3's **M1** (`[R]` HIGH: extract through a
pinned dirfd with `openat2(RESOLVE_IN_ROOT)`), and it replaces an argument from 2014 with one
from eleven weeks ago. The sentence "lexical string checks and then … resolved by the OS" is
precisely D3 §6.2's *"lexical checking cannot [close the TOCTOU]"*, confirmed by Docker
shipping the bug in 2026. **D3 §6.11's `[C]` HIGH — "containerd (`archive/tar.go`): lexical
`filepath.Clean`, `securejoin` for symlink containment…" — reads as reassurance about the
mature implementations. It should not. Moby's extractor was still lexical in 2026.** Treat
every "how the mature implementations do it" line in §6.11 as *what they intend*, not as
evidence that the approach is sufficient.

### 4.3 **NEW — RUSTSEC-2026-0068 / CVE-2026-33055: the PAX-vs-ustar hazard is real, and it hit the exact crate D3 picks**

D3 §6.9 `[R]` HIGH says *"PAX vs ustar disagreement is a security bug, not a curiosity"* and
discusses `path`/`linkpath`. It does not discuss **`size`**, which is where the real-world
exploitation is:

- **CVE-2025-62518** (`astral-tokio-tar` < 0.5.6, published 2025-10-21): *"a boundary parsing
  vulnerability that allows attackers to **smuggle additional archive entries** by exploiting
  inconsistent PAX/ustar header handling. When processing archives with PAX-extended headers
  containing size overrides, the parser incorrectly advances stream position based on ustar
  header size (often zero) instead of the PAX-specified size, causing it to **interpret file
  content as legitimate tar headers**."*
- **RUSTSEC-2026-0068 / CVE-2026-33055** (`tar` ≤ 0.4.44, published 2026-03-19): the inverse —
  *"tar-rs incorrectly ignores PAX size headers if header size is nonzero … the tar-rs crate
  is an outlier in checking for the header size — other tar parsers (including Go
  `archive/tar`) unconditionally use the PAX size override. This can affect anything that uses
  the tar crate to parse archives and expects to have a consistent view with other parsers."*

**This is a parser-differential class, not a path-validation class**, and it defeats every
mitigation D3 lists: the smuggled entries are *invisible* to one parser and *extracted* by
another. It is also the only hazard that can make our `diff_id` verification lie — we hash the
uncompressed byte stream (which is honest), but what we *extract* from it depends on where we
think the entry boundaries are.

**`[R]` HIGH — three rules D3 does not have:**
1. **Honour the PAX `size` record unconditionally** when present, exactly as Go's
   `archive/tar` does, and reject any entry where a PAX `size` record is present *and*
   disagrees with a non-zero ustar size field. Do not pick one silently.
2. **Require `tar` ≥ 0.4.45** — not merely "0.4.46 is current". 0.4.45 fixed *both*
   RUSTSEC-2026-0067 and -0068. `[O]` crates.io, **2026-10-23**: `tar` is still **0.4.46**
   (2026-05-18), so no action beyond pinning `>= 0.4.45`.
3. **After parsing, assert the stream position**: the bytes consumed for an entry must equal
   `round_up(size, 512)`. A parser differential cannot survive an explicit cursor check.

D3 §6.10's "Trailing garbage after the two zero blocks → read to EOF for the digest, but stop
extracting at the end-of-archive marker" is correct and is the same defence applied at the
other end of the stream.

### 4.4 **NEW — CVE-2026-41579 flips D3 §9.3's `[R]` on the init layer from a cost decision to a security decision**

D3 §9.3 `[R]` MED: *"Skip the init layer in the MVP; create the mountpoints directly in the
container's upper dir."* Tradeoff given: a `docker diff`/`docker commit` nicety.
D3 §14's handoff to D1 repeats it: *"we are **not** creating it in the MVP, so D1 must create
those paths in the container's upper dir before bind-mounting over them."*

**CVE-2026-41579** (runc, published 2026-07-01):

> *"when setting up the container rootfs, `setupPtmx` and `setupDevSymlinks` call `os.Remove`
> and `os.Symlink` with a `filepath.Join` string which allow **an image with `/dev` as a
> symlink** to trick runc into deleting files called `ptmx` on the host or creating a
> hardcoded set of symlinks with specific names and targets in an arbitrary pre-existing host
> directory. **This issue is not exploitable under Docker, because Docker creates a top-level
> read-only layer that masks any malicious `/dev` symlink present in the container image** —
> unlike some other Linux container tooling, whose higher-level runtimes built on runc remain
> exposed."* Fixed in runc 1.3.6 / 1.4.3 / 1.5.0.

The "top-level read-only layer" in that sentence **is the `-init` layer**. Docker's `-init`
layer is not only a `docker diff` nicety: it is the mitigation that made Docker non-exploitable
for a 2026 runc escape, because it places a real directory at `/dev`, `/etc/hosts`,
`/etc/resolv.conf`, `/etc/hostname` *above* whatever the image put there.

**DISPUTED — D3's `[R]` MED mis-weighs the tradeoff.** The facts D3 gives are right; the
missing fact is that the init layer is a security boundary. Skipping it is defensible **only
if** the replacement creates those paths through a pinned-root `openat2(RESOLVE_IN_ROOT|
RESOLVE_NO_MAGICLINKS)` walk that refuses to traverse an image-supplied symlink, and
`unlinkat`s any existing non-directory at each mountpoint before `mkdirat`. That is a D1
requirement, and D3's §14 handoff item 6 currently states it as a convenience
("D1 must create those paths") rather than as a containment requirement. **`[R]` HIGH —
restate handoff item 6 as: those mountpoints must be created through the pinned root, never
by `join`-ing strings, and D1 must assume `/dev` and `/etc` in the image are hostile.**
Alternatively, implement the init layer; it is one extra read-only lower directory and it
makes the whole class structurally impossible.

Two neighbours confirm the class is live and is *not* about extraction at all —
it is about every later path operation on an extracted tree:
- **CVE-2026-55686** (Podman 3.0.0–5.7.0, 2026-06-26): *"running a malicious container image
  where the **WORKDIR** path contains a symlink can create a directory or modify ownership on
  the host filesystem"*; the ownership half needs a race, i.e. a TOCTOU. Fixed 5.7.1.
- **CVE-2024-9676** (containers/storage; Podman, Buildah, CRI-O): *"The containers/storage
  library will read `/etc/passwd` inside the container, but does not properly validate if that
  file is a symlink, which can be used to cause the library to read an arbitrary file on the
  host."* Our `User` resolution (image config `User: "nginx"` → uid) reads exactly that file.

**`[R]` HIGH — the pinned-root rule is not an extractor rule, it is an engine rule.** Every
path the engine ever derives from image-controlled data — the rootfs mountpoints, `WorkingDir`,
`/etc/passwd` for `User`, `Volumes`, and the `merged` mount target — must go through the same
`RESOLVE_IN_ROOT` handle. Three separate projects shipped this bug in 2024–2026 and in every
case the extraction code was already fixed.

### 4.5 `openat2` — **CONFIRMED, with a caveat D3 should carry**

`openat2(2)` (man7, HISTORY: *"Linux 5.6"*) — D3 §6's M1 and its "needs Linux 5.6+" are
**CONFIRMED**.

- `RESOLVE_IN_ROOT`: *"Treat the directory referred to by `dirfd` as the root directory while
  resolving `path`."* Absolute symlinks and `..` are reinterpreted against `dirfd`.
- `RESOLVE_BENEATH`: refuses any resolution leaving the subtree (stricter: no reinterpretation,
  just refusal). `RESOLVE_NO_SYMLINKS`: no symlink resolution at all.
- **Caveat worth recording**: the man page says `RESOLVE_IN_ROOT` *"currently … also disables
  magic-link resolution"* but adds *"However, this may change in the future."* So the brief's
  `RESOLVE_IN_ROOT|RESOLVE_NO_MAGICLINKS` pairing is correct and **`RESOLVE_NO_MAGICLINKS`
  must be passed explicitly** rather than relied upon implicitly.
- `RESOLVE_IN_ROOT` does **not** stop mount-point traversal; that is `RESOLVE_NO_XDEV`. For an
  empty per-layer staging directory (D3's M2) there is nothing to cross, so this is cheap
  insurance rather than a requirement — but it is required for the §4.4 rule above, where the
  tree being walked is a *populated* container rootfs that may contain our own bind mounts.

What `openat2` does **not** give you, and D3 does not say: it constrains *resolution*, not
*operations*. `linkat(2)` with an already-open fd, `renameat2`, and `fchownat` on a resolved
handle are all still your responsibility, and the hardlink hazard of §6.4 is unaffected by
`RESOLVE_IN_ROOT` because a hardlink has nothing to resolve. D3 §6.4's mitigation (resolve
`linkname` through the same pinned root, require the target to exist *within this layer's
staging dir*, refuse absolute targets) is **CONFIRMED as the right and necessary complement**.

### 4.6 §6 items checked and found correct

- §6.1 "Zip Slip"/"Tar Slip", strip leading `/`, reject `..` lexically *and* rely on M1 — correct.
- §6.3 device nodes: mechanism correct; see §3.1 on the spec-conformance nuance.
- §6.5 setuid: `[O]` "chmod 4755 on a file you own succeeds unprivileged" — plausible and
  consistent with `chmod(2)`; the dangerous part being the *owner* is correct.
- §6.6 `AT_SYMLINK_NOFOLLOW` on chown is not optional — correct, and `fchownat(2)` says so.
- §6.7 `SCHILY.xattr.*` / `LIBARCHIVE.xattr.*` PAX conventions — correct; the spec lists
  xattrs as a MUST-include attribute (`layer.md#L58`) without naming the encoding, so the
  `SCHILY.` convention is `[C]`, which is how D3 tags the Docker half.
- §6.8 three independent limits (descriptor `size`, uncompressed cap, entry-count cap) —
  correct and cheap; the `[R]` MED to cap manifest/config JSON size is exactly CVE-2023-25153.
- §6.9 `NAME_MAX` 255 / `PATH_MAX` 4096, and depth as a cleanup DoS — correct.
- §6.10 `g` (global) PAX headers ignored, vendor types rejected — correct and conservative.
- §6.11 `cyphar/filepath-securejoin` and `pathrs` as the canonical implementations — correct;
  `[O]` crates.io 2026-10-23: **`pathrs` is now 0.2.6 (2026-09-06)**, D3 says 0.2.5.

---

## Part 5 — §4 layer format, digests and ChainID

### 5.1 `diff_id` vs blob digest — **CONFIRMED**

`image-spec@v1.1.1/config.md#L26-31`, verbatim: *"A layer DiffID is the digest over the
layer's uncompressed tar archive and serialized in the descriptor digest format… **NOTE: Do
not confuse DiffIDs with layer digests**, often referenced in the manifest, which are digests
over compressed or uncompressed content."*

D3 §4.3's table is exactly right, including "for an uncompressed layer the two digests are
identical" and the parallel-array claim. **CONFIRMED.** One addition worth carrying: the same
paragraph says *"Layers SHOULD be packed and unpacked reproducibly to avoid changing the layer
DiffID, **for example by using tar-split** to save the tar headers."* So `tar-split` is named
in the spec, not only in moby. That does **not** overturn D3 §4.4's `[R]` HIGH "do not
implement tar-split" — the SHOULD is about producing layers, which we do not do — but the
`[R]` should cite the spec sentence it is declining, rather than presenting tar-split as a
purely Docker-internal concern.

### 5.2 ChainID — **CORRECTED tag: this is `[F]`, not `[C]`**

D3 §4.4 tags the ChainID recursion `[C]` HIGH and sources it to `moby/layer/layer.go` and
containerd's `identity.ChainID`. **It is normative in the OCI image specification.**
`image-spec@v1.1.1/config.md`, section **"Layer ChainID" → "Definition"**:

```text
ChainID(L₀) =  DiffID(L₀)
ChainID(L₀|...|Lₙ₋₁|Lₙ) = Digest(ChainID(L₀|...|Lₙ₋₁) + " " + DiffID(Lₙ))
```

Identical to D3's formula, including the single ASCII space. The spec's expansion
`ChainID(A|B|C) = Digest(Digest(DiffID(A) + " " + DiffID(B)) + " " + DiffID(C))` matches, and
the spec's own security rationale (*"if an attacker can define `x`, relying on `C` provides no
guarantee that the layers were applied in any order"*) is the argument for D3 §4.5's
`[R]` HIGH to key the extracted-layer cache on ChainID.

**Re-tag to `[F]` HIGH.** This matters: a `[C]` can be deviated from; an `[F]` cannot without
breaking interoperability with every other OCI tool. D3's own `[O]` (7/7 chain IDs for
`nginx:stable` matched live `layerdb` directory names) is strong empirical confirmation that
moby implements the spec formula including the `sha256:` prefixes in the concatenation.

### 5.3 Gzip determinism — **CONFIRMED, and the `[R]` is right for a stronger reason than D3 gives**

D3 §4.2 `[F]` HIGH: *"the compressed digest depends on the compressor: MTIME, XFL, OS, and the
deflate implementation's choices all land in the hash"*, therefore `[R]` HIGH **never
recompress a layer**.

`[O]` **local (macOS), HIGH** — same 110 000-byte input, four compressors, sha256 of the gzip
stream (first 16 hex):

```
python gzip (zlib) L6, mtime=0 : affcf52a90ca2c02
python gzip (zlib) L9, mtime=0 : 131e801f5a96c042
system gzip -6 -n              : ed189217de3234cf
system gzip -9 -n              : 78d0cb7691ac4959
```

Three independent axes of divergence, all demonstrated:
1. **Compression level** changes the bytes (L6 vs L9, both implementations).
2. **Implementation** changes the bytes *at the same nominal level* — zlib L6 and GNU gzip -6
   produce different streams from identical input with identical header fields. This is the
   important one and it is the one D3 states least directly: it is not just the header, the
   **DEFLATE encoder's match/length choices differ between implementations and versions**.
3. **Header MTIME** changes the bytes (`mtime=0` vs `mtime=1` differed in the same run).

And the converse, also measured: the *same* library at the *same* level with `mtime=0` is
**byte-identical across runs** (`affcf52a90ca2c02` twice). So gzip is deterministic *per
compressor build and settings*, and non-reproducible *across* them.

**Verdict: D3 §4.2's `[R]` HIGH "never recompress a layer; store and forward the compressed
bytes exactly as received" is CONFIRMED, and the answer to the brief's question is: no, our
CAS can never key on the compressed digest of anything we produced.** Not even if we match
BuildKit's zeroed header, because we would also have to match `zlib`'s exact version and
settings. The only compressed digests we may treat as authoritative are the ones that arrived
over the wire.

**What zstd changes:** `image-spec@v1.1.1/layer.md` registers
`application/vnd.oci.image.layer.v1.tar+zstd` (and the nondistributable variant), added in
image-spec v1.1 — **CONFIRMED**. Zstd makes reproducibility *worse*, not better, for three
reasons worth recording: (a) the frame header carries an optional content size, a window
descriptor and an optional dictionary id, so two encoders at "level 3" differ structurally,
not just in entropy coding; (b) `libzstd` changes its default parameter tables between
releases, so the same level in two library versions is a different compressor; (c) zstd's
multi-threaded mode (`ZSTD_c_nbWorkers > 1`) splits the input into jobs and is **not
byte-reproducible against single-threaded output at all**. So D3's `[R]` MED — decompress
zstd, never produce it — is **CONFIRMED and if anything understated**. `[O]` crates.io
**2026-10-23**: the `zstd` crate is now **0.14.3 (2026-10-16)**, not 0.13.3 as D3 records —
a major-version move in the seven weeks since; re-evaluate the API before pinning.

### 5.4 §4 items checked and correct

- `manifest.layers` ∥ `config.rootfs.diff_ids`, base-layer-first, and the `[R]` to refuse on a
  length mismatch — correct and follows from the spec text D3 cites.
- `config.history` is **not** parallel; `empty_layer: true` consumes no layer — correct.
- the `cache-id` indirection (ChainID ↔ graphdriver directory name) — **confirmed on the live
  host**, `[O]` read-only: `/var/lib/docker/image/overlay2/layerdb/sha256/08000c18…/` contains
  exactly `cache-id`, `diff`, `size`, `tar-split.json.gz` — and **no `parent` file**, because
  this is a base layer. That is the terminator of D3's "linked list" and worth stating.
- the `diffid-by-digest` / `v2metadata-by-diffid` maps and the `[R]` to implement only the
  first — correct.

---

## Part 6 — §7 CAS layout, GC and interrupted pulls

### 6.1 The OCI image layout and the three-store split — **CONFIRMED**

`[O]` **host-a, read-only (`ls` only) HIGH** — re-verified independently of D3:

```
/var/lib/containerd/io.containerd.content.v1.content/{blobs,ingest}
  blobs/sha256/  -> 12 entries
  ingest/        -> empty
/var/lib/docker/buildkit/content/{blobs,ingest}
/var/lib/docker/image/overlay2/layerdb/{sha256,mounts}
/var/lib/docker/tmp/                     <-- not mentioned in D3
/var/lib/docker/overlay2/l/
```

D3's §7.2 table is accurate. **One addition: `/var/lib/docker/tmp/` is Docker's own staging
directory** (it held `moby-detect-rro184324465` at the time of observation) — so Docker, like
containerd, has a fourth "scratch" location distinct from both the blob store and the
graphdriver root. D3's proposed layout (`blobs/`, `ingest/`, `layers/<mode>/`, `containers/`,
`meta/`) covers this, but the `[R]` should note that layer *extraction* staging and blob
*download* staging are different lifetimes and should not share a directory.

### 6.2 **CORRECTED — D3 §7.5 step 1 contradicts D3's own resume `[R]`, and contradicts containerd**

D3 §7.5 step 1, `[R]` HIGH:

> *"`open(ingest/<uuid>/data, O_CREAT|O_EXCL|O_WRONLY)` — a fresh, **unguessable name per
> attempt, never a name derived from the digest**. Reason: two concurrent pulls of the same
> blob must not share a partial file."*

containerd does the **opposite**, deliberately.
`containerd/containerd@v1.7.27/content/local/store.go#L648-653`:

```go
func (s *store) ingestRoot(ref string) string {
	// we take a digest of the ref to keep the ingest paths constant length.
	// Note that this is not the current or potential digest of incoming content.
	dgst := digest.FromString(ref)
	return filepath.Join(s.root, "ingest", dgst.Encoded())
}
```

The ingest directory name is a **deterministic function of the ref**, and concurrency is
handled by a **lock on the ref** (`s.writer` takes it; `Commit` releases it with
`defer unlock(w.ref)`), not by name uniqueness.

**This is not a style difference — D3's design makes D3's own resume recommendation
unimplementable.** D3 §7.5's `[R]` HIGH on resume says: *"keep `ingest/<uuid>/meta` with
`{digest, expected_size, bytes_written, started_at}`; on resume, re-`GET` … with
`Range: bytes=<bytes_written>-`"*. With a fresh random uuid per attempt, a restarted process
has no way to *find* the previous attempt except by scanning every `ingest/*/meta` and
matching digests — an O(ingest) scan that also has to resolve the case where two abandoned
attempts exist for the same digest. A ref-derived name makes resume a single `stat`.

**DISPUTED → resolved in containerd's favour, with D3's concern preserved.**
`[R]` HIGH — name the ingest directory deterministically from the **pull ref** (not the
digest: containerd's comment is explicit that these differ, because the same blob may be
pulled under different refs and you want per-pull progress), and serialise concurrent writers
to the same ref with a lock file or an `O_EXCL` sentinel. D3's stated worry ("two concurrent
pulls of the same blob must not share a partial file") is real and is solved by the lock, not
by the name.

### 6.3 Resume by re-hashing the prefix — **CONFIRMED, and containerd does exactly this, with the same reservation**

D3 §7.5 `[R]` HIGH: *"feed the already-written prefix back through the hasher first … sha256
is not resumable across process restarts without either re-reading the prefix or serialising
the hasher state; re-reading local disk is fast and obviously correct. Tradeoff: an O(n) local
read on resume."*

`containerd@v1.7.27/content/local/store.go#L500-527`, `resumeStatus()`:

```go
	//nolint:dupword
	// TODO(stevvooe): slow slow slow!!, send to goroutine or use resumable hashes
	fp, err := os.Open(data)
	…
	status.Offset, err = io.CopyBuffer(digester.Hash(), fp, *p)
```

**CONFIRMED.** containerd re-reads the prefix through the digester, and its own source comment
names the same tradeoff D3 names. It also validates `ref != status.Ref` (*"This is fairly
catastrophic"*) and `total != status.Total` before resuming — two checks D3's `meta` schema
supports but does not require. `[R]` MED — require both.

### 6.4 fsync discipline — **CONFIRMED, and D3 is stricter than the reference implementation, correctly**

D3 §7.5 steps 4–6: fsync the data fd, rename, **then fsync the `blobs/sha256/` directory fd**,
citing `fsync(2)`'s *"Calling fsync() does not necessarily ensure that the entry in the
directory containing the file has also reached disk. For that an explicit fsync() on a file
descriptor for the directory is also needed."*

`containerd@v1.7.27/content/local/writer.go#L78-140`, `Commit()`: `fp.Sync()` → size check →
digest check → `os.Rename(ingest, target)`. **There is no directory fsync.** So containerd's
blob store can, after a power loss, lose a blob whose data was synced and whose directory
entry was not — recoverable, because the next pull simply re-downloads, but it is a real gap.

**D3's step 6 is correct and should be kept.** This is one of the few places where D3's design
is more careful than containerd's, and the report should say so rather than implying parity.
Two refinements:
- containerd checks the digest **before** the rename and returns `ErrAlreadyExists` if the
  target already exists (`os.Stat(target)`), removing the ingest dir in that case. D3's
  procedure does not handle the "someone else committed this blob while we were downloading"
  race; it should (discard, do not overwrite — the content is identical by construction).
- containerd `os.Chtimes(target, commitTime, commitTime)` after the rename, explicitly so that
  mtime means "committed at", which is what makes D3 §7.3's `[R]` MED **"young object" grace
  period** (don't sweep anything whose mtime is under N minutes) actually sound. Without that
  `Chtimes`, mtime would be the download start time and the grace window would be wrong.
  `[R]` MED — adopt the `Chtimes`; it is one call and it is what makes the GC heuristic safe.

### 6.5 Mark-and-sweep vs refcounts, and leases — **CONFIRMED as a `[R]`, with one caveat**

D3 §7.3's reasoning (refcounts require every mutation to be transactional with the counter; a
crash between "write blob" and "increment count" leaks or under-counts; mark-and-sweep is
crash-safe by construction) is sound and matches containerd's actual model
(leases + `containerd.io/gc.ref.*` labels + mark-and-sweep from roots). **No correction.**

The caveat D3 does not state: **mark-and-sweep's roots must include in-flight work**, and
"every entry in `index.json` + every existing container" does not. A pull that has committed
layers 0–4 and is still fetching layer 5 has no tag pointing at anything; a sweep that runs at
that moment deletes the four committed layers out from under it. That is precisely what
containerd's **leases** are for, and D3 dismisses them (*"no leases"* is rung 4's explicit
non-goal). The `[R]` MED grace period mitigates it only if the grace window exceeds the
longest pull, which for a 20 GB ML image it will not.

**`[R]` HIGH — the MVP needs one lease-shaped thing, and it is cheap: a `meta/roots/<pull-id>`
file listing the digests an in-flight pull depends on, created before the first blob is
committed and removed at the end; the sweeper treats every such file as a root.** That is
~20 lines, it is crash-safe (a stale root file is swept by the same expiry rule as `ingest/`),
and it removes the one failure mode where the GC destroys live data rather than merely leaking.

---

## Part 7 — the three-way overlay-in-userns kernel-floor dispute, adjudicated

The three positions:
- **D5 §7.1**: "unprivileged overlayfs in userns + `userxattr` | upstream **5.11**".
- **D5 §7.2** `[O]` old-host: it **works on RHEL 8 / 4.18**; the mount line carries **no
  `userxattr`**; `[F]` MED that Red Hat backported it, "the RHEL backport predates the
  `user.overlay.*` xattr namespace split, so it uses `trusted.overlay.*`-free semantics under
  a different mechanism".
- **D3 §5.5/§8.4** `[O]` host-a: a **6.11 userns overlay wrote `user.overlay.uuid`** and
  `user.overlay.impure` **without** the option being passed; D3 §15 open question 1 marks the
  mechanism and starting version **LOW**.

### 7.1 Upstream version — **CONFIRMED: 5.11, for both halves**

Measured across tags: `fs/overlayfs/super.c` contains **zero** occurrences of
`FS_USERNS_MOUNT` or `userxattr` at **v5.10**, and **nine** at **v5.11**. In v6.11 the flag is
still there (`super.c#L1487`: `.fs_flags = FS_USERNS_MOUNT,`).
**D5 §7.1's 5.11 is correct and D3 does not contradict it.** Both the `FS_USERNS_MOUNT`
capability *and* the `userxattr` option landed in the same release — they were one series.

### 7.2 What RHEL 8 actually backported — **CORRECTED: D5's mechanism guess is wrong**

D5 §7.2 reasons: *"The absence of `userxattr` fits: the RHEL backport predates the
`user.overlay.*` xattr namespace split, so it uses `trusted.overlay.*`-free semantics under a
different mechanism."*

**That cannot be right, and D3's own data disproves it.** D3 §5.5 `[O]` ROCKY records that on
the same 4.18 kernel, *"passing `,userxattr` explicitly on 4.18 fixes it: `rm -rf` succeeds,
the upper dir gets `user.overlay.impure="y"`, and the removed directory becomes a `c 0 0`
whiteout."*

A kernel that honours `-o userxattr` and writes `user.overlay.*` **has the 5.11 userxattr
code backported**. There is no "different mechanism". RHEL 8 backported **both** halves of the
5.11 series; it simply does not turn `userxattr` on by itself — which is also exactly what
**mainline** does (§7.3).

The observed 4.18 failure mode without the option is fully explained: with `userxattr` off,
overlay uses `trusted.overlay.*`; removing a *directory* that exists only in a lower layer
requires creating an **opaque** upper directory, i.e. `setxattr(trusted.overlay.opaque)`,
which `xattr(7)` makes impossible from any user namespace → the copy-up fails → overlay
surfaces **`EIO`**. D3's `[O]` (`rm: cannot remove '.../mrg/d': Input/output error`) is a
directory, which is the tell. A plain file unlink would have produced a `c 0 0` whiteout and
succeeded, because §3.3's `vfs_mknod` exemption needs no capability at all.

**Adjudication: D5 §7.2's *observation* is right, its *mechanism* is wrong. Correct it to:
RHEL 8's 4.18 kernel carries a backport of the full 5.11 unprivileged-overlay series,
including the `userxattr` mount option, which is opt-in exactly as upstream.**

### 7.3 The 6.11 auto-enable — **CORRECTED: it is an Ubuntu patch, not kernel 6.11 behaviour.** D3 §15 open question 1 is closed

D3 §5.5 `[F]` MED/LOW: *"I could not find the auto-enable in `fs/overlayfs/params.c` or
`super.c` … the mechanism and the kernel version at which it started is **LOW**."*

The reason D3 could not find it is that **it is not there.** In mainline
`torvalds/linux@v6.11`, `config->userxattr` is assigned in exactly one place —
`fs/overlayfs/params.c#L596-597`, `case Opt_userxattr: config->userxattr = true;` — i.e. only
when the option is passed. The only other mention in `super.c` is the hint D3 already found
(`#L730-731`, *"try mounting with 'userxattr' option"*). **Mainline 6.11 does not auto-enable
`userxattr` for a userns mount.**

`[O]` **host-a** — `/proc/version_signature` reads `Ubuntu 6.11.0-29.29-generic 6.11.11`.
The auto-enable is an **Ubuntu SAUCE patch**, introduced alongside Canonical's fixes for
**CVE-2023-2640 / CVE-2023-32629** ("GameOver(lay)") and carried forward: the series reverts
the older Ubuntu patch that used `__vfs_setxattr_noperm()` in `ovl_do_setxattr()` (which
bypassed `cap_convert_nscap()` and was the escalation), and then switches overlay's internal
metadata to the unprivileged `user.overlay.*` namespace when the mount comes from a user
namespace — because without that, every unprivileged userns overlay mount would fail `EPERM`
on `trusted.overlay` writes. The patch's own stated regression risk is *overlays on disk that
carry `trusted.overlay.*` xattrs written by a prior kernel*. Source tier: Ubuntu kernel-team
list (community/vendor discussion), used here only because this is vendor-patch behaviour
with no upstream documentation — the *absence* from mainline is established from source.

**Consequences, and they are the ones that matter:**

1. **D3's `[O]` generalises to "Ubuntu kernels", not to "kernel 6.11".** A mainline, Debian,
   Fedora or Amazon Linux 6.11 will **not** auto-select `userxattr`, and our rootless mode
   would then silently get a `trusted.overlay.*` overlay that `EIO`s on the first directory
   removal — the exact 4.18 failure. The `[O]` must be re-scoped or it will be read as a
   kernel-version fact and used to skip the option on a non-Ubuntu host.
2. **D3's `[R]` HIGH — "always pass `userxattr` explicitly when we are not
   CAP_SYS_ADMIN-in-init-userns, and never when we are" — is CONFIRMED, and is now load-bearing
   on every distro rather than a belt-and-braces measure on one.** D3 reached the right
   recommendation from a wrong premise; the recommendation survives, the premise does not.
3. The Ubuntu patch's regression note is our §5.5 store-partition problem stated by the kernel
   team: a store written under one xattr mode is wrong under the other.

### 7.4 **The question the brief asked: is a layer extracted on one kernel readable by the other?**

**No, and the failure is silent.** Concretely, across the three environments:

| | xattr namespace overlay reads | what a rootless extractor can write | a layer extracted the other way |
|---|---|---|---|
| root, init userns (any kernel ≥ 4.18) | `trusted.overlay.*` | — (we are root) | `user.overlay.opaque` is **ignored** → deleted directories reappear |
| userns, 4.18-RHEL / mainline ≥5.11 / non-Ubuntu 6.x, `userxattr` **passed** | `user.overlay.*` | `user.overlay.opaque` | `trusted.overlay.opaque` is **ignored** → same |
| userns, same kernels, `userxattr` **not** passed | `trusted.overlay.*` | nothing (EPERM) | `rm -rf` of a lower dir → **`EIO`** |
| userns, Ubuntu ≥ ~6.x (auto) | `user.overlay.*` | `user.overlay.opaque` | as row 2 |

**The whiteout *device nodes* are portable** (char 0:0 is namespace-independent, §3.3) —
**only the opaque-directory markers are not.** So a cross-mode layer does not fail loudly; it
produces a filesystem in which a directory the Dockerfile deleted is visible again, with its
original contents. D3's description of the symptom — *"a file I deleted in the Dockerfile is
back"* — is exactly right.

**`[R]` HIGH — D3 §5.5's store partition (`layers/trusted/<chainid>` vs
`layers/userxattr/<chainid>`) is CONFIRMED as mandatory, and the partition key should be the
xattr namespace, not the privilege mode**, because (per §7.3) privilege does not determine
namespace on its own — the kernel's patch set does. Record the namespace actually used in the
layer's metadata at extraction time and refuse to mount a layer whose recorded namespace does
not match the mount's.

---

## Part 8 — §9 overlay2, §10 alternatives, §11 progression, §13 crate behaviour

### 8.1 §9.1–9.2 overlay2 on-disk layout and the short-link arithmetic — **CONFIRMED verbatim**

`moby/moby@v28.3.1/daemon/graphdriver/overlay2/overlay.go#L73-85` — D3's quote is exact:

```go
	mergedDirName = "merged"
	lowerFile     = "lower"
	maxDepth      = 128

	// idLength represents the number of random characters
	// which can be used to create the unique link identifier
	// for every layer. If this value is too long then the
	// page size limit for the mount command may be exceeded.
	// The idLength should be selected such that following equation
	// is true (512 is a buffer for label metadata).
	// ((idLength + len(linkDir) + 1) * maxDepth) <= (pageSize - 512)
	idLength = 26
```

And the `mountFrom` fallback, `#L574-591` — **note the threshold**:

```go
	pageSize := unix.Getpagesize()

	// Use relative paths and mountFrom when the mount data has exceeded
	// the page size. The mount syscall fails if the mount data cannot
	// fit within a page and relative links make the mount data much
	// smaller at the expense of requiring a fork exec to chroot.
	if len(mountData) > pageSize-1 {
```

**`pageSize - 1` = 4095 — moby's own threshold is exactly the limit Part 1 measured.** That is
an independent confirmation of §1.1 from a completely different source. D3 §9.2 is **CONFIRMED
in full**, and D3's `[R]` MED fallback ("relative paths + `chdir` into the store root") is
literally moby's `mountFrom`, still present in Docker 28.3.1.

`[O]` **host-a, read-only HIGH** — the layer directory and `layerdb` shapes D3 records are
reproduced, with one detail D3's example obscures: a **base** layer's `layerdb` directory
contains `cache-id`, `diff`, `size`, `tar-split.json.gz` and **no `parent`**. The absence of
`parent` is the list terminator and is how you identify the base without recomputing chain IDs.

### 8.2 §9.4 the snapshotter interface — **CORRECTED (minor): it is eleven methods, not six**

`containerd/containerd@v1.7.27/snapshots/snapshotter.go` — the `Snapshotter` interface is
`Stat`, `Update`, `Usage`, `Mounts`, `Prepare`, `View`, `Commit`, `Remove`, `Walk`, `Close`,
`Cleanup`. D3 §9.4's table lists seven and its `[R]` argues against building "the full
six-method trait". The count is wrong in a direction that *weakens* D3's own argument — the
real interface is larger and therefore an even worse thing to adopt speculatively.

The two ideas D3 extracts are **CONFIRMED** against the source's own doc comments: the file
documents unpacking as `Prepare(key, parent)` → write → `Commit(digest, key)` and running a
container as `Prepare(containerKey, imageRootFSChainID)` with `Remove` instead of `Commit`
(`#L191-253`), and `Prepare`/`View`/`Mounts` all return `[]mount.Mount` rather than mounting.
D3's `[R]` HIGH ("copy the two ideas, not the interface"; MVP = `ensure_layer(chain_id)` +
`prepare_container(id, chain_ids) -> MountSpec`) is sound and I would not change it.

### 8.3 §10 alternatives — no corrections, one addition

Every verdict in §10 survives: plain-directory first, bind mounts are a different feature,
`fuse-overlayfs` only for kernels that refuse unprivileged overlay outright, btrfs/zfs not for
us, devicemapper removed in Engine v25, erofs/composefs is where the ecosystem is going and is
the reason to keep the `MountSpec` boundary clean, lazy pulling is out of scope.

One addition that strengthens the `fuse-overlayfs` verdict: Part 7 shows native overlay works
unprivileged on *both* test kernels **provided `userxattr` is passed**, and §3.3 shows
whiteout creation needs no privilege at all on any kernel. So the residual case for
`fuse-overlayfs` is narrower than D3 says — it is "mainline < 5.11 without a vendor backport",
and D3's own 4.18-RHEL data shows the vendor backports are widespread. `[R]` — D3's "do not
implement a `fuse-overlayfs` backend; if we need it, exec it" is correct and the window for
needing it is closing.

### 8.4 §11 progression — **one real internal contradiction, and one decision placed on the wrong rung**

The brief asks whether any stage depends on something a later stage introduces. The dependency
*chain* is sound (rung 0 → 7 each needs only its predecessors). But:

**Contradiction — rung 2's non-goal column contradicts §11.3's resolution.** §11.2's table
gives rung 2 the explicit non-goal *"no overlay — **extract each layer into one flat tree**"*,
while §11.3's `[R]` HIGH resolution says *"make the extractor emit an **overlay-native
per-layer diff** from rung 2 onward"*. Those are the two sides of the expensive decision
§11.3 is adjudicating, and the table still carries the losing option. Anyone implementing
from the table builds the thing §11.3 spent a page arguing against, and rungs 5–6 then require
the re-extraction §11.3 says to avoid. **Fix the table, not the prose.**

**Mis-placed decision — the store key is committed at rung 4 but is baked in at rung 2.**
§11.3 says *"commit to (i) the OCI image layout for blobs, (ii) chain-id as the layer key,
(iii) the mode partition, **at rung 4**, before anyone has a populated store."* But rung 2
already writes one directory per layer and already has everything it needs to compute chain
IDs (it has the config's `diff_ids`). If rung 2 names those directories anything else, rung 4
is a migration — the exact outcome §11.3 exists to prevent. **`[R]` HIGH — move (ii) and (iii)
to rung 2.** They cost one path component each and nothing downstream changes.

Two smaller notes:
- Rung 1's success test (`run --rootfs ./alpine-extracted`) depends on an extracted rootfs
  that rung 2 produces. It is fine *if* the directory comes from an external tool
  (`skopeo` + `umoci`, or `docker export`), but the table should say so, because as written
  rung 1 reads as self-contained and is not.
- Rung 5's fixture is two layers, which is lucky: per §2.7 a read-only overlay with **one**
  lower is rejected by the kernel, so a single-layer fixture (alpine!) would fail for a reason
  having nothing to do with the code under test. Add that as an explicit note on rung 5.

### 8.5 §13 — behavioural claims about `tar` / `flate2` / `zstd`

**`tar` crate defaults — CONFIRMED verbatim from source**, `alexcrichton/tar-rs@0.4.46/src/archive.rs#L55-69`:

```rust
            inner: ArchiveInner {
                mask: u32::MIN,
                unpack_xattrs: false,
                preserve_permissions: false,
                preserve_ownerships: false,
                preserve_mtime: true,
                overwrite: true,
                ignore_zeros: false,
```

Every row of D3 §13.3's table is right. **Two values D3's table leaves blank, both of which
matter:**
- **`overwrite` defaults to `true`** — existing files *and symlinks* are replaced by default.
  Combined with RUSTSEC-2026-0067 (the chmod-through-symlink bug in `unpack_dir`), this is the
  default that made the advisory exploitable. D3's table has a dash in the default column.
- **`ignore_zeros` defaults to `false`** — the iterator stops at the two zero blocks, which is
  the behaviour D3 §6.10 wants ("stop extracting at the end-of-archive marker"). Good news,
  and worth stating so nobody "fixes" it.

**D3's `[R]` HIGH — "use `tar` as a header *parser*, own the filesystem layer ourselves, never
call `unpack`/`unpack_in`" — is CONFIRMED and is the correct reading.** Part 4.3 adds the
reason D3 does not have: the crate's hazards are not only in its filesystem layer
(RUSTSEC-2021-0080, -2026-0067) but in its **header parsing** too (RUSTSEC-2026-0068 / the PAX
`size` divergence). So "parser good, filesystem layer bad" is too clean a split.
**`[R]` HIGH — pin `tar >= 0.4.45`, and add the "bytes consumed == round_up(size,512)" cursor
assertion from §4.3 as our own check on top of the crate's parse.**

`flate2` — D3's description is accurate; `[O]` crates.io 2026-10-23: still **1.1.10
(2026-08-28)**, no drift. The behavioural claim that matters for us is not in D3 and is worth
one line: `flate2`'s `GzDecoder` stops at the end of the **first** gzip member, while
`MultiGzDecoder` concatenates members. A layer blob is a single member, so `GzDecoder` is
correct *and* is the safer choice — a multi-member stream is trailing-data smuggling and
should be rejected, which `GzDecoder` does for us by leaving bytes unread.

`zstd` — **drifted**: D3 records 0.13.3 (2025-02-20); `[O]` crates.io 2026-10-23 shows
**0.14.3 (2026-10-16)**. The `[R]` MED (decompression only) is unaffected, but the version is
stale by a major release.

**Out of my scope by instruction:** maintenance health, advisories about the dependency set as
a whole, and the overall crate selection are **V7's**. The only drift figures I record above
are the ones attached to a behavioural claim I verified.

---

## Corrections that change a recommendation

1. **`lowerdir+`/`datadir+` is a Linux 6.7 feature, not 6.8** (D3 §8.3 quotes
   `overlayfs.rst`, which is itself wrong — the code is in `torvalds/linux@v6.7`, commit
   `24e16e385f22`, tag `ovl-update-6.7`). → shifts §8.3/§9.2/§14's kernel floor for the
   no-symlink-farm design down one release.
2. **`fsconfig` removes the 4095-byte limit but imposes a 255-byte limit per lower path**
   (`fs/fsopen.c#L430`, `strndup_user(_value, 256)`; measured: 255 OK, 256 EINVAL;
   `FSCONFIG_SET_PATH` is rejected by overlayfs). → D3 §8.3's `[R]` HIGH ("the page limit
   disappears entirely") needs a new validation rule, and our store root must stay shallow.
3. **`fsconfig` with the *aggregate* `lowerdir=` string is capped at 255 bytes too** (measured:
   439 B → EINVAL), so porting to the new API without switching to `lowerdir+` makes things
   *worse* than `mount(2)`. → new `[R]`, not in D3.
4. **`mount(8)` / `mount(2)` with `-o lowerdir+=` does NOT escape the page limit**
   (strace: util-linux 2.40.2 issues one `mount(2)` under every `LIBMOUNT_FORCE_MOUNT2` mode;
   300 × `lowerdir+=` = 7799 B → ENOENT). → the §8.3 `[R]` must say *genuine `fsconfig(2)`
   syscalls*; a `mount(8)` command line is not an implementation of it.
5. **Below 6.5, `fsopen("overlay")` silently falls back to the legacy fs_context shim**, which
   re-imposes a ~4094-byte cumulative cap (`fs/fs_context.c#L602`). → the probe must test
   `lowerdir+` acceptance, not `fsopen` success.
6. **`OVL_MAX_STACK = 500`** (`fs/overlayfs/params.h#L20`, and the kernel's own error string at
   501). → D3 §8.3's `[C]` LOW becomes `[F]` HIGH; §15 open question 2 closed.
7. **upperdir and workdir must be on the same *mount*, not merely the same filesystem**
   (`super.c#L789-793`; a `mount --bind` of the same tmpfs fails). → D3 §8.6's `[R]` HIGH to
   compare `statfs.f_fsid`/`st_dev` is **wrong**; compare `statx` `STATX_MNT_ID` or mountinfo
   mount ids.
8. **A read-only overlay requires at least TWO lower layers** (`super.c#L1123-1126`; measured).
   → `alpine` has one layer; D3 §8.1's `[F]` and the `--read-only` design need the
   tmpfs-upper-plus-`MS_RDONLY` form or a padding lower.
9. **`metacopy=on`, `redirect_dir=on`, `redirect_dir=off` and `verity` are hard-rejected under
   `userxattr`** (`params.c#L886-902`; measured). → §8.5's "pass none of these" is now a
   kernel constraint in rootless mode, and §15 open question 8 is moot there.
10. **A `volatile` mount permanently poisons its workdir** (`work/incompat/volatile/dirty`;
    measured: every later non-volatile mount on that workdir fails). → §8.5's `[R]` MED to use
    `volatile` for extraction staging needs "fresh workdir every time, `rm -rf` after".
11. **The 6.11 `userxattr` auto-enable is an Ubuntu SAUCE patch, not upstream kernel
    behaviour** (mainline v6.11 sets `config->userxattr` only from `Opt_userxattr`). → D3's
    `[O]` must be re-scoped to "Ubuntu kernels"; §5.5's `[R]` to always pass `userxattr`
    explicitly becomes load-bearing on every non-Ubuntu host. §15 open question 1 closed.
12. **D5 §7.2's mechanism for RHEL 8 is wrong**: RHEL 8's 4.18 backported the *whole* 5.11
    series including the `userxattr` option (D3's own `[O]` shows `-o userxattr` works there);
    it simply does not auto-enable it, exactly like mainline. The `EIO` is `trusted.overlay.opaque`
    being unwritable from a userns when removing a lower **directory**. → corrects D5 §7.2 and
    §17's "no `userxattr` in the options" interpretation.
13. **Unprivileged `mknod c 0 0` is a generic VFS exemption** (`fs/namei.c#L4066-4074`,
    `is_whiteout`, guarded by `capable()` against the *initial* userns), confirmed with
    `CapEff=0` on tmpfs **and** ext4. → D3 §5.4's `[F]` MED → `[F]` HIGH; §15 open question 3
    closed; D5 §17's blanket "`mknod` a device node → fails (EPERM)" row needs the exception.
14. **ChainID is normative in `image-spec@v1.1.1/config.md`, not a Docker convention.** →
    D3 §4.4's `[C]` HIGH → `[F]` HIGH.
15. **`image-spec` MUST NOT allow duplicate path entries in a layer** (`layer.md#L22`) and
    **SHOULD NOT use sparse files** (`#L62`). → D3 §6.10's "apply duplicates in stream order"
    and "support sparse decoding" can both become "reject", deleting attack surface.
16. **The `.wh.`-without-basename sentence D3 quotes as spec text does not exist** in
    image-spec v1.0.2/v1.1.0/v1.1.1. → re-tag that `[F]` to `[R]`.
17. **CVE-2026-17106 (2026-08-18): moby's own `go-archive` extractor used lexical checks and
    was escapable via link following.** → replaces D3 §6.11's implied reassurance about mature
    implementations; M1 is non-negotiable.
18. **RUSTSEC-2026-0068 / CVE-2026-33055 + CVE-2025-62518: PAX `size`-record divergence lets an
    attacker smuggle whole tar entries.** → new `[R]`: honour PAX `size` unconditionally,
    reject disagreement with a non-zero ustar size, assert `bytes_consumed == round_up(size,512)`,
    pin `tar >= 0.4.45`.
19. **CVE-2026-41579: Docker is not exploitable by the malicious-`/dev`-symlink runc bug
    *because of the `-init` layer*.** → D3 §9.3's `[R]` MED to skip the init layer, and §14's
    handoff item 6 to D1, change from a convenience tradeoff to a containment requirement:
    create those mountpoints through a pinned `RESOLVE_IN_ROOT` root, or build the init layer.
20. **CVE-2026-95838 (2026-10-12): containerd DoS via repeated opaque whiteouts.** → D3 §5.1's
    plain-tree "opaque set then sweep at layer end" is the vulnerable shape; collapse to
    minimal non-nested prefixes, sweep once, cap the marker count.
21. **containerd names ingest directories deterministically from the ref and serialises with a
    lock** (`content/local/store.go#L648-653`), the opposite of D3 §7.5 step 1's random uuid —
    and D3's own resume `[R]` needs the deterministic name to work. → change step 1.
22. **Mark-and-sweep roots must include in-flight pulls.** D3 §7.3 lists only tags and
    containers; a sweep mid-pull deletes committed layers. → add a `meta/roots/<pull-id>` file
    as a root (the cheap lease), ~20 lines; the grace period alone is not sufficient for long pulls.

---

## Still unverifiable, and the exact test

1. **NFS `upperdir` returns `EACCES` (D3 §8.6 `[O]` ROCKY).** → `ssh old-host`:
   `mkdir -p /tmp/v/{lo,m} $HOME/ovlu $HOME/ovlw && strace -f -e trace=mount mount -t overlay overlay -o lowerdir=/tmp/v/lo,upperdir=$HOME/ovlu,workdir=$HOME/ovlw /tmp/v/m`
   (read the errno from the strace line, not from `mount`'s message).
2. **NFS as `lowerdir` with a local upper works (D3 §8.6 `[O]`).** → same host, swap the roles.
3. **Whether the `vfs_mknod` whiteout exemption holds on xfs and btrfs upperdirs.** Source says
   it must (the check is above `dir->i_op->mknod`), but only tmpfs and ext4 were measured. →
   on any host with xfs/btrfs scratch:
   `setpriv --reuid=65534 --regid=65534 --clear-groups --inh-caps=-all sh -c 'cd <xfs-dir> && mknod w c 0 0; echo $?'`
4. **Whether RHEL 8's 4.18 `fs/overlayfs/` really contains the 5.11 `userxattr` code** (I
   inferred it from D3's `-o userxattr` working). → on a RHEL 8 box with the source RPM:
   `rpm -q --changelog kernel | grep -i -A3 'overlay.*userns\|userxattr'` and
   `grep -rn 'userxattr\|FS_USERNS_MOUNT' kernel-4.18.0-425.el8/fs/overlayfs/`.
5. **The exact Ubuntu kernel versions that carry the auto-`userxattr` SAUCE patch.** → on an
   Ubuntu host: `apt-get source linux-image-$(uname -r)` then
   `grep -rn 'userxattr' debian/patches/ fs/overlayfs/params.c fs/overlayfs/super.c`, or
   `git log --oneline -S userxattr` in `git://git.launchpad.net/~ubuntu-kernel/ubuntu/+source/linux`.
6. **`fsconfig`-mounted overlay at 500 layers with realistic 90-byte store paths and a real
   image.** I measured 300 layers of 16-byte paths and 500 layers for the limit error, not both
   at once. → on a 6.7+ host, rerun `v4probe.py` Test B with paths padded to 90 bytes and
   `nlow=500`.
7. **Whether `mount(8)` on util-linux ≥ 2.41 uses the new mount API for overlay** (2.40.2 does
   not, measured). → `strace -e trace=mount,fsopen,fsconfig,fsmount,move_mount mount -t overlay …`
   on a host with a newer util-linux.
8. **Docker's `/var/lib/docker/tmp` lifecycle** (what is staged there, when it is swept). →
   non-production host: `inotifywait -m -r /var/lib/docker/tmp &` then `docker pull <image>`.
9. **containerd ingest expiry behaviour on restart** (D3 §7.5 `[R]` MED asserts containerd
   expires old ingests). → non-production host: create a partial ingest, restart containerd,
   `ctr content active` and `ctr content ls`.
10. **Whether a layer extracted with `user.overlay.opaque` is silently mis-merged by a
    `trusted.overlay.*` mount** (the §7.4 table's central claim — argued from the xattr
    namespace, not measured end-to-end). → on a non-production 6.x host: extract a two-layer
    fixture with an opaque marker in `user.overlay.*`, mount it as **root in the init userns**
    without `userxattr`, and assert the deleted directory is visible again.
