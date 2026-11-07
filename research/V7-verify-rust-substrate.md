# V7 — Adversarial verification: the Rust implementation substrate

**Verifier:** V7. I did not write D1–D5. Fresh eyes, hostile intent.
**Mandate:** final adjudication of the four-way `nix` / `caps` / rustix-scope disagreement, plus
every claim about the post-`clone` pre-`execve` window, crate APIs, maintenance health, and
RUSTSEC advisories.
**Date of verification:** 2026-09-04.

## Method and evidence base

`[M]` I read **crate source**, not docs.rs prose and not READMEs. Every crate under discussion was
downloaded from `static.crates.io` and extracted locally:

```
nix-0.31.3      rustix-1.1.4      caps-0.5.6       oci-spec-0.10.0
seccompiler-0.5.0  libseccomp-0.4.0  pentacle-1.1.0
libcontainer-0.7.0  libcgroups-0.7.0  liboci-cli-0.7.0
```

`[M]` Crate metadata and repo activity from the crates.io API (`api/v1/crates/<name>`) and the
GitHub API (`repos/<owner>/<repo>`, `/commits`, `/stats/participation`), fetched 2026-09-04.

`[M]` **A local Rust toolchain exists** (`cargo 1.96.0`, `rustc 1.96.0`, darwin/aarch64). The task
brief said there is no toolchain on either *remote host*, which is true, but I could not use the
local one for the decisive tests either: every question that needs compiling needs a
**Linux x86_64** target with the real kernel headers and a real `clone`. So compile-dependent
claims are still marked UNVERIFIABLE-HERE with a named test for the local VM. What the local
toolchain did buy: cargo's network path works, so I have the real sources rather than
recollections of them.

`[M]` **Correction to my own access assumptions, recorded for the next verifier:** the crates.io
JSON API returns **HTTP 403 without a `User-Agent` header**. `static.crates.io` needs no UA. Any
future agent scripting crates.io must set one.

**Verdict key:** CONFIRMED / CORRECTED / DISPUTED / WEAK-EVIDENCE / UNVERIFIABLE-HERE.

---

# Part 1 — `nix`: does it cover the one place a wrapper would earn its keep?

## 1.1 "`nix` has no `clone3`" — **CONFIRMED**

`[O]` HIGH — `nix-0.31.3`, `grep -rn clone3 src/` → **0 hits**. `grep -rn clone_args src/` → 0
hits. There is no `clone3`, no `struct clone_args`, and no `SYS_clone3` path anywhere in the crate.

`[O]` HIGH — **and the gap is wider than D2 says.** Also **0 hits** for each of:

| Constant / call | hits in `nix-0.31.3/src` | Who needs it |
|---|---|---|
| `clone3` | 0 | D1 §10.1 step 6, D2 §10.2 primary path |
| `CLONE_PIDFD` | **0** | D1 §8.2 / §12 — "keep the pidfd as the container handle" |
| `CLONE_INTO_CGROUP` | **0** | D5 §1 — race-free cgroup placement |
| `CLONE_NEWTIME` | 0 | D1 §1.8 |
| `pidfd_open` / `pidfd_send_signal` / `pidfd_getfd` | **0** | D1 §8.2, D2 §5.4 |
| `close_range` | 0 | D1 §11.2 fd hygiene (step 30) |
| `fsopen`/`fsmount`/`move_mount`/`open_tree`/`fsconfig`/`mount_setattr` | 0 | D1 §4.5, D2 §12.1.2 |

`[O]` HIGH — `nix::sched::CloneFlags` is `libc_bitflags! { pub struct CloneFlags: c_int }` and its
flag list ends at `CLONE_IO`. `CLONE_PIDFD` (0x1000) and `CLONE_INTO_CGROUP` are simply not
declared, and three flags are commented out in-source with *"Not supported by Nix due to lack of
varargs support in Rust FFI"* (`CLONE_SETTLS`, `CLONE_PARENT_SETTID`, `CLONE_CHILD_CLEARTID`).

`[R]` HIGH — **D2's framing is the correct one and I am strengthening it.** The absence is not one
missing function; it is the entire *modern* process-creation and process-handle surface. `nix`
models `clone(2)` as it existed around 2016. Every primitive D1 and D2 independently converged on
as the right design — `clone3`, `CLONE_PIDFD`, `CLONE_INTO_CGROUP`, `pidfd_open` — is absent.

## 1.2 "`nix::sched::clone` takes a `Box<dyn FnMut>` and is unsound for our purpose" — **CONFIRMED**

`[O]` HIGH — `nix-0.31.3/src/sched.rs`. The type alias, verbatim (line 88):

```rust
pub type CloneCb<'a> = Box<dyn FnMut() -> isize + 'a>;
```

and the function, verbatim, with the fatal line marked:

```rust
pub unsafe fn clone(
    mut cb: CloneCb, stack: &mut [u8], flags: CloneFlags, signal: Option<c_int>,
) -> Result<Pid> {
    extern "C" fn callback(data: *mut CloneCb) -> c_int {
        let cb: &mut CloneCb = unsafe { &mut *data };
        (*cb)() as c_int
    }
    let combined = flags.bits() | signal.unwrap_or(0);
    let res = unsafe {
        let ptr = stack.as_mut_ptr().add(stack.len());
        let ptr_aligned = ptr.sub(ptr as usize % 16);
        libc::clone(
            mem::transmute::<…>(callback as extern "C" fn(*mut Box<dyn FnMut() -> isize>) -> i32),
            ptr_aligned as *mut c_void,
            combined,
            &mut cb as *mut _ as *mut c_void,   // <-- &mut to a local in clone()'s own frame
        )
    };
    Errno::result(res).map(Pid::from_raw)
}
```

`[O]` HIGH — **D1's and D2's quotations are both accurate.** `cb` is a `mut` parameter, i.e. a
local of `clone`'s stack frame; `&mut cb` is a pointer into that frame; the child dereferences it
from a different stack. D1's characterisation ("a pointer to a `Box` living in `nix::clone`'s own
stack frame") and D2's ("neither side owns the `Box`, and nothing drops it in the child") are both
correct readings of this code. D2's rendering compresses the `transmute` for readability but
changes nothing material.

`[O]` HIGH — the `signal: Option<c_int>` parameter is OR'd straight into the flags word
(`flags.bits() | signal.unwrap_or(0)`), which is the correct `clone(2)` ABI, so that part is fine.

`[R]` HIGH — **I endorse D2's *reason* over D1's.** D1 rejects the API for unsoundness. D2 rejects
it because *the signature is the wrong shape for the job* — "a closure is an invitation to capture
`String`s, `PathBuf`s and a `tracing` span into the one region of the program where allocation can
deadlock forever." D2's reason is strictly stronger, because it survives nix fixing the soundness
bug. A sound `clone(Box<dyn FnMut>)` would still be the wrong API for a post-`clone` child that
must behave like signal-handler code. Adopt D2's framing.

`[O]` MED — **the soundness-bug age.** I could not read nix issues #360 and #2803 directly (GitHub
issue bodies were not fetched), so D1's "open since 2016-04-19" and D2's citation of youki's
`fork.rs` comment are **WEAK-EVIDENCE** as to the *issue metadata*. But they are not load-bearing:
the code above is the primary evidence and it is unambiguous. `[O]` HIGH — the crate's own
`CHANGELOG.md` shows no `sched::clone` change in 0.31.0 → 0.31.3, so nothing has been fixed in the
current release line.

## 1.3 "Has this changed recently?" (the changelog question) — **CONFIRMED, nothing has changed**

`[O]` HIGH — `nix-0.31.3/CHANGELOG.md`, the 0.31.x entries in full, are: Cygwin `ioctl`, NetBSD
clock constants, Redox `mkfifo`, Apple `kevent64`, Redox `SaFlags_t`, two `from_bits_retain` panic
fixes (epoll/kqueue), FreeBSD `pthread_getthreadid_np`, FreeBSD `timerfd`, `WatchDescriptor::as_raw`,
a libc version bump, and two termios constants. **Not one line touches `sched`, `clone`, or
process creation.** The 0.31 line is BSD/Redox/Cygwin portability work.

## 1.4 `nix`'s maintenance status — **CORRECTED: worse than D2 recorded, and D2's figures were right when written**

`[O]` HIGH — GitHub API, `nix-rust/nix`, fetched today: **stars 3073, open issues 354, pushed_at
2026-05-19, archived = false**. Those are D2's exact numbers, so D2's data collection was accurate.

`[O]` HIGH — `/commits?per_page=10`: the ten most recent commits on `master` are dated
**2026-05-11 … 2026-05-19**. The newest commit in the repository is **2026-05-19**.

`[R]` HIGH — **from 2026-05-19 to today (2026-09-04) is 15.4 weeks, not 13.** D2 measured 13 weeks
of silence and asked "decline or a release gap?" The answer available now: **it has continued.**
Two and a half more months elapsed with zero commits. This is not a release gap — a release gap
still has commits landing on `master` behind it. `[O]` HIGH — there is no `master` activity at all.

`[O]` HIGH — corroborating signal from a third party: **youki pins `nix = "0.29.0"`** (D2 §12.4
finding 6) while 0.31.3 shipped 2026-05-11. `[O]` HIGH — verified independently in
`libcontainer-0.7.0/Cargo.toml` (see Part 5). A downstream project that *does* actively maintain
its deps has not moved. That is two-minor lag on a foundational crate.

`[O]` HIGH — **counter-evidence, stated fairly.** `nix` is not dying by usage: 770,306,911 all-time
downloads and **170,408,957 in the last 90 days**. It has 354 open issues but is not archived, and
the 0.31.x releases were regular until they stopped (0.31.0 2026-01-22 → .1 2026-01-23 → .2
2026-02-28 → .3 2026-05-11). D2's read — "29 distinct authors over 52 weeks says a healthy
contributor base with no one steering" — matches what I see: a crate with contributors and no
maintainer bandwidth.

`[R]` HIGH — **Verdict on maintenance: DISPUTED-in-D2's-favour, upgraded from "caution flag" to
"do not put this in the foundation."** D2 called it "a caution flag, not a disqualification." I
agree that is the right *category*, and I note the additional 2.5 months makes it a firmer flag.
But maintenance is not the reason to reject `nix` and should not be the argued reason — §1.1 is.
A perfectly maintained `nix` still lacks `clone3`, `CLONE_PIDFD` and `pidfd_open`.

## 1.5 **CORRECTION TO D2: `nix` *does* have `openat2`.**

`[O]` HIGH — D2 §12.1.2's table says `openat2` + `RESOLVE_*` is **absent** from `nix` and present
only in `rustix`, and D2 §12.1.7 justification 2 lists `openat2` among "four things we specifically
asked for and `nix` has none of them." **Both statements are false.**

`[O]` HIGH — `nix-0.31.3/src/fcntl.rs:389`:

```rust
pub fn openat2<P: ?Sized + NixPath, Fd: std::os::fd::AsFd>(
    dirfd: Fd, path: &P, mut how: OpenHow,
) -> Result<OwnedFd>
```

with a full `ResolveFlag` bitflags set (`RESOLVE_NO_SYMLINKS`, `RESOLVE_NO_XDEV`,
`RESOLVE_BENEATH`, `RESOLVE_IN_ROOT`, `RESOLVE_NO_MAGICLINKS`), an `OpenHow(libc::open_how)`
newtype correctly zero-initialised via `MaybeUninit::zeroed()` with the man page's requirement
quoted in a comment, and a builder (`.flags()`, `.mode()`, `.resolve()`). It returns `OwnedFd`. It
is implemented as `libc::syscall(libc::SYS_openat2, …)`. `[O]` HIGH —
`grep -n openat2 CHANGELOG.md` → *"Added support for openat2 on linux."*

`[R]` HIGH — **CORRECTED.** D2's argument 2 loses one of its four legs. This does **not** change
the verdict (the other three legs — `pidfd_open`, the new mount API, named prctls — hold, and §1.1
is decisive on its own), but the document must not claim `nix` lacks `openat2`, because that claim
is checkable and wrong, and it is the kind of error that makes a reader distrust the rest.

## 1.6 **CONFIRMATION FOR D4: `nix::fcntl::Flock` exists and is an RAII type — with a footgun D4 did not flag.**

`[O]` HIGH — `nix-0.31.3/src/fcntl.rs`: `pub struct Flock<T: Flockable>(T)` with
`Flock::lock(t, FlockArg) -> Result<Self, (T, Errno)>` (note: returns the value back on failure —
a genuinely good API), `Deref`/`DerefMut` to the inner fd type, and `unsafe trait Flockable`. The
plain `fcntl::flock(fd, arg)` function is `#[deprecated(since = "0.28.0", note = "fcntl::Flock
should be used instead.")]`. So D4's claim that `nix::fcntl::Flock` is a better `fs2` is
**CONFIRMED** on API quality.

`[O]` HIGH — **but read its `Drop`:**

```rust
impl<T: Flockable> Drop for Flock<T> {
    fn drop(&mut self) {
        let res = Errno::result(unsafe { libc::flock(self.0.as_raw_fd(), libc::LOCK_UN) });
        if res.is_err() && !std::thread::panicking() {
            panic!("Failed to remove flock: {}", res.unwrap_err());
        }
    }
}
```

`[R]` HIGH — **`Flock` panics in `Drop`.** For a container engine this is a real hazard: a
`Flock` alive across a `clone()` means the child's `Drop` (on any error path that is not
`_exit`) can `panic!` — and D1 §11.2 and D2 §8.1 both forbid panicking in that window, because
the panic machinery allocates and formats. It also means an `EBADF` from an fd closed out from
under it turns a recoverable error into an abort. This is not a reason to reject `Flock`; it is a
reason to keep it strictly parent-side and out of anything that forks, which is where D4 wants it
anyway (state-directory locking). **Flagging it because nobody did.**

---

# Part 2 — `rustix`: scope, the `runtime` trap, allocation, bus factor

## 2.1 The `not_implemented.rs` boundary — **CONFIRMED, exactly as D2 quotes**

`[O]` HIGH — `rustix-1.1.4/src/not_implemented.rs`, module header, verbatim:

> *"Most Rust programs have a libc present, and when a libc is present, it expects to be the only
> thing in the process that can do certain operations. For example, there can be only one `atexit`
> list in a process, only one `pthread_atfork` list in a process, only one implementation of
> pthreads in a process…"*

`[O]` HIGH — I enumerated the module contents programmatically. `pub mod libc_internals` contains
**147** entries. The ones that matter to us, present exactly as D2 claims:

```
exit, fork, clone, clone3, brk, sigaction, sigaltstack, sigprocmask,
sigwait, sigwaitinfo, sigtimedwait, set_thread_area, set_tid_address, tkill,
sched_setscheduler, rseq, setuid, setgid, seteuid, setegid, setreuid, setregid,
setresuid, setresgid, setgroups, pthread_atfork, + 121 pthread_* entries
```

`[O]` HIGH — `pub mod quite_yet` contains exactly **7**: `_exit`, `_Exit`, `exit_group`,
`sigpending`, `sigsuspend`, `execveat`, `execve`.

`[O]` HIGH — `pub mod yet` contains **30**, including `signalfd` and **`mount_setattr`**.
`pub mod impossible` contains `vfork, sigreturn, setjmp, longjmp, sigsetjmp, siglongjmp`.
`pub mod memory_allocation` contains the seven malloc-family functions.

`[R]` HIGH — **CONFIRMED in every particular.** D2's claim 3 second half is accurate: `fork` /
`clone` / `clone3` / `sigprocmask` / `setuid` are deliberately out of scope (`libc_internals`),
while `execve` / `_exit` are "not quite yet" (`quite_yet`). D1's separate claim that rustix lacks
`mount_setattr` is also **CONFIRMED** — it is in `yet`.

## 2.2 The `rustix::runtime` UB warning — **CONFIRMED verbatim, with one precision correction**

`[O]` HIGH — `rustix-1.1.4/src/runtime.rs`, lines 1–45. The actual doc comment, verbatim and
complete on the points at issue:

> ```
> //! Experimental low-level implementation details for libc-like runtime
> //! libraries such as [Origin].
> //!
> //! ⚠ These are not normal functions. ⚠
> //!
> //!  - Some of the functions in this module cannot be used in a process which
> //!    also has a libc present. This can be true even for functions that have
> //!    the same name as a libc function that Rust code can use. Such functions
> //!    are not marked `unsafe` (unless they are unsafe for other reasons), even
> //!    though they invoke Undefined Behavior if called in a process which has a
> //!    libc present.
> //!  …
> //!  - The API for these functions is not considered stable, and this module is
> //!    `doc(hidden)`.
> //!
> //! ⚠ Caution is indicated. ⚠
> ```
>
> and, in its `# Safety` section:
>
> ```
> //! This module is intended to be used for implementing a runtime library such
> //! as libc. Use of these features for any other purpose is likely to create
> //! serious problems.
> ```

`[R]` HIGH — **D2's quotation is verbatim-accurate.** I checked it word for word.

`[O]` HIGH — the module does export what D2 says: `kernel_fork`, `execve`, `execveat`,
`kernel_sigprocmask`, `kernel_sigaction`, `exit_group`, `kernel_sigwait`, plus
`kernel_sigaltstack`, `kernel_sigpending`, `kernel_sigsuspend`, `kernel_sigwaitinfo`,
`kernel_sigtimedwait`, `kernel_brk`, `tkill`, `set_tid_address`, `exit_thread`, `set_fs`,
`exe_phdrs`, `entry`, `random`, `linux_secure`.

`[O]` HIGH — **the one precision correction.** The header says "**Some** of the functions in this
module cannot be used in a process which also has a libc present" — it is a blanket warning over
an unspecified subset, and the per-function docs for `execve`/`execveat` do **not** individually
repeat it (they say only *"Taking raw-pointers-to-raw-pointers is convenient for c-scape, but we
should think about potentially a more Rust-idiomatic API if this is ever made public"*). So the
precise sentence "`rustix::runtime`'s `kernel_fork`/`execve` are documented as UB when a libc is
present" is **slightly overstated**: what is documented is that an unspecified subset of the
module is, and the module is `doc(hidden)`, unstable, and self-described as for libc
implementations only.

`[R]` HIGH — **the practical ruling is unchanged and I strengthen it: never use `rustix::runtime`.**
The reason is better than "it's UB": it is `doc(hidden)`, explicitly not API-stable, and the
author's stated audience is Origin/Eyra/c-scape. A foundation crate's unstable hidden module is
the worst possible place to put the single most important unsafe call in the project. D2's
`[R]` HIGH stands. Restate the *reason* as "unstable, hidden, wrong audience, plus a blanket UB
warning" rather than "execve is documented UB", so the claim is exactly as strong as the evidence.

`[O]` HIGH — **worth quoting, because it is the best statement of D1 §11.2 I found anywhere,
written by the author of our foundation crate.** `runtime.rs`'s `kernel_fork` doc:

> *"The program environment in the child after a `fork` and before an `execve` is very special.
> All code that executes in this environment must avoid: — Acquiring any other locks that are held
> in other threads on the parent at the time of the `fork`… — Performing any dynamic allocation
> using the global allocator, since global allocators may use locks to ensure thread safety, and
> their locks may not be released in the child process, so attempts to allocate may deadlock…
> — Accessing any thread runtime state, since this function does not update the thread id in the
> thread runtime… — Calling any C function which isn't known to be async-signal-safe… **But, what
> does async-signal-safe even mean in a Rust program? This documentation does not have all the
> answers. So you're on your own.**"*

`[R]` HIGH — **this independently corroborates D1 §11.2 and D2 §8.1 from a source neither cited**,
and it does so in the *Rust* register rather than the POSIX one. D1 §11.2 marks the
Rust-specific mapping `[F]` MED and says "what would settle it is `library/std/src/sys/alloc/unix.rs`."
This is not that file, but it is the author of `rustix` stating the same conclusion as a
requirement on callers. `[R]` — cite it in D1 §11.2; it upgrades that `[F]` MED materially.

## 2.3 rustix coverage claims — **MOSTLY CONFIRMED, one hard CORRECTION**

`[O]` HIGH — verified by grepping `rustix-1.1.4/src` for `pub fn` definitions:

| D2 claims rustix has | Reality | Verdict |
|---|---|---|
| `pidfd_open` | `src/process/pidfd.rs:29` `pub fn pidfd_open(pid: Pid, flags: PidfdFlags) -> io::Result<OwnedFd>` | CONFIRMED |
| `pidfd_send_signal` | `src/process/pidfd.rs:41` | CONFIRMED |
| `pidfd_getfd` | `src/process/pidfd_getfd.rs:50` | CONFIRMED |
| `openat2` | `src/fs/openat2.rs:14` `pub fn openat2<Fd: AsFd, P: path::Arg>(…)` | CONFIRMED |
| `set_child_subreaper` | `src/process/prctl.rs:866`; `child_subreaper()` at :849 | CONFIRMED |
| new mount API | `src/mount/fsopen.rs`: `fsopen`, `fsmount`, `move_mount`, `open_tree`, `fspick`, `fsconfig_set_flag`, `fsconfig_set_string`, `fsconfig_set_binary`, `fsconfig_set_path`, `fsconfig_set_path_empty`, `fsconfig_set_fd`, `fsconfig_create`, `fsconfig_reconfigure`, `fsconfig_create_exclusive` — **14 functions, the complete set** | CONFIRMED, and D1's "best new-mount-API coverage of any crate" is fair |
| `pivot_root` | `src/process/pivot_root.rs:14` | CONFIRMED |
| `unshare` / setns | `src/thread/setns.rs`: `unshare(UnshareFlags)`:141, `move_into_link_name_space`:112, `move_into_thread_name_spaces`:130 | CONFIRMED |
| `set_no_new_privs` | `src/thread/prctl.rs:631` | CONFIRMED |
| `set_parent_process_death_signal` | `src/process/prctl.rs:66` | CONFIRMED |
| capability sets | `src/thread/libcap.rs`: `capabilities`:123, `set_capabilities`:136; `src/thread/prctl.rs`: `capability_is_in_bounding_set`:467, `remove_capability_from_bounding_set`:489, `clear_ambient_capability_set`:725, `configure_capability_in_ambient_set`:739 | CONFIRMED |
| **`close_range`** | **DOES NOT EXIST** | **CORRECTED** |

### 2.3.1 **`rustix::io::close_range` does not exist. D1 was right; D2 is wrong, in three places.**

`[O]` HIGH — `grep -rin close_range` over the **entire** `rustix-1.1.4` crate (not just `src`)
returns **zero hits**. `src/io/mod.rs` exports exactly `close`, `dup`, `errno`, `fcntl`, `ioctl`,
`read_write`; `src/io/close.rs` contains only `pub unsafe fn close(raw_fd: RawFd)` and a
feature-gated `try_close`. The `not_implemented.rs` module does not list `close_range` either — it
lists BSD's `closefrom()` under `higher_level`, pointing at `rustix-openpty`.

`[R]` HIGH — **D2 asserts `rustix::io::close_range` in three places and every one is wrong:**

1. §12.5, `close_fds`/`command-fds` row: *"`rustix::io::close_range` covers it. One syscall, no
   crate."*
2. §12.6 take-table, `rustix` row: *"it has `pidfd_open`, `openat2`, `close_range`,
   `set_child_subreaper` and the new mount API that `nix` does not."*
3. §12.6 avoid-table, `close_fds`/`command-fds` row: *"Instead: `rustix::io::close_range` — Lines:
   1."*

`[O]` HIGH — **D1 §11.1 states the truth**: *"Missing: `mount_setattr` (PRs #975/#1002/#1630 open)
and **`close_range`**."* And D1 §11.1 goes further, correctly: *"A `close_range` crate does not
exist."*

`[R]` HIGH — **this is the single most consequential factual error I found in the substrate
material, and it lands in the worst place: the post-`clone` window.** D1 §11.2 makes the
`close_range` sweep **step 30, the last action before `execve`**, in the child, where allocation
and error formatting are forbidden. A developer following D2 §12.6 writes
`rustix::io::close_range(3, u32::MAX, CloseRangeFlags::CLOEXEC)`, gets a compile error, and then —
at the worst moment, in the least reviewed code — improvises. The improvisation that fails
silently is the one that iterates `/proc/self/fd` (a `readdir`, which allocates, and which is
mutating the very fd table it is iterating).

`[R]` HIGH — **the fix, and it is small.** `close_range` is syscall **436** on x86_64.
`libc::close_range` exists but D1 §11.1 correctly notes it is declared **gnu-only** — on
`x86_64-unknown-linux-musl` (the target §7.3 wants) it is not declared at all. So the correct entry
is:

```rust
// close_range(2), Linux >= 5.9. Not in rustix (verified absent in 1.1.4);
// libc::close_range is glibc-only, and we target musl. syscall 436.
// CLOSE_RANGE_CLOEXEC = 2 — mark cloexec rather than close, so a live Rust
// `File`'s Drop cannot later close an unrelated fd (D1 §11.2).
unsafe { libc::syscall(436, 3usize, u32::MAX as usize, 2usize) }
```

**This makes the `libc` surface six calls, not five.** D2's §12.1.7 list must gain a line. That is
the correct outcome — D2's own rule is *"one syscall layer, plus a written-down list of the raw
calls that layer refuses to make."* The list was simply short by one because a function was
believed to exist that does not.

`[O]` HIGH — **and D2's empirical check of `close_range` was sound**, which is why the error
survived: §12.5 records the ctypes probe `libc.syscall(436, 0, 0, 0)` returning 0 on
`my-droplet` kernel 6.11.0-29. The *syscall* is present. Only the *rustix wrapper* is imaginary.

## 2.4 `linux_raw` is the default backend on x86_64 — **CONFIRMED, from `build.rs`**

`[O]` HIGH — `rustix-1.1.4/build.rs`, lines 108–133. The backend decision, verbatim:

```rust
let libc = feature_use_libc
    || cfg_use_libc
    || os != "linux"
    || !inline_asm_name_present
    || is_unsupported_abi
    || miri
    || ((arch == "powerpc" || arch == "powerpc64" || arch == "s390x"
         || arch.starts_with("mips")) && !rustix_use_experimental_asm);
if libc { … use_feature("libc"); } else { use_feature("linux_raw"); }
```

`[R]` HIGH — on `x86_64-unknown-linux-{gnu,musl}`, none of those disjuncts is true, so
`linux_raw` is selected. **CONFIRMED.** Note the escape hatches for the record:
`--cfg=rustix_use_libc`, the `use-libc` cargo feature, `--cfg=rustix_no_linux_raw`, and **`miri`**
— under Miri rustix silently switches to the libc backend, which matters if we ever try to Miri
the child path.

`[O]` HIGH — `Cargo.toml` confirms D2's dependency claim: `linux-raw-sys 0.12` with features
`auxvec, general, errno, ioctl, no_std, elf`; `bitflags`; and `libc` present but **`optional =
true`**, pulled only by the `use-libc` feature. So on our target rustix's own graph is two crates.

## 2.5 "`linux_raw` never touches the inherited `errno` TLS" — **CONFIRMED**

`[O]` HIGH — this is D2's strongest technical argument, so I tested it directly rather than
reasoning about it. `grep -rn "errno_location\|__errno\|thread_local\|#\[thread_local\]"` over
**`src/backend/linux_raw/`** returns **zero hits**. There is no TLS access of any kind in the
linux_raw backend.

`[O]` HIGH — and the positive evidence for how errors actually travel,
`src/backend/linux_raw/io/errno.rs`:

```rust
#[repr(transparent)]
#[doc(alias = "errno")]
// Linux returns negated error codes, and we leave them in negated form, so
// error codes are in `-4095..0`.
#[cfg_attr(rustc_attrs, rustc_layout_scalar_valid_range_start(0xf001))]
#[cfg_attr(rustc_attrs, rustc_layout_scalar_valid_range_end(0xffff))]
pub struct Errno(u16);
```

`[R]` HIGH — **CONFIRMED, and D2's reasoning is correct and non-obvious.** `Errno` is a
`#[repr(transparent)] u16` decoded from the syscall's return register (`RetReg`), with the value
left in the kernel's negated form and a `rustc_layout_scalar_valid_range` attribute proving the
author intends it as a register value. There is no `errno` variable to be stale in the child, and
no `std::io::Error` to allocate. This is a real, mechanical property, not a marketing claim, and
D2's observation that *"this is not why most people pick rustix"* is fair.

`[R]` MED — **one honest deflation.** D1 §11.2's own analysis (`[F]` MED) points out that after a
`fork`/`clone` **without `CLONE_VM`**, the child's TLS is a private copy, so a libc `errno` read
would be *defined* (if possibly stale-looking). So the errno-TLS argument is a robustness
argument, not a soundness one, for the non-`CLONE_VM` case we actually use. It removes a category
of confusing bug rather than a category of UB. It is still worth having; it should not be sold as
preventing UB.

## 2.6 Does any rustix function in the child subset allocate? — **CORRECTED to more precise, and D2's mitigation is AIRTIGHT**

D2 flags `path::Arg` as a MED risk "mooted by passing pre-built `CString`s." I read the code.

`[O]` HIGH — **the mitigation is exactly airtight.** `rustix-1.1.4/src/path/arg.rs`, for
`&CStr` (line 454), `&CString` (492) and `CString` (527), `into_with_c_str` is the identity:

```rust
impl Arg for &CStr {
    fn into_with_c_str<T, F>(self, f: F) -> io::Result<T>
    where F: FnOnce(&CStr) -> io::Result<T> { f(self) }        // no copy, no alloc
}
impl Arg for CString { … { f(&self) } }                        // no copy, no alloc
```

`[R]` HIGH — **CONFIRMED, and stronger than "mooted".** Passing a pre-built `CString` or `&CStr`
makes the path argument a *zero-copy borrow* — not merely a cheap allocation, but literally no
work at all. D2's structural mitigation ("the child receives every path as a pre-built `CString`
prepared by the parent before the clone") is therefore not a mitigation but an elimination.

`[O]` HIGH — **the precise failure mode if you don't, which D2 states too vaguely.** For `&str`,
`String`, `&String`, `&Path` etc., `into_with_c_str` calls the shared helper
`with_c_str(bytes, f)` (`src/path/arg.rs:948`):

- `SMALL_PATH_BUFFER_SIZE` is **256** (`src/path/mod.rs:9`).
- `bytes.len() < 256` → a stack `MaybeUninit<[u8; 256]>`. **No allocation.**
- `bytes.len() >= 256` → `#[cold] with_c_str_slow_path`, which **with the `alloc` feature does
  `CString::new(bytes)` — a heap allocation.** Without `alloc` it uses a stack
  `MaybeUninit<[u8; PATH_MAX]>` instead.

`[O]` HIGH — **and `std` implies `alloc`.** `Cargo.toml`: `std = ["bitflags/std", "alloc",
"libc?/std", "libc_errno?/std"]`, and `default = ["std"]`. D2 §12.6's recommended feature list
begins `features = ["std", …]`, so **the heap slow path is live in D2's own recommended
configuration.**

`[R]` HIGH — **so the risk is real and it is not theoretical for us.** A path like
`/var/lib/mydocker/overlay2/<64-hex-digest>/merged/usr/lib/x86_64-linux-gnu/…` crosses 256 bytes
easily, and D3's CAS layout makes long paths the norm rather than the exception. Passing a `&str`
for a >256-byte path in the child is a `malloc` in the forbidden window. Two things follow:

1. `[R]` HIGH — **D2's pre-built-`CString` discipline is mandatory, not hygienic**, and the
   threshold (256) should be written into the comment so nobody "simplifies" it away.
2. `[R]` MED — **a cheap belt-and-braces option D2 missed:** the child's code path could be a
   separate crate compiled with `rustix = { default-features = false, features = ["mount",
   "process", "thread", …] }` — i.e. **`alloc` off**. Then `with_c_str_slow_path` uses a
   `PATH_MAX` stack buffer and there is no `malloc` reachable from any rustix path argument, at
   any length. Cost: no `std` in that crate, which is a large constraint (no `std::fs`, no
   `String` at all) and probably not worth it for phase 1. Record it as the hardening move if we
   ever get a real deadlock.

`[O]` MED — **coverage limit, stated honestly.** I verified the `Arg` impls exhaustively and read
`libcap.rs`, `close.rs`, `errno.rs`, `not_implemented.rs`, `runtime.rs`, `build.rs`,
`mount/fsopen.rs` signatures, and `path/arg.rs` in full. I did **not** read the bodies of all ~40
functions in our subset. D2's own confidence on this was `[O]` MED for the same reason. The
non-path-argument risk surface is small — every function I read returns `io::Result<T>` with
`Errno` as the error, and `Errno` is a `u16` — but "no rustix function in our subset allocates"
remains **WEAK-EVIDENCE** as a universal claim. The test that settles it is in Part 8.

## 2.7 rustix's bus factor — **CONFIRMED as a fact, and I weigh it differently to D2**

`[O]` HIGH — GitHub API, `bytecodealliance/rustix`: **stars 2090, open issues 145, pushed_at
2026-08-25, archived = false**. D2's numbers again exact.

`[O]` MED — **I could not independently re-derive "sunfishcode authored 22 of the last 61
commits."** `/stats/participation` returns weekly totals without per-author attribution, and the
per-author breakdown D2 used (`/stats/contributors`) is a computed endpoint that returns 202 while
GitHub builds it. So the specific 22/61 split is **WEAK-EVIDENCE — I accept it as plausible and
consistent with everything else, but I did not confirm the arithmetic.** What I did confirm:
`rustix 1.1.4`'s `Cargo.toml` lists exactly two authors, `Dan Gohman <dev@sunfishcode.online>` and
`Jakub Konka`, and the repo has been pushed within the last 10 days.

`[R]` HIGH — **how I weigh it, which differs from D2 in emphasis.** D2 lists bus factor as
"cost 2" and mitigates it with download volume. I think that mitigation is stated backwards, and
the correct framing is better for us:

- `[O]` HIGH — **1,097,873,909 all-time downloads; 244,167,863 in the last 90 days.** rustix is a
  transitive dependency of `tempfile`, `procfs`, `is-terminal`, `terminal_size` and much of the
  tooling ecosystem. It is load-bearing infrastructure for a large fraction of all Rust builds.
- `[R]` HIGH — that does not mean *the maintainer* will be replaced if he stops. It means
  **something will happen**: a fork, a bytecodealliance handover, or a `libc`-backed shim. The
  ecosystem cannot afford rustix to be unmaintained, and it lives in a foundation org, not a
  personal account. Compare `nix`: 29 contributors, no steering, 15 weeks silent, and the
  ecosystem *is* absorbing that without drama because the consumers pin old versions.
- `[R]` HIGH — **and the real mitigation is architectural, not social.** rustix's value to us is
  concentrated in ~40 thin syscall wrappers. If rustix died tomorrow, replacing our subset is a
  few hundred lines of `libc::syscall` — which is precisely what D2's "written-down list of raw
  calls" discipline already prepares us for. The bus-factor exposure on a *thin syscall wrapper*
  is categorically smaller than on, say, an async runtime or an HTTP stack. That is the argument
  D2 should make, and it is stronger than the download count.

`[R]` MED — **the cost I would raise above D2's ranking:** `linux_raw` is **per-architecture inline
`asm!`**. D2 lists this as cost 3 and waves at it. The project brief says "x86_64 first but avoid
needless architecture coupling." Taking `linux_raw` means our syscall correctness on aarch64 is
rustix's asm quality on aarch64, which is a smaller-tested path than x86_64. That is an acceptable
bet (aarch64 is well covered), but it should be recorded as the coupling it is: **`linux_raw` is
where our architecture portability actually lives.**

## 2.8 rustix advisory history — **CONFIRMED (see Part 7 for the advisory-by-advisory audit)**

`[O]` MED — D2's table claims rustix has **0** RUSTSEC advisories ever, and `nix` has exactly one
(RUSTSEC-2021-0119, `getgrouplist` out-of-bounds write). I verify these against the advisory DB in
Part 7.

`[R]` HIGH — D2's own hedge on this is the right one and should be kept: *"Zero advisories in
rustix's lifetime is weak evidence on its own (fewer years, fewer eyes on the security surface)."*
I agree, and I would go one step further: rustix has **1,792 occurrences of `unsafe`** by D2's
count versus `nix`'s 926, and carries per-arch inline asm. Zero advisories on that surface is
mildly surprising, not reassuring. The *structural* argument (no libc buffer contracts to misread
on the Linux path) is the one that carries weight; the advisory count is decoration.

---

# Part 3 — `libc` and the raw-syscall surface: is it five calls or two hundred lines?

`[M]` Source read this session: **`libc-0.2.191`** (released 2026-10-16 — note this is *newer* than
the 0.2.189 D1 §11.1 tabulated and the 0.2.x D2 assumed), downloaded from `static.crates.io` and
extracted. Kernel side verified against **`torvalds/linux@v6.11`** — pinned to the tag, and chosen
because it is `my-droplet`'s exact kernel (6.11.0-29).

## 3.1 The structs: are they present, and are they right? — **CONFIRMED, 3 of 4**

`[O]` HIGH — exhaustive grep over `libc-0.2.191/src`:

| Type | Present? | Where | Verdict |
|---|---|---|---|
| `struct clone_args` | **yes** | `src/unix/linux_like/linux/{gnu,musl}/b64/x86_64/mod.rs` (+ aarch64, riscv64, loongarch64, powerpc64) | CONFIRMED |
| `struct mount_attr` | **yes** | `src/new/linux_uapi/linux/mount.rs:60` | CONFIRMED |
| `struct open_how` | **yes** | `src/unix/linux_like/linux/mod.rs:353` | CONFIRMED, with a construction trap |
| `__user_cap_header_struct` / `__user_cap_data_struct` | **NO — zero hits anywhere** | — | **CONFIRMED-ABSENT** |

### 3.1.1 `clone_args` — correct, size-versioned, and **64-bit only**

`[O]` HIGH — `libc-0.2.191/src/unix/linux_like/linux/musl/b64/x86_64/mod.rs:121` (identical in the
`gnu` tree at `b64/x86_64/mod.rs:236`), inside an `s!` block so it gets `#[repr(C)]` + `Copy`/`Clone`:

```rust
#[repr(align(8))]
pub struct clone_args {
    pub flags: c_ulonglong,       pub pidfd: c_ulonglong,
    pub child_tid: c_ulonglong,   pub parent_tid: c_ulonglong,
    pub exit_signal: c_ulonglong, pub stack: c_ulonglong,
    pub stack_size: c_ulonglong,  pub tls: c_ulonglong,
    pub set_tid: c_ulonglong,     pub set_tid_size: c_ulonglong,
    pub cgroup: c_ulonglong,
}
```

`[O]` HIGH — eleven `u64` fields, `repr(align(8))` → `size_of::<clone_args>() == 88`. The kernel at
`v6.11/kernel/fork.c:2937-2942` asserts exactly this:

```c
BUILD_BUG_ON(offsetofend(struct clone_args, tls)          != CLONE_ARGS_SIZE_VER0);  /* 64 */
BUILD_BUG_ON(offsetofend(struct clone_args, set_tid_size) != CLONE_ARGS_SIZE_VER1);  /* 80 */
BUILD_BUG_ON(offsetofend(struct clone_args, cgroup)       != CLONE_ARGS_SIZE_VER2);  /* 88 */
BUILD_BUG_ON(sizeof(struct clone_args)                    != CLONE_ARGS_SIZE_VER2);
```

So libc's layout is field-for-field the kernel's current UAPI. **CONFIRMED.**

`[O]` HIGH — **but `clone_args` is absent from every 32-bit target.** `grep -rln "struct clone_args"`
over `src/unix/linux_like/linux/{gnu,musl}/b32/` returns **nothing**: not `x86`, not `arm`, not
`riscv32`. Irrelevant to us (x86_64 first) but it means `libc::clone_args` is **not** a portable
type, and a future i686 or armv7 target would need our own `#[repr(C)]` copy. Record it as a known
edge of the `linux_raw`-style arch coupling flagged in §2.7.

`[O]` HIGH — **libc does not define `CLONE_ARGS_SIZE_VER0/1/2`.** Zero hits. The correct size
argument is therefore `size_of::<libc::clone_args>()`, which is right *because* libc tracks the
newest kernel. §3.3 shows why you must not hand-shrink it.

### 3.1.2 `mount_attr` — correct, and it has only ever had one version

`[O]` HIGH — `libc-0.2.191/src/new/linux_uapi/linux/mount.rs:60`, in an `s!` block:

```rust
pub struct mount_attr {
    pub attr_set: __u64, pub attr_clr: __u64,
    pub propagation: __u64, pub userns_fd: __u64,
}
pub const MOUNT_ATTR_SIZE_VER0: c_int = 32;
```

`[O]` HIGH — **reachability check, because the path looks experimental.** `src/new/mod.rs`'s own
header says *"This module contains the future directory structure … All modules are only
crate-public since we don't reexport this structure."* That sentence is about the module *tree*, not
the items: `src/lib.rs:224` is `mod new;` and `src/lib.rs:235` is `pub use new::*;`. So
`libc::mount_attr`, `libc::MOUNT_ATTR_IDMAP`, `libc::fsconfig_command::FSCONFIG_SET_FD`,
`libc::OPEN_TREE_CLONE`, `libc::MOVE_MOUNT_BENEATH` and the rest **are** at the crate root.
D1 §11.1's *"a UAPI module carrying `struct mount_attr`, all `MOUNT_ATTR_*` (including `_IDMAP`) …"*
is **CONFIRMED** and I add that `MOVE_MOUNT_BENEATH` (0x200, Linux 6.5) is there too, which D1 §4.5
would want for atomic mount replacement.

`[O]` HIGH — the kernel at `v6.11/fs/namespace.c` (`SYSCALL_DEFINE5(mount_setattr)`) asserts
`BUILD_BUG_ON(sizeof(struct mount_attr) != MOUNT_ATTR_SIZE_VER0)`. **`mount_attr` has never been
extended.** So for `mount_setattr` the size argument is unconditionally
`size_of::<libc::mount_attr>() == 32` and the whole versioning question is moot. That is a *simpler*
story than `clone3`'s and worth writing down so nobody invents a version dance that is not needed.

### 3.1.3 `open_how` — present, correct, and **`#[non_exhaustive]`**

`[O]` HIGH — `src/unix/linux_like/linux/mod.rs:353`:

```rust
// linux/openat2.h
#[non_exhaustive]
pub struct open_how { pub flags: __u64, pub mode: __u64, pub resolve: __u64 }
```

`[R]` MED — **`#[non_exhaustive]` means you cannot write `libc::open_how { flags, mode, resolve }`
from our crate. It is a compile error.** You must `let mut how: open_how = mem::zeroed();` and then
assign fields. This is mildly annoying and completely correct: `openat2(2)` requires every unknown
field to be zero, and `nix-0.31.3/src/fcntl.rs` does precisely this (`MaybeUninit::zeroed()`, with
the man page's requirement quoted in a comment — §1.5). Flagging it only because "struct is present"
is not the same as "struct is constructible", and the first developer to hit it will think libc is
broken. It is not.

`[O]` HIGH — `size_of::<open_how>() == 24 == OPEN_HOW_SIZE_VER0 == OPEN_HOW_SIZE_LATEST`
(`v6.11/fs/open.c:1456-1457`). Note `openat2` is the one of the three that does **not** have a
`usize > PAGE_SIZE → -E2BIG` guard; it relies entirely on `copy_struct_from_user`.

### 3.1.4 Capability structs — **absent, and so are the constants and the wrappers**

`[O]` HIGH — `libc-0.2.191` has:
- **no** `__user_cap_header_struct`, `__user_cap_data_struct`, `cap_user_header_t`, `cap_user_data_t`
- **no** `fn capget` / `fn capset` declaration
- **no** `CAP_*` constants at all (`CAP_SYS_ADMIN`, `CAP_CHOWN`, `CAP_NET_ADMIN` → zero hits)
- only `SYS_capget = 125` and `SYS_capset = 126`.

`[R]` HIGH — **D1 §11.1's two claims here are both CONFIRMED and I am sharpening them:** not only
are `capget`/`capset` undeclared and `CAP_*` missing, the *argument types* are missing too. Doing
capabilities from bare `libc` is therefore not "one unsafe line each" like `pivot_root` or
`openat2` — it is two `#[repr(C)]` structs, the v3 magic constant `_LINUX_CAPABILITY_VERSION_3`
(`0x20080522`), the two-`__u32` hi/lo split, and a ~40-entry name↔bit table. That is D5 §19.2's
*"maybe 200 lines of `unsafe` we would otherwise write"*, and D5 is right about the number **for a
`libc`-only stack**. §5.1 shows why it is zero lines for ours.

## 3.2 The kernel's size-versioning rule — **CONFIRMED, verbatim, and it is one rule not three**

`[O]` HIGH — `torvalds/linux@v6.11/include/linux/uaccess.h`, `copy_struct_from_user`'s doc comment
and body. The three cases, quoted:

> ```
>  * There are three cases to consider:
>  *  * If @usize == @ksize, then it's copied verbatim.
>  *  * If @usize < @ksize, then the userspace has passed an old struct to a
>  *    newer kernel. The rest of the trailing bytes in @dst (@ksize - @usize)
>  *    are to be zero-filled.
>  *  * If @usize > @ksize, then the userspace has passed a new struct to an
>  *    older kernel. The trailing bytes unknown to the kernel (@usize - @ksize)
>  *    are checked to ensure they are zeroed, otherwise -E2BIG is returned.
> ```

and the body: `if (usize > ksize) { int ret = check_zeroed_user(src + size, rest); if (ret <= 0)
return ret ?: -E2BIG; }`.

`[R]` HIGH — **the brief's framing ("the kernel rejects unknown-nonzero tail bytes") is CONFIRMED
exactly**, and the operational consequence is the one that matters: **zero-initialise the whole
struct and set only the fields you mean.** A `clone_args` built with `MaybeUninit::uninit()` and
partially filled will hand the kernel garbage in `set_tid`/`set_tid_size`/`cgroup` and get
`-E2BIG` on a pre-5.7 kernel — or, far worse, on a 5.5-vintage kernel will be *accepted* and
interpreted. `core::mem::zeroed()` or `clone_args { ..unsafe { mem::zeroed() } }`, never `uninit`.

`[O]` HIGH — the per-syscall guards around that shared helper:

| syscall | lower bound | upper bound | source |
|---|---|---|---|
| `clone3` | `usize < CLONE_ARGS_SIZE_VER0 (64)` → `-EINVAL` | `usize > PAGE_SIZE` → `-E2BIG` | `kernel/fork.c:2944-2947` |
| `mount_setattr` | `usize < MOUNT_ATTR_SIZE_VER0 (32)` → `-EINVAL` | `usize > PAGE_SIZE` → `-E2BIG` | `fs/namespace.c` |
| `openat2` | `usize < OPEN_HOW_SIZE_VER0 (24)` → `-EINVAL` | **none** | `fs/open.c:1459` |

## 3.3 Two `clone3` traps the D-files do not mention — **NEW**

`[O]` HIGH — **Trap 1: `CLONE_INTO_CGROUP` requires the *full* `size_of`.** `kernel/fork.c`, inside
`copy_clone_args_from_user`:

```c
if ((args.flags & CLONE_INTO_CGROUP) &&
    (args.cgroup > INT_MAX || usize < CLONE_ARGS_SIZE_VER2))
	return -EINVAL;
```

`[R]` HIGH — so an implementation that "plays it safe" by passing `CLONE_ARGS_SIZE_VER0` (64) to
maximise kernel compatibility **silently loses D5 §1's race-free cgroup placement**, with `-EINVAL`
and no hint why. Always pass `size_of::<clone_args>()`. The compatibility worry it was trying to
solve does not exist: §3.2's rule already handles an older kernel, provided the tail is zeroed.

`[O]` HIGH — **Trap 2: the exit signal moves.** `clone(2)` ORs the termination signal into the flags
word — `nix::sched::clone` does exactly that (§1.2: `flags.bits() | signal.unwrap_or(0)`), and it is
correct *for `clone`*. `clone3` **rejects** it. `kernel/fork.c`, `clone3_args_valid`:

```c
/* - make the CLONE_DETACHED bit reusable for clone3
   - make the CSIGNAL bits reusable for clone3  */
if (kargs->flags & (CLONE_DETACHED | (CSIGNAL & (~CLONE_NEWTIME))))
	return false;      /* -> -EINVAL */
```

`[R]` HIGH — `CSIGNAL` is `0xff`; `CLONE_NEWTIME` is `0x80` and is carved out of that range. So with
`clone3` the signal goes in **`args.exit_signal = SIGCHLD as u64`** and the low byte of `flags` must
be clear. Anyone transliterating a `clone()` call into a `clone3()` call gets `EINVAL` and no
explanation. Worth one comment line in the ~15 lines of §10.2's unsafe block, which D2 already
insists be read line by line.

`[O]` HIGH — a third constraint for the record: `if ((kargs->flags & (CLONE_THREAD | CLONE_PARENT))
&& kargs->exit_signal) return false;` — so D2 §10.2's `as_sibling`/`CLONE_PARENT` reading of youki
(its §14 item 18) must pair `CLONE_PARENT` with `exit_signal = 0`.

## 3.4 **`libc::CLONE_INTO_CGROUP` is zero on every glibc target, and missing on every musl target.** — **CORRECTED, and this is a live landmine**

`[O]` HIGH — `libc-0.2.191/src/unix/linux_like/linux/gnu/mod.rs:766-780`, verbatim:

```rust
// linux/sched.h
pub const CLONE_NEWTIME: c_int = 0x80;
// DIFF(main): changed to `c_ulonglong` in e9abac9ac2. This is broken so should be fixed.
#[allow(overflowing_literals)]
#[deprecated(since = "0.2.188", note = "This constant overflows. In the near future, `libc` will
             change to a wider type, see #3584.")]
pub const CLONE_CLEAR_SIGHAND: c_int = 0x100000000;
#[allow(overflowing_literals)]
#[deprecated(since = "0.2.188", note = "This constant overflows. …")]
pub const CLONE_INTO_CGROUP: c_int = 0x200000000;
```

`[O]` HIGH — `c_int` is `i32` on every Linux target. `0x200000000` is 2³³ and `0x100000000` is 2³².
Both are out of range; the deny-by-default `overflowing_literals` lint is suppressed in place with
`#[allow]`, so they compile and **truncate to zero**. Demonstrated on the local toolchain
(`rustc 1.96.0`, `/tmp/v7/t/lit.rs`, exact same `#[allow]` + literal + `i32`):

```
CLONE_INTO_CGROUP   = 0 (0x0)
CLONE_CLEAR_SIGHAND = 0 (0x0)
as u64 in a flags word: 0
```

`[O]` HIGH — the same two constants are **correctly** typed `c_ulonglong` in
`src/unix/linux_like/android/mod.rs:2779-2780`. So the bug is glibc-targets-only, and upstream has
already fixed it on `main` (commit `e9abac9ac2`, per libc's own `DIFF(main)` marker) — it has simply
not reached the 0.2.x line, as of 0.2.191 released **2026-10-16**.

`[O]` HIGH — **and on musl there is no constant at all.** `grep -rn "CLONE_INTO_CGROUP"` over
`src/unix/linux_like/linux/musl/` returns nothing. Our stated target is
`x86_64-unknown-linux-musl` (D2 §7.3), so on the target we actually want to ship,
`libc::CLONE_INTO_CGROUP` **does not compile**.

`[R]` HIGH — **three consequences, and the fix is one line.**

1. D5 §1's race-free cgroup placement is the thing at risk. `args.flags = … | libc::CLONE_INTO_CGROUP as u64` on glibc ORs in **zero**: `clone3` succeeds, returns a pid, and the child is in the *parent's* cgroup. There is no error to catch. This is the same failure signature D2 §8.3 calls the worst possible one — it will be blamed on cgroup delegation for a week.
2. The `#[deprecated]` attribute is the saving grace: you get a warning. `#![deny(deprecated)]` in the core crate turns it into a build failure, which is the §12.6 "structural beats disciplined" move applied to a dependency's bug.
3. **Define it ourselves. It costs one line and it is the same line on gnu and musl:**

```rust
// linux/sched.h. NOT libc::CLONE_INTO_CGROUP: that constant is `c_int` on glibc
// targets and truncates to 0 (libc 0.2.191, issue #3584, fixed on main in e9abac9ac2),
// and is undefined entirely on musl. Verified by V7 against libc-0.2.191 source.
const CLONE_INTO_CGROUP: u64 = 1 << 33;   // 0x200000000
const CLONE_CLEAR_SIGHAND: u64 = 1 << 32; // 0x100000000
```

`[O]` HIGH — `libc::CLONE_PIDFD` is fine: `src/unix/linux_like/linux/mod.rs:3281`,
`c_int = 0x1000`, in range, not deprecated. Only the two above-32-bit flags are affected, and they
are affected *because* they are above 32 bits — which is also why `nix`'s
`CloneFlags: c_int` (§1.1) could never have carried them either.

## 3.5 What `libc` *does* declare that we need — **CONFIRMED**

`[O]` HIGH — function declarations present (checked by grep for `pub fn <name>(`):
`execve`, `execveat`, `_exit(status) -> !`, `sigprocmask`, `signalfd`, `prctl(option, ...)`,
`syscall(num, ...)`, `setresuid`, `setresgid`, `setgroups`, `mount`, `umount2`, `unshare`, `setns`,
`waitid`, `sethostname`. D1 §11.1's list is **CONFIRMED** including its own correction that `setns`
*is* declared.

`[O]` HIGH — `prctl` constants all present and correct: `PR_SET_NO_NEW_PRIVS = 38`,
`PR_CAPBSET_READ = 23`, `PR_CAPBSET_DROP = 24`, `PR_SET_KEEPCAPS = 8`, `PR_CAP_AMBIENT = 47`,
`PR_CAP_AMBIENT_RAISE = 2`, `PR_CAP_AMBIENT_CLEAR_ALL = 4`, `PR_SET_CHILD_SUBREAPER = 36`,
`PR_SET_PDEATHSIG = 1`, `PR_SET_DUMPABLE = 4`. So D5 §8.5's whole ordering is expressible in
`libc` constants even though §5.1 shows rustix already wraps most of it.

`[O]` HIGH — `SYS_*` constants on **`x86_64-unknown-linux-musl`** specifically, since that is the
target where libc's *function* coverage is thinnest. All present:

```
SYS_clone3 435   SYS_close_range 436   SYS_mount_setattr 442   SYS_capget 125   SYS_capset 126
SYS_openat2 437  SYS_pivot_root 155    SYS_open_tree 428       SYS_move_mount 429
SYS_fsopen 430   SYS_fsconfig 431      SYS_fsmount 432         SYS_pidfd_open 434
SYS_pidfd_getfd 438  SYS_pidfd_send_signal 424   SYS_execveat 322   SYS_seccomp 317
```

`[R]` HIGH — **this improves §2.3.1's `close_range` snippet.** I wrote `libc::syscall(436, …)` with
the number hard-coded because `libc::close_range` is gnu-only. That was unnecessarily crude:
`libc::SYS_close_range` **is** defined on musl x86_64 (`musl/b64/x86_64/mod.rs:515`) and on every
other musl arch (and on mips as `4000 + 436`, which a hard-coded 436 would get wrong). Use the
constant:

```rust
// close_range(2), Linux >= 5.9. Absent from rustix (verified 1.1.5); `libc::close_range`
// is declared gnu-only, but `libc::SYS_close_range` is defined on musl too.
// CLOSE_RANGE_CLOEXEC=2: mark cloexec rather than close, so a live Rust `File`'s Drop
// cannot later close an unrelated fd (D1 §11.2).
unsafe { libc::syscall(libc::SYS_close_range, 3, u32::MAX, libc::CLOSE_RANGE_CLOEXEC) }
```

`[O]` HIGH — `libc::CLOSE_RANGE_CLOEXEC = 1 << 2` and `CLOSE_RANGE_UNSHARE = 1 << 1`
(`src/unix/linux_like/linux/mod.rs`, under a `// linux/close_range.h` comment) — present on all
Linux targets, not gnu-gated. D1 §11.1's *"`close_range` is declared gnu-only"* is **CONFIRMED** for
the function and **does not apply** to the constants or the syscall number.

## 3.6 Adjudication: five calls, or two hundred lines? — **DISPUTED, and both are right about their own stack**

D2 §12.6 says `rustix` + a **five-call `libc` surface**. D1 §11 says `libc` + `nix` + later `rustix`
and **~200 lines of our own**. These are not contradictory estimates of the same thing; they are
accurate estimates of two different stacks. Adjudicating:

`[O]` HIGH — the actual surface, item by item, for **D2's stack** (rustix 1.1.5 + libc), with what
libc supplies and what we must write:

| # | Call | Why rustix won't | libc supplies | Our lines |
|---|---|---|---|---|
| 1 | `clone3` | `not_implemented::libc_internals` | `SYS_clone3` + `struct clone_args` | **~15** (zeroed struct + 1 `syscall`, with §3.3's two comments) |
| 2 | `execve` | `not_implemented::quite_yet` | `pub fn execve` | 1 |
| 3 | `_exit` | `quite_yet` | `pub fn _exit(..) -> !` | 1 |
| 4 | `sigprocmask` | `libc_internals` | `pub fn sigprocmask` | 1 |
| 5 | `signalfd` | not wrapped | `pub fn signalfd` | 1 |
| 6 | **`close_range`** | **absent (§2.3.1)** | `SYS_close_range` + `CLOSE_RANGE_CLOEXEC` | 1 |
| 7 | **`mount_setattr`** | `not_implemented::yet` (still, in 1.1.5) | `SYS_mount_setattr` + `mount_attr` + all `MOUNT_ATTR_*` | 1 |
| 8 | `seccomp(2)` | not wrapped | `SYS_seccomp` | **0** if we take `seccompiler` (§5.2), else 1 |
| — | `CLONE_INTO_CGROUP` | — | **broken/absent (§3.4)** | 1 `const` |
| — | `capget`/`capset` | **rustix has them** (`thread::{capabilities, set_capabilities}`) | nothing (§3.1.4) | **0** |
| — | `prctl(NO_NEW_PRIVS)` | **rustix has it** (`thread::set_no_new_privs`) | `PR_SET_NO_NEW_PRIVS` | **0** |
| — | raw `setresuid`/`setresgid`/`setgroups` | **rustix has them, already raw** (§4.4) | — | **0** |

`[R]` HIGH — **the answer is: D2's five is short by two, becomes seven, and is still not 200 lines.**
`close_range` and `mount_setattr` must join the list — the first because D2 believed a rustix
function that does not exist (§2.3.1), the second because D2's §12.6 take-table never mentioned it
even though D1 §4.5 wants the new mount API and D1 §11.1 correctly flagged it absent. Add the
`CLONE_INTO_CGROUP` constant and `seccomp(2)` if D5 goes hand-rolled, and the written-down list is
**seven calls and one constant**, total **≈25 lines including comments** — of which 15 are the one
`clone3` block D2 §12.6 already says must be ours and read line by line.

`[R]` HIGH — **D1's ~200 lines is also correct, for D1's stack, and the difference is entirely
`capget`/`capset`.** D1's list is `clone3`, `mount_setattr`, `close_range`, `capget`/`capset`,
`prctl(NO_NEW_PRIVS)`. Of those, items 1–3 are ~17 lines by the table above; `prctl(NO_NEW_PRIVS)`
is one line in either stack. The remaining ~180 lines are **capabilities**, and they are ~180 lines
only because `libc` supplies neither the structs nor the `CAP_*` table (§3.1.4) — exactly D5 §19.2's
reasoning for taking the `caps` crate. **Taking `rustix` deletes that entire block**, because
`rustix::thread::libcap` already contains the v3 header/data structs and the hi/lo split. That is a
genuine, mechanical, ~180-line argument for rustix that **neither D1 nor D2 makes**, and it is a
better one than most of §12.1.7's four.

`[R]` HIGH — so the two documents do not actually disagree. **The correct statement for the
specification is: "rustix + a seven-call `libc` surface and one hand-defined constant, ≈25 lines;
the ~200-line figure in D1 §11 is the cost of the `libc`+`nix` stack, and ~180 of it is
`capget`/`capset`, which rustix supplies."** Both documents should be made to say that, because as
written a reader sees two numbers an order of magnitude apart and cannot tell which to budget for.

---

# Part 4 — the post-`clone`, pre-`execve` window

`[M]` Primary sources read this session, all pinned: **glibc 2.40** (`bminor/glibc@glibc-2.40` —
2.40 is `my-droplet`'s version, per D2 §15.11's `getconf GNU_LIBC_VERSION`): `posix/fork.c`,
`sysdeps/nptl/fork.h`, `sysdeps/nptl/_Fork.c`, `sysdeps/nptl/setxid.h`, `nptl/nptl_setxid.c`,
`sysdeps/unix/sysv/linux/{setuid,setresuid}.c`, `sysdeps/unix/sysv/linux/single-thread.h`.
**Rust std** (`rust-lang/rust@1.90.0`): `library/std/src/sys/alloc/unix.rs`, `library/std/src/io/stdio.rs`.
**man7** `signal-safety(7)`. Plus `rustix-1.1.5` source.

## 4.1 The malloc-arena deadlock — **CORRECTED. The real hazard is sharper, narrower, and worse for us than D1 §11.2 states.**

D1 §11.2 and D2 §8.1 both tell the classic story: another thread holds the malloc arena lock at
fork time; the child's first allocation deadlocks forever. The story is **true of a raw `clone`**
and **false of glibc's `fork()`** — and we use a raw `clone3`.

`[O]` HIGH — `glibc-2.40/posix/fork.c`, `__libc_fork()`, parent side before the clone:

```c
bool multiple_threads = !SINGLE_THREAD_P;
lastrun = __run_prefork_handlers (multiple_threads);
if (multiple_threads)
  {
    call_function_static_weak (__nss_database_fork_prepare_parent, &nss_database_data);
    _IO_list_lock ();
    /* Acquire malloc locks.  This needs to come last because fork
       handlers may use malloc, and the libio list lock has an
       indirect malloc dependency as well (via the getdelim function).  */
    call_function_static_weak (__malloc_fork_lock_parent);
  }
pid_t pid = _Fork ();
```

and the child side:

```c
if (pid == 0) {
    fork_system_setup ();
    if (multiple_threads) {
        __libc_unwind_link_after_fork ();
        fork_system_setup_after_fork ();      /* __nptl_nthreads = 1; reset nptl locks */
        call_function_static_weak (__malloc_fork_unlock_child);
        fresetlockfiles ();                   /* reset every FILE*'s recursive mutex */
        _IO_list_resetlock ();
        call_function_static_weak (__nss_database_fork_subprocess, &nss_database_data);
    }
    __rtld_lock_initialize (GL(dl_load_lock));
    __rtld_lock_initialize (GL(dl_load_tls_lock));
    reclaim_stacks ();                        /* unconditional */
    __run_postfork_handlers (atfork_run_child, multiple_threads, lastrun);
}
```

`[R]` HIGH — **glibc goes to considerable trouble to make `malloc` safe across its own `fork()`.**
It takes the arena locks *before* the clone and releases them on both sides, resets every `FILE*`
lock, resets the two dynamic-loader locks, and `reclaim_stacks()` marks every other thread's stack
free. So the sentence *"the child's first allocation deadlocks"* is **not** a correct description of
glibc `fork()`. D1 §11.2 states it as unconditional; it is not.

`[R]` HIGH — **and that makes our situation worse, not better, which is why this correction
matters.** D1 §10.1 step 6 and D2 §10.2 both create the container child with
`syscall(SYS_clone3, …)`. A raw `clone3` is a bare kernel entry: **none** of the code above runs.
No `__run_prefork_handlers`, no `__malloc_fork_lock_parent`, no `__malloc_fork_unlock_child`, no
`fresetlockfiles`, no `_IO_list_resetlock`, no `__rtld_lock_initialize`, no `reclaim_stacks`, no
`pthread_atfork` handlers on either side. The child inherits every lock in whatever state it was
in. So:

- **The deadlock the D-files describe is real for us precisely because we bypass the mitigation
  that would have prevented it.** The correct statement is not "fork is dangerous" but **"a raw
  `clone`/`clone3` is strictly more dangerous than `fork()`, because `fork()` has a documented
  recovery path and `clone3` has none."**
- The protection we substitute is D2 §8.2's `[R]` HIGH: *never have a second thread*. If
  `multiple_threads` is false, every mitigation above is a no-op anyway, so the two paths converge
  — which is the real reason the invariant is load-bearing rather than merely tidy.

`[R]` HIGH — **restate D1 §11.2's opening in the specification as:** *"After a raw `clone`, the
child inherits the parent's address space including every lock, and — unlike `fork(3)` — glibc has
had no opportunity to quiesce or reset any of them. Therefore the rules below are not best practice;
they are the only thing standing between us and an unreproducible hang."* Same conclusion, correct
mechanism, and it closes off the reviewer who says "but glibc handles fork, doesn't it?"

`[O]` HIGH — **D1 §11.2's `[F]` MED on the Rust mapping is now SETTLED, from the exact file D1
named.** `rust-lang/rust@1.90.0/library/std/src/sys/alloc/unix.rs`:

```rust
unsafe impl GlobalAlloc for System {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        if layout.align() <= MIN_ALIGN && layout.align() <= layout.size() {
            unsafe { libc::malloc(layout.size()) as *mut u8 }
        } else { unsafe { aligned_malloc(&layout) } }
    }
    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 { … libc::calloc(…) … }
```

`[R]` HIGH — `System::alloc` is `libc::malloc`, `alloc_zeroed` is `libc::calloc`, and
`aligned_malloc` is `posix_memalign` — all the same arenas, all the same locks. **Rust inherits the
hazard exactly. Upgrade D1 §11.2's `[F]` MED to `[F]` HIGH and cite this file.**

## 4.2 glibc's `setxid` broadcast — **CORRECTED to conditional, and the condition is one we deliberately rely on elsewhere**

D1 §11.2's `[C]` HIGH: *"glibc implements the `setxid` family with a broadcast: signal every
registered thread and wait for acknowledgement. In a cloned child whose libc thread bookkeeping is
inherited but whose threads do not exist, that hangs."*

`[O]` HIGH — `glibc-2.40/sysdeps/nptl/setxid.h`, the macro every `set*id` goes through
(`sysdeps/unix/sysv/linux/setuid.c` is literally `INLINE_SETXID_SYSCALL (setuid32, 1, uid)`;
`setresuid.c` likewise):

```c
#define INLINE_SETXID_SYSCALL(name, nr, args...)                        \
  ({ int __result;                                                      \
     if (!SINGLE_THREAD_P)                                              \
       { struct xid_command __cmd; __cmd.syscall_no = __NR_##name;      \
         __SETXID_##nr (__cmd, args);                                   \
         __result = __nptl_setxid (&__cmd); }                           \
     else                                                               \
       __result = INLINE_SYSCALL (name, nr, args);                      \
     __result; })
```

`[R]` HIGH — **the broadcast is conditional on `!SINGLE_THREAD_P`.** D1 states it unconditionally.
`SINGLE_THREAD_P` is `(THREAD_GETMEM (THREAD_SELF, header.multiple_threads) == 0)`
(`sysdeps/unix/sysv/linux/single-thread.h`) — a TCB field, inherited verbatim by a raw `clone3`
child. So:

- Process **never had a second thread** → `header.multiple_threads == 0` in parent and child →
  glibc `setuid` is a plain `INLINE_SYSCALL`. **No broadcast, no hang.**
- Process **ever had a second thread** → the flag is set, it is inherited, and the child runs
  `__nptl_setxid` against a stale thread list.

`[O]` HIGH — what `__nptl_setxid` then does (`glibc-2.40/nptl/nptl_setxid.c:175-278`) is worse than
D1's one-line summary, and contains **two independent hangs**:

1. Its very first statement is `lll_lock (GL (dl_stack_cache_lock), LLL_PRIVATE);`. If any thread
   held that lock at clone time, the child blocks here, before it has signalled anything.
2. It then walks `GL(dl_stack_used)` and `GL(dl_stack_user)`, `setxid_mark_thread`s each entry,
   `tgkill`s them with `SIGSETXID`, and waits:
   ```c
   int cur = cmdp->cntr;
   while (cur != 0) { futex_wait_simple ((unsigned int *) &cmdp->cntr, cur, FUTEX_PRIVATE);
                      cur = cmdp->cntr; }
   ```
   Those threads do not exist in the child. `cntr` is never decremented. The futex wait is
   **uninterruptible by anything except a signal**, and the loop re-enters.

`[O]` HIGH — and this is exactly the hazard `fork()` avoids via `reclaim_stacks()`, which runs
*unconditionally* in the fork child and splices every other thread off `dl_stack_used`. **Under a
raw `clone3` it does not run**, so `dl_stack_used` in our child still lists the ghosts. §4.1's
point again, in a second guise.

`[R]` HIGH — **D1's `[R]` is unchanged and is now better supported: use raw syscalls for the uid/gid
drop.** Not because the broadcast always happens, but because (a) the single-threaded guarantee is a
property of the whole transitive dependency graph, which D1 itself says you cannot audit, (b) the
flag is inherited rather than reset under `clone3`, and (c) when it does go wrong it hangs in an
uninterruptible futex with no diagnostic. The corrected claim to put in D1 §11.2:
*"glibc's `set*id` wrappers broadcast `SIGSETXID` to every thread **when `SINGLE_THREAD_P` is
false** — a TCB flag the `clone3` child inherits and `clone3` does not reset. Use raw syscalls; and
note the first thing the broadcast does is take `dl_stack_cache_lock`, so it can hang before it
signals anything."*

`[O]` HIGH — youki's comment that D1 quotes (*"This is safe because at this point we have only one
thread in the process"*) is now exactly justified: it is an assertion that `SINGLE_THREAD_P` holds,
made by code that is bypassing the branch that depends on it.

## 4.3 Settling D2 §14 item 4 — is `errno` well-defined in the child? — **CONFIRMED well-defined, and the question is moot for us**

`[O]` HIGH — glibc's `errno` is `__thread int errno` in the TCB. After a `clone` **without
`CLONE_VM`** (which is our case — D1 §10.1 and D2 §10.2 both clone with namespace flags and no
`CLONE_VM`), the child has a private copy-on-write address space, so the TCB and the `errno` slot in
it are the child's own. Reads and writes are **defined**; the inherited value is simply whatever the
parent last left there.

`[R]` HIGH — so D1 §11.2's `[F]` MED deflation (which I already recorded in §2.5) is the right
answer: this is a *robustness* question, not a soundness one. There is no UB either way.

`[R]` HIGH — **and D2 §14 item 4's own observation is the one that settles it: the question
disappears under `linux_raw`.** §2.5 verified mechanically that `rustix`'s `linux_raw` backend has
**zero** TLS accesses and decodes errors from the syscall return register into a
`#[repr(transparent)] Errno(u16)`. Re-verified in **1.1.5**: `grep -rn "errno_location|__errno|
thread_local" src/backend/linux_raw/` → still zero hits. So in the child subset there is no `errno`
read at all, from any rustix call. **Item 4: CLOSED.** The residual exposure is only the seven
`libc` calls of §3.6, of which `execve`, `_exit` and `syscall(SYS_clone3)` do not meaningfully
consult `errno` and the rest are checked immediately against their return value.

## 4.4 Settling D2 §14 item 9 — is `rustix::thread::set_thread_{uid,gid,groups}` enough? — **CONFIRMED, and better than D2 hoped: rustix already *is* youki's mitigation**

This is the item D2 called *"load-bearing for the whole rustix-not-nix decision."* It is settled
affirmatively, from source, on both backends.

`[O]` HIGH — `rustix-1.1.5/src/thread/id.rs` exposes `set_thread_uid`, `set_thread_res_uid`,
`set_thread_gid`, `set_thread_res_gid`, `set_thread_groups`, each carrying the warning D2 quotes:

> `/// # Warning`
> `/// This is not the `setuid` you are looking for… POSIX requires uids to be process granular, but`
> `/// on Linux they are per-thread. Thus, this call only changes the uid for the current *thread*,`
> `/// not the entire process even though that is in violation of the POSIX standard.`
> `/// … This call implements the kernel behavior.`

`[O]` HIGH — **what they actually issue.** `linux_raw` backend
(`src/backend/linux_raw/thread/syscalls.rs:402-447`):

```rust
pub(crate) fn setuid_thread(uid: Uid) -> io::Result<()> {
    unsafe { ret(syscall_readonly!(__NR_setuid, uid)) }
}
pub(crate) fn setresuid_thread(ruid: …, euid: …, suid: …) -> io::Result<()> { … __NR_setresuid … }
pub(crate) fn setgroups_thread(gids: &[Gid]) -> io::Result<()> {
    let (addr, len) = slice(gids);
    unsafe { ret(syscall_readonly!(__NR_setgroups, len, addr)) }
}
```

— inline `asm!`, no libc symbol touched at all.

`[O]` HIGH — **and the `libc` backend bypasses glibc too**, which I did not expect and which is the
finding that closes the item. `src/backend/libc/thread/syscalls.rs:391`:

```rust
pub(crate) fn setuid_thread(uid: Uid) -> io::Result<()> {
    syscall! { fn setuid(uid: c::uid_t) via SYS_setuid -> c::c_int }
    unsafe { ret(setuid(uid.as_raw())) }
}
```

and that `syscall!` macro, `rustix-1.1.5/src/weak.rs:164-252` under `#[cfg(linux_kernel)]`, expands
to **`libc::syscall($sys_name, …)`** — the raw-syscall entry point — **not** `libc::setuid`. So
rustix never calls the glibc wrapper, on either backend, on any Linux target.

`[R]` HIGH — **SETTLED, and the correct statement is stronger than D2's.** It is not merely that
"per-thread == process-wide for a process that never had a second thread" (true, and the kernel's
own `setuid(2)` NOTES say so). It is that **`rustix::thread::set_thread_res_uid` is byte-for-byte
the mitigation youki hand-writes** — `libc::syscall(libc::SYS_setresuid, …)` under the comment
*"This is safe because at this point we have only one thread in the process"* — already packaged,
already `unsafe`-free at our call site, and already returning a non-allocating `Errno`. §4.2's
`SIGSETXID` broadcast is unreachable through it by construction.

`[R]` HIGH — **this retires the last substantive objection to rustix-not-nix and should be recorded
as such.** For the record, `nix::unistd::{setuid, setgid, setgroups}` call the **glibc wrappers**
(`libc::setuid`, `libc::setgroups`), i.e. §4.2's broadcast path. Taking `nix` here would require
bypassing `nix` for exactly the call that matters — which is §1.1's pattern a third time.

`[R]` MED — **two caveats to carry into the spec, neither of which changes the verdict:**
1. `set_thread_groups(&[])` must issue `setgroups(0, ptr)`. rustix's `slice(gids)` on an empty slice
   yields a dangling-but-aligned pointer with `len == 0`; the kernel does not dereference it when
   `len == 0`. Fine — but D5 §8.5's "deny setgroups then drop supplementary groups" ordering is ours
   to get right, and in a user namespace `/proc/self/setgroups` must be written `deny` *before* the
   uid map, which is a file write, not this call.
2. The per-thread semantics make the **`Threads == 1` assertion from `/proc/self/status`**
   (D1 §3.4, D2 §8.2) a correctness precondition for these calls and not just hygiene. §4.6 names
   the test.

## 4.5 Settling D2 §14 item 3 — does rustix's path argument allocate in our child subset? — **CONFIRMED moot by construction, with the threshold and the test named**

`[O]` HIGH — re-verified in **1.1.5** (`src/path/arg.rs`, `src/path/mod.rs:9`), unchanged from the
1.1.4 reading in §2.6: `impl Arg for &CStr` / `&CString` / `CString` make `into_with_c_str` the
identity — `f(self)`, zero copy, zero allocation. For every other type the shared `with_c_str`
helper uses a stack `MaybeUninit<[u8; SMALL_PATH_BUFFER_SIZE]>` with
`SMALL_PATH_BUFFER_SIZE = 256`, and at `bytes.len() >= 256` falls into
`#[cold] with_c_str_slow_path`, which under `feature = "alloc"` is
`f(&CString::new(bytes).map_err(|_| io::Errno::INVAL)?)` — **a heap allocation**. `default = ["std"]`
and `std = [… "alloc" …]`, so D2 §12.6's own recommended feature list has the slow path live.

`[R]` HIGH — **the mitigation is an elimination, not a mitigation, and item 3 is CLOSED** — provided
the discipline is structural rather than remembered. Make it structural: the child-side setup
function should take a struct whose every path field is already a `CString`, built parent-side:

```rust
/// Every path the child touches, pre-encoded parent-side. No `&str`, no `PathBuf`,
/// no `String` may appear in this struct or in any function reachable from
/// `child_main`: rustix's `path::Arg` heap-allocates for any path >= 256 bytes
/// (SMALL_PATH_BUFFER_SIZE, rustix 1.1.5 src/path/mod.rs:9), and D3's CAS layout
/// makes >= 256-byte paths the norm, not the exception.
struct ChildPaths { rootfs: CString, old_root: CString, proc_target: CString, /* … */ }
```

`[R]` HIGH — **the `#[global_allocator]` counting test D2 §14 item 3 asks for, named concretely.**
It does not need a container and it does not need root, so it runs in CI and on the dev VM both:

```rust
// tests/no_alloc_in_child.rs  — name it this, run it with `cargo test -p core --test no_alloc_in_child`
struct Counting;
static ALLOCS: AtomicUsize = AtomicUsize::new(0);
unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, l: Layout) -> *mut u8 { ALLOCS.fetch_add(1, Relaxed); System.alloc(l) }
    unsafe fn dealloc(&self, p: *mut u8, l: Layout) { System.dealloc(p, l) }
}
#[global_allocator] static A: Counting = Counting;

#[test] fn child_setup_steps_allocate_nothing() {
    let paths = ChildPaths::build(&spec);          // parent-side; allocations allowed
    let before = ALLOCS.load(Relaxed);
    run_child_setup_dry(&paths);                   // every D1 §10.1 step 8..30, against /tmp, no clone
    assert_eq!(ALLOCS.load(Relaxed), before, "child setup path allocated");
}
```

`[R]` MED — and give it the adversarial case explicitly: a `rootfs` path of **≥ 256 bytes**, which
is the only input that distinguishes the stack fast path from the heap slow path. A test built from
`/tmp/x` passes whether or not the discipline is being followed, and is therefore worthless.

## 4.6 What Rust `std` is and is not safe to call in that window — **CONFIRMED, with one deflation of D1's observation**

`[O]` HIGH — `signal-safety(7)` is the governing list and D2 §8.1's characterisation of it is
accurate: it is a short POSIX-mandated set, essentially raw syscalls plus a few reentrant string
helpers. The man page's own explanation of *why* is the stdio buffer argument:

> *"When performing buffered I/O on a file, the stdio functions must maintain a statically allocated
> data buffer along with associated counters and indexes … If, at that moment, the program is
> interrupted by a signal handler that also calls `printf(3)`, then the second call to `printf(3)`
> will operate on inconsistent data, with unpredictable results."*

`[O]` HIGH — mapping it onto `std`, from `rust-lang/rust@1.90.0/library/std/src/io/stdio.rs`:

| Operation | Verdict in the child | Mechanism |
|---|---|---|
| `println!` / `print!` | **forbidden** | `stdio.rs:612`: `Stdout { inner: &'static ReentrantLock<RefCell<LineWriter<StdoutRaw>>> }`, behind `static STDOUT: OnceLock<…>` (`:644`). Three hazards: a `ReentrantLock` that may be held, a `OnceLock` that may be mid-initialisation, and a `LineWriter` buffer inherited with content. |
| `eprintln!` / `eprint!` | **forbidden**, but *less* bad | `stdio.rs:969`: `ReentrantLock<RefCell<StderrRaw>>` — **unbuffered**, and a plain `static`, not a `OnceLock`. The lock remains a hazard; there is no inherited buffer. |
| `format!`, `String`, `Vec` growth, `to_owned`, `Box::new`, `CString::new`, any collection insert | **forbidden** | §4.1 — `System::alloc` → `libc::malloc`. |
| `panic!` / any unwind / `unwrap` on an `Err` / slice index OOB / integer-overflow panic in debug | **forbidden** | the panic machinery formats and allocates, then may run the hook. D1 §11.2 and D2 §8.1 both say so; §1.6 adds that `nix::fcntl::Flock`'s `Drop` can *itself* `panic!`. |
| `std::process::Command` | **forbidden** | spawns, allocates, reads env. |
| `std::fs::*`, `std::io::Read/Write` on an existing `File` | **avoid** | the syscall is fine; the error path is `std::io::Error`, which **allocates** for custom errors and carries a `Box` niche. Use `rustix` and its `Errno(u16)`. |
| `rustix::*` in the §3.6 subset with `CString` paths | **safe** | §2.5 (no TLS, no `std::io::Error`) + §4.5 (no allocation). |
| raw `libc::syscall(…)`, `libc::_exit`, `libc::execve` | **safe** | async-signal-safe by construction. |
| reading a pre-built `&[u8]` and `write(2)`ing it to a pre-opened raw fd | **safe** | this is D2 §8.3's child-log design and runc's `_LIBCONTAINER_LOGPIPE`. |
| `core::fmt::Write` into a pre-allocated `[u8; N]` buffer | **safe** | no allocator involvement. The practical way to get a readable child error into D2 §9.3(4)'s `[u8; 512]` record. |

`[O]` MED — **one deflation of D1 §11.2's empirical observation, stated because it is the kind of
thing that gets over-generalised.** D1 records duplicated output as `[O]` DROPLET 6.11.0-29 with
HIGH confidence, from its `clone3` probe script. That script was **Python** (D2 §15.11: a
`python3 -c` ctypes probe), and C/Python `stdout` is **fully buffered** when it is not a tty — hence
a fat unflushed buffer at clone time and verbatim duplication. **Rust's `Stdout` is a `LineWriter`
unconditionally**, even to a pipe (`stdio.rs:612`, with the source's own `// FIXME: this should be
LineWriter or BufWriter depending on the state of the console`). So a Rust parent that has only
emitted `\n`-terminated lines has an empty buffer, and the duplication would not reproduce.

`[R]` HIGH — **the hazard is real but the vector is different, and the spec should say which.** For
us the inherited-buffer risk comes from (a) any `BufWriter` we construct ourselves, (b) `print!`
without a newline, (c) **any C library in the process** — and (d) the one that will actually bite:
a `tracing` subscriber's `fmt` layer, which §6.3 shows buffers into a `Vec<u8>` before writing.
D1's conclusion ("flush or close everything before the clone, and do not use `std::io::Stdout` in
the child") is correct; its stated evidence is from a Python parent and should be re-labelled
`[O]` as *corroborating the mechanism in C*, not as measuring Rust.

`[R]` MED — the cheap belt-and-braces: `io::stdout().flush()` and `io::stderr().flush()` as the
statement immediately before the `clone3`, next to the `Threads == 1` assertion. Two lines, and it
makes the whole class impossible regardless of what a dependency did.


---

# Part 5 — the domain crates, one at a time

`[M]` Every crate below was downloaded from `static.crates.io` and read as source, except where I
say otherwise. Maintenance figures are from the GitHub API fetched **2026-10-23** (today), and
versions/licences/MSRVs from the crates.io API the same day. Advisory status is from a
`git clone` of `RustSec/advisory-db` whose head is `84d943a6` dated **2026-10-23 16:42 +0200** —
i.e. current to today, which is what makes Part 7's "anything filed since 2026-09-04" sweep
meaningful.

## 5.1 `caps` vs `rustix` — **D2 WINS, and on three grounds, only one of which D2 made**

This is the headline conflict: D5 §19.2 `[R]` MED *take `caps`*; D2 §12.5 `[R]` HIGH *avoid it*.
I read both sides. **D2 is right, and its own argument is the weakest of the three available.**

### 5.1.1 D2's mechanical argument — **CONFIRMED verbatim**

`[O]` HIGH — `caps-0.5.6/src/lib.rs:266` and `:304`, exactly as D2 quotes:

```rust
pub type CapsHashSet = std::collections::HashSet<Capability>;
pub fn set(tid: Option<i32>, cset: CapSet, value: &CapsHashSet) -> Result<(), CapsError>;
```

`[O]` HIGH — and the other side, `rustix-1.1.5/src/thread/libcap.rs`:

```rust
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CapabilitySets { pub effective: CapabilitySet, pub permitted: CapabilitySet,
                            pub inheritable: CapabilitySet }
bitflags! { #[repr(transparent)] pub struct CapabilitySet: u64 {
    const CHOWN = 1 << linux_raw_sys::general::CAP_CHOWN; … const _ = !0; } }
pub fn capabilities(pid: Option<Pid>) -> io::Result<CapabilitySets>;
pub fn set_capabilities(pid: Option<Pid>, sets: CapabilitySets) -> io::Result<()>;
```

`[R]` HIGH — **CONFIRMED.** Honouring OCI `process.capabilities` through `caps` means building a
`HashSet<Capability>` per set — a heap allocation plus up to 41 hash insertions — in the child,
after `clone3`, in the window §4.1 shows has no glibc recovery path. `rustix`'s is a `Copy` struct
of three `u64`s. D2's argument stands exactly as written.

### 5.1.2 The stronger argument D2 missed: **every `caps` error path allocates *and* reads `errno` TLS**

`[O]` HIGH — `caps-0.5.6/src/errors.rs`: `pub struct CapsError(pub(crate) String)`. And every single
call site builds it the same way — `src/base.rs:12`, `:20`, `src/ambient.rs:14,32,47,62`,
`src/bounding.rs:19,30`:

```rust
Err(format!("capget failure: {}", std::io::Error::last_os_error()).into())
```

`[R]` HIGH — that one line does **three** forbidden things in the §4.6 window: `format!`
(allocates), `String` (allocates), and `std::io::Error::last_os_error()` (**reads the inherited
`errno` TLS** — §4.3's question, reintroduced through the back door after rustix had eliminated it).
And it fires on the **failure** path, which is the path you most need to survive in order to report
anything at all. D1 §11.1 spotted this (*"errors are `CapsError(String)`, so it is unusable
post-clone"*) and **D2 did not**, while D5 §19.2 recommends the crate without mentioning it. This is
the decisive argument and it should be the one written down.

### 5.1.3 The third argument: **`caps` is syscall-inefficient and non-atomic where it matters**

`[O]` HIGH — `caps-0.5.6/src/base.rs:95` — `set()` is a **read-modify-write of one set at a time**:
`capget` → mutate one of `{effective, permitted, inheritable}` → `capset`. So installing the three
base sets costs **3 `capget` + 3 `capset` = 6 syscalls**, and leaves the thread in two intermediate
capability states en route. `rustix::thread::set_capabilities(None, sets)` is **one `capset`** with
all three sets, atomically.

`[O]` HIGH — `caps-0.5.6/src/bounding.rs:6` — `clear()` is
`for c in all() { if has_cap(c)? { drop(c)? } }`: up to **82 `prctl` calls**, plus `all()` which is
`CapsHashSet::from_iter(vec![…41 items…])` — a `Vec` *and* a `HashSet` allocation, every call.
`runtime::thread_all_supported()` calls `all()` **twice** and adds another 41 `prctl`s.

`[O]` HIGH — and the one that would actually break in a container:
`caps::runtime::procfs_all_supported()` opens **`/proc/sys/kernel/cap_last_cap`**
(`src/runtime.rs:40`). In the container child, after `pivot_root` and before `/proc` is mounted
(D1 §6.1 and §10.1 put the `/proc` mount *inside* the new root), that path does not exist; and once
it does, it is the container's `/proc`, not the host's. A capability library that consults the
filesystem is the wrong shape for this window.

### 5.1.4 The asymmetry the brief asked about: does rustix cover bounding and ambient? — **YES, all five sets**

`[O]` HIGH — re-verified in `rustix-1.1.5`:

| OCI set | `caps` | `rustix` |
|---|---|---|
| effective / permitted / inheritable | `base::{read,set}` via `capget`/`capset` | `thread::capabilities` / `thread::set_capabilities` (`src/thread/libcap.rs:123,136`) |
| **bounding** | `bounding::{has_cap,drop,clear}` | `thread::capability_is_in_bounding_set` (`prctl.rs:467`), `thread::remove_capability_from_bounding_set` (`:489`) |
| **ambient** | `ambient::{has_cap,raise,drop,clear,set}` | `thread::capability_is_in_ambient_set` (`:698`), `thread::configure_capability_in_ambient_set` (`:739`), `thread::clear_ambient_capability_set` (`:725`) |
| the rest of D5 §8.5's ordering | **absent** — `caps` has **zero** hits for `PR_SET_NO_NEW_PRIVS` (D1 §11.1, CONFIRMED) and none for `PR_SET_KEEPCAPS` | `thread::set_no_new_privs` (`:631`), `thread::set_keep_capabilities` (`:59`), `thread::secure_computing_mode` (`:159`) |

`[R]` HIGH — **D2 §12.5's parenthetical — "five sets, which is one more than rustix's `capset`-shaped
API exposes as a set" — is misleading and should be struck.** rustix covers all five; it simply
models them the way the kernel does, as `capget`/`capset` for three and `prctl` for two, instead of
flattening them into one enum. And rustix covers **two more** things in D5 §8.5's ordering
(`PR_SET_NO_NEW_PRIVS`, `PR_SET_KEEPCAPS`) that `caps` does not cover at all. The set coverage runs
the *other* way.

`[O]` MED — one real `caps` advantage, stated fairly: it has a **name↔bit table** with
`FromStr`/`Display` (`Capability::CAP_SYS_ADMIN` ⇄ `"CAP_SYS_ADMIN"`), which is what you need to
parse OCI `config.json`'s string capability names. rustix's `CapabilitySet` is `bitflags` with
*Rust* names (`SYS_ADMIN`, not `CAP_SYS_ADMIN`) and no `FromStr`. So the OCI parse step needs a
~41-line `match` from us either way — and D1 §11.1's *"use `caps` only for its name↔bit table if we
want it"* is the right residual use. `[R]` MED — **don't even do that.** Write the `match` in the
**parent**, parent-side, where allocation is legal, converting OCI strings into a
`rustix::thread::CapabilitySets` that is then `Copy`d into the child. Forty-one lines, no
dependency, and the table is exactly the kind of thing we want visible.

`[O]` HIGH — **maintenance, both sides, today.** `lucab/caps-rs`: 96 stars, 8 open issues,
`pushed_at` **2026-10-02**, not archived. Commit log: **one** commit since 2025-10-17
(`1345977d` *"cargo: bump MSRV and tweak CI check"*, 2026-10-02), by the sole maintainer. So D2's
*"4 commits in the last 52 weeks by 2 authors"* has become **≈1 commit in 12 months by 1 author**.
Zero RUSTSEC advisories, 10 `unsafe` occurrences in 1,044 lines — a clean, small, finished-ish crate.
It is not rotten; it is simply inert, and D1 §11.1's note that its table is hardcoded to
`CAP_CHECKPOINT_RESTORE = 40` means it will need a commit the day the kernel adds capability 41.

`[R]` HIGH — **VERDICT: avoid `caps`. D5 §19.2's `[R]` MED should flip to "take `rustix`".** D5's
reasoning — *"maybe 200 lines of `unsafe` we would otherwise write, and getting the
`capget`/`capset` v3 header struct and the two-`__u32` split right is fiddly and easy to get subtly
wrong"* — is **factually correct and §3.1.4 confirms it**, but it is an argument against
hand-writing, not an argument for `caps`. `rustix::thread::libcap` already contains that v3 header,
that two-`__u32` split (`u64::from(data.0.effective) | (u64::from(data.1.effective) << u32::BITS)`),
and `_LINUX_CAPABILITY_VERSION_3` from bindgen'd kernel headers. D5 gets its 200 lines deleted
*and* keeps allocation-free, errno-free, single-`capset` semantics. **D5 §19.2's `[R]` HIGH — that
we own the ordering explicitly and let no crate hide it — survives untouched and is the part that
actually matters.**

## 5.2 `seccompiler` vs `libseccomp` — **CORRECTED: `seccompiler`'s repo was archived five weeks ago, and it has a functional gap D5 must price**

### 5.2.1 The repository move — **NEW since the research**

`[O]` HIGH — GitHub API today: `rust-vmm/seccompiler` has **`archived: true`**, `pushed_at`
2026-08-18, `updated_at` 2026-09-23, and its description is now:

> *"This is a public archive. The code now lives in the mono-repo: https://github.com/rust-vmm/rust-vmm/"*

`[O]` HIGH — the monorepo `rust-vmm/rust-vmm` is alive (146 stars, `pushed_at` **2026-10-19**,
not archived) and its tree contains a `seccompiler/` directory alongside `vm-memory`,
`linux-loader`, `kvm` and the rest. So **this is a consolidation, not an abandonment** — but it
happened **after** the research was written and `crates.io`'s `repository` field for `seccompiler`
still points at the archived repo.

`[R]` HIGH — **this is exactly the drift the contract warned about, and it cuts both ways.** A
reviewer who checks `seccompiler`'s linked repo today sees an archive banner and concludes the crate
is dead; that conclusion would be wrong. But the crate itself has not been released since
**0.5.0 / 2025-03-07**, and `SECCOMP_RET_USER_NOTIF` support landed in the repo on **2026-04-14**
(commit `0858710634`) and is **still unreleased**. Record both facts.

### 5.2.2 Does it emit a correct per-architecture filter? — **CONFIRMED, and the arch check is better than the OCI default**

`[O]` HIGH — `seccompiler-0.5.0/src/backend/bpf.rs:72`, the prologue prepended to every program:

```rust
pub(crate) fn build_arch_validation_sequence(target_arch: TargetArch) -> Vec<sock_filter> {
    vec![
        bpf_stmt(BPF_LD | BPF_W | BPF_ABS, SECCOMP_DATA_ARCH_OFFSET as u32),  // offset 4
        bpf_jump(BPF_JMP | BPF_JEQ | BPF_K, target_arch.get_audit_value(), 1, 0),
        bpf_stmt(BPF_RET | BPF_K, libc::SECCOMP_RET_KILL_PROCESS),
    ]
}
```

`[R]` HIGH — **correct, and deliberately strict.** It loads `seccomp_data.arch`, compares against
`AUDIT_ARCH_X86_64` (`62 | 0x8000_0000 | 0x4000_0000`, matching the kernel's
`EM_X86_64|__AUDIT_ARCH_64BIT|__AUDIT_ARCH_LE`), and on mismatch returns **`KILL_PROCESS`**. That
closes the classic bypass where a 64-bit process issues `int 0x80` and the filter, written against
x86_64 syscall numbers, reads an `i386` number as something else entirely. Verified for
`x86_64`, `aarch64`, `riscv64` — `src/backend/mod.rs:81-99`.

`[O]` HIGH — **the gap: `TargetArch` is a single value, not a set.** `SeccompFilter` carries one
`target_arch`; there is no `seccomp_arch_add()` equivalent. The OCI runtime-spec's
`linux.seccomp.architectures` is a **list** (`SCMP_ARCH_X86_64`, `SCMP_ARCH_X86`, `SCMP_ARCH_X32`),
and runc/libseccomp honours all of them in one program by emitting a per-arch dispatch.
**`seccompiler` structurally cannot express a multi-architecture OCI profile.** It will *kill*
anything that is not the one arch, which is safe but is not what the config asked for — and Docker's
default profile does list three architectures.

`[R]` HIGH — **this is the fact D5 needs to price and neither D2 §12.5 nor D5 §19.1 states it.**
Three outcomes are possible: (a) accept single-arch and document that we reject multi-arch profiles,
(b) emit the multi-arch dispatch ourselves on top of `seccompiler`'s `BpfProgram` (it is a
`Vec<sock_filter>`, so this is mechanical — load arch, jump table, concatenate per-arch bodies,
fix up relative jumps), or (c) take `libseccomp` and lose static musl. For phase 1 (a) is clearly
right; the point is that it is a *choice* and it must be recorded as one.

### 5.2.3 `BPF_MAXINSNS` — **CONFIRMED handled, conservatively by one instruction**

`[O]` HIGH — `src/backend/bpf.rs:25` `pub const BPF_MAX_LEN: usize = 4096;` and
`src/backend/filter.rs:221`:

```rust
if result.len() >= BPF_MAX_LEN { return Err(Error::FilterTooLarge(result.len())); }
```

`[R]` MED — the kernel's rule is `len == 0 || len > BPF_MAXINSNS → -EINVAL`, so a program of
*exactly* 4096 instructions is legal and `seccompiler` rejects it. Off-by-one in the safe direction;
costs one instruction out of 4096 and produces a clear typed error rather than an `EINVAL`. Fine.
Worth knowing because Docker's default profile is large and a conversion that emits one comparison
per syscall will approach the ceiling.

### 5.2.4 What `seccompiler` does and does not do

`[O]` HIGH — **does:** `SeccompAction::{Allow, Errno(u32), KillThread, KillProcess, Log, Trace(u32),
Trap}`; argument conditions (`SeccompCondition` over `args[0..6]`, 32- and 64-bit comparands,
`Eq/Ne/Ge/Gt/Le/Lt/MaskedEq`); a JSON frontend (`compile_from_json`, behind the `json` feature);
`apply_filter` / `apply_filter_all_threads`, which **do the whole install for us**
(`src/lib.rs:344-365`): `prctl(PR_SET_NO_NEW_PRIVS, 1)` then
`syscall(SYS_seccomp, SECCOMP_SET_MODE_FILTER, flags, &sock_fprog)`, with TSYNC handled and the
man page's "return value > 0 is the offending tid" convention honoured.

`[O]` HIGH — **does not:** no `SCMP_ACT_NOTIFY` / `SECCOMP_RET_USER_NOTIF` in 0.5.0 (merged
2026-04-14, unreleased); no multi-architecture filters (§5.2.2); no syscall-name resolution for
arches other than x86_64/aarch64/riscv64; no `SCMP_ACT_KILL_THREAD`-vs-`KILL` default negotiation.

`[O]` HIGH — **dependencies: `libc` only** (plus optional `serde`/`serde_json` for the JSON
frontend). **5 `unsafe` occurrences in 3,815 lines.** No C library, no `build.rs`, no `pkg-config`.
→ **static musl is unaffected by it.**

`[O]` HIGH — **`libseccomp 0.4.0`, the other side.** `build.rs` is `pkg_config::Config::new()
.atleast_version("2.6.0").probe("libseccomp")`, with `LIBSECCOMP_LIB_PATH` and
`PKG_CONFIG_ALLOW_CROSS` escape hatches; the real binding is `libseccomp-sys 0.3.0`, FFI to the C
library. 52 `unsafe` in 10,275 lines. Zero RUSTSEC advisories. Maintained
(`libseccomp-rs/libseccomp-rs`, 49 stars, `pushed_at` 2026-10-14) but **tiny by download volume —
328 K in 90 days versus `seccompiler`'s 9.1 M.**

`[R]` HIGH — **VERDICT: `seccompiler`, and D2 §12.5's "genuine either/or" is now resolvable rather
than genuinely open.** `libseccomp` means a C library on the link line, which is D2 §12.5's own
observed reason `runc` is a dynamic executable while `containerd-shim-runc-v2` is static. The only
thing `libseccomp` buys that `seccompiler` cannot is multi-architecture filters and
`SCMP_ACT_NOTIFY` — both of which are phase-3 OCI-compatibility concerns, not phase-1 ones. Take
`seccompiler`, document the single-arch limitation, and revisit at phase 3. `[R]` MED — and note the
**archived-repo trap** in the dependency comment so nobody deletes the crate in a year's audit.

## 5.3 `oci-spec` 0.10.0 — **CORRECTED on the version confusion, and item 11 is answerable now**

### 5.3.1 The version constants — **D2 is half right, and the real state is stranger**

`[O]` HIGH — `oci-spec-0.10.0/src/runtime/version.rs`, verbatim:

```rust
pub const VERSION_MAJOR: u32 = 1;
pub const VERSION_MINOR: u32 = 1;
pub const VERSION_PATCH: u32 = 0;
/// Indicates development branch. Releases will be empty string.
pub const VERSION_DEV: &str = "";
pub const VERSION: &str = formatcp!("{VERSION_MAJOR}.{VERSION_MINOR}.{VERSION_PATCH}{VERSION_DEV}");
```

with the crate's own test asserting `version() == "1.1.0"`.

`[R]` HIGH — **`runtime::VERSION` is `"1.1.0"`, not `"1.1.0-dev"`. D2 §14 item 10 is CORRECTED on
that detail.** `VERSION_DEV` is the empty string in this release.

`[O]` HIGH — **but the rest of D2's observation is CONFIRMED and is the part that matters:**
`src/runtime/mod.rs:195` — `Spec::default()` emits `version: String::from("1.0.2-dev")`. And the
structs carry runtime-spec 1.3.0 fields. So **one crate asserts three different spec versions**:
`VERSION` says 1.1.0, `Spec::default()` says 1.0.2-dev, the schema says 1.3.0.
(`image::VERSION` is likewise `"1.0.1"` while image-spec is at 1.1.1.)

`[O]` HIGH — **has a newer release fixed it? No.** crates.io today: `oci-spec` max stable is still
**0.10.0, updated 2026-05-27**. The repo `youki-dev/oci-spec-rs` is very much alive —
`pushed_at` **2026-10-23 (today)**, 300 stars, 25 open issues — so this is an unreleased-fixes
situation, not a dead crate.

`[R]` HIGH — **the operational consequence, which is small and should be stated so nobody panics:**
we do not use `Spec::default()` and we do not emit `VERSION`. D2 §12.2 already says `oci-spec`'s
types are converted once into our own `Config` and never leak. The `ociVersion` we *write* is a
project decision (D2 §14 item 10 `[R]` MED: target 1.2.0 for phase 1). The `ociVersion` we *read* we
validate ourselves. So the crate's internal confusion is a **code smell about the crate's release
discipline, not a correctness problem for us** — but it is a reason to pin an exact version rather
than a caret range, and to re-read `version.rs` on every bump.

### 5.3.2 Settling item 11 — does `oci-spec` + a key-diff auditor cost less than hand-writing? — **CONFIRMED, take it**

`[O]` HIGH — the measurements: **10,462 lines across 32 files**, **3 `unsafe` occurrences**, zero
RUSTSEC advisories, licence Apache-2.0, 6.45 M downloads in 90 days. Dependencies with
`default-features = false, features = ["runtime"]`: `serde`, `serde_json`, `thiserror`,
`const_format`, `derive_builder`, `getset`, `strum`. It is a pure serde data model; there is no
runtime logic in it to hide a mechanism from us.

`[R]` HIGH — **item 11 answered: yes, and the margin is bigger than §12.2's estimate.** D2 §12.2
compared ~150 lines of conversion + ~30 of auditor against "~300+ of transcription". The real
transcription target is **10,462 lines**, of which the `runtime` subtree alone is the bulk — because
`config.json`'s schema is not 300 lines, it is `Spec` + `Process` + `User` + `Linux` +
`LinuxResources` + `LinuxDevice` + `LinuxSeccomp` + `LinuxIntelRdt` + `LinuxPersonality` + `Mount` +
`Hooks` + `Hook` + `Root` + `Solaris` + `VM` + `Windows` + roughly 40 more types with builders.
Hand-writing even the subset we need is several hundred lines of pure clerical transcription with no
learning in it, and it goes stale on every spec revision. **Take the crate.**

`[R]` HIGH — **and D2 §12.2's `serde_json::Value` key-diff auditor is the right companion and is
worth more than D2 claims**, precisely *because* of §5.3.1: the crate's own version metadata is
unreliable, so the only trustworthy answer to "what fields does this `config.json` contain that we
do not handle?" is to diff the parsed keys against our handled set at runtime. Thirty lines, and it
converts "the crate tracks the spec for us" from a hope into an assertion. Build it first, as
D2 says, against the 9,907-byte nginx `config.json` on `my-droplet`.

## 5.4 `cgroups-rs` — **CONFIRMED reject, and the real reason is far worse than D-Bus**

`[O]` HIGH — `cgroups-rs-0.5.1/Cargo.toml`, the complete non-dev dependency list:

```toml
bit-vec = "0.6"   libc = "0.2"   log = "0.4"
nix = { version = "0.25.0", default-features = false, features = ["event","fs","process"] }
oci-spec = { version = "0.10.0", optional = true }
serde = { version = "1.0", features = ["derive"], optional = true }
thiserror = "1"
zbus = "5.8"        # <-- NOT optional. No feature gate. Unconditional.
```

`[R]` HIGH — **D1 §11.1's rejection is CONFIRMED and understated.** `zbus` is not behind a feature;
every consumer of `cgroups-rs 0.5.1` links it.

`[O]` HIGH — **what `zbus 5` drags in** (crates.io dependency API for `zbus 5.12.0`, non-optional
unless marked): `async-broadcast`, `async-recursion`, `async-trait`, `event-listener`,
`futures-core`, `futures-lite`, `ordered-stream`, `enumflags2`, `hex`, `serde`, `serde_repr`,
`tracing`, `uuid`, `winnow`, `zvariant`, `zbus_names`, `zbus_macros`, **and `nix ^0.30`**; plus,
*enabled by its default features*, `async-io`, `async-executor`, `async-task` and **`blocking`**.

`[R]` HIGH — **three consequences, in increasing order of seriousness:**

1. **D1's "two versions of `nix`" is CONFIRMED, and I can now name both:** `cgroups-rs` depends
   directly on `nix 0.25.0` *and* transitively on `nix ^0.30` through `zbus`. Two semver-incompatible
   `nix` crates compiled into one binary — in a project whose §12.1.7 conclusion is *not to have
   `nix` at all*. D1's `uuid` and `tracing` claims are likewise CONFIRMED, both via `zbus`.
   (D1 also lists `strum`; I did **not** find it in `zbus 5.12`'s direct deps —
   **WEAK-EVIDENCE**, possibly transitive through `zvariant` or from an older `cgroups-rs`.)
2. **It drags in a second `thiserror` major version** (`thiserror 1` vs D2 §12.6's `thiserror 2.0`).
3. **The one that is disqualifying on its own: `blocking` is, by its own crates.io description,
   *"A thread pool for isolating blocking I/O in async programs"*, and `async-io` is an async
   reactor.** D2 §8.2's `[R]` HIGH is *"the container creation path must run in a process that has
   **never had a second thread**"*, and D2 §12.6's structural enforcement is *"the core crate lists
   seven dependencies and nothing else, ever"*. **Putting `cgroups-rs` in the core crate puts an
   async executor and a thread pool in the address space that calls `clone3`.** That is not a
   dependency-weight objection; it is the single invariant the whole runtime design rests on.
   D1 rejected `cgroups-rs` for a D-Bus dependency and was right for a smaller reason than the one
   available.

`[O]` HIGH — **maintenance, for completeness:** `kata-containers/cgroups-rs`, 157 stars, 24 open
issues, `pushed_at` 2026-10-23 (today) — actively maintained, 15 `unsafe` in 12,052 lines, zero
RUSTSEC advisories. **It is a good crate. It is the wrong crate for this process.**

### 5.4.1 Does cgroup v2 need a crate at all? — **No. Measured.**

`[O]` HIGH — `my-droplet` (`AIKYNETIX-PROD-NY`, kernel 6.11.0-29), **read-only** `stat`/`cat`/`ls`
under `/sys/fs/cgroup` only:

```
$ stat -f -c %T /sys/fs/cgroup                 → cgroup2fs
$ cat /sys/fs/cgroup/cgroup.controllers        → cpuset cpu io memory hugetlb pids rdma misc
$ ls /sys/fs/cgroup/system.slice/docker-919e3725….scope | wc -l   → 81
```

and the files a runtime actually writes, with their modes:

```
cgroup.procs           -rw-r--r--      memory.max        -rw-r--r--   cpu.max      -rw-r--r--
cgroup.subtree_control -rw-r--r--      memory.swap.max   -rw-r--r--   cpu.weight   -rw-r--r--
cgroup.kill            --w-------      memory.low        -rw-r--r--   pids.max     -rw-r--r--
cgroup.freeze          -rw-r--r--      io.max            -rw-r--r--   cpuset.cpus  -rw-r--r--
```

live values on the running nginx container: `cpu.max = "max 100000"`, `cpu.weight = "100"`,
`memory.max = "max"`, `pids.max = "19147"`, `cgroup.type = "domain"`.

`[R]` HIGH — **CONFIRMED: cgroup v2 is `mkdir` + `std::fs::write` of short ASCII strings, exactly as
D1 §11.1 and D2 §12.6 say.** Note `cgroup.kill` is `--w-------` (write-only, root-only) — a detail
worth carrying into D5 §5.5. The whole write surface for phase 1 is about a dozen files. **No crate.**
`[R]` MED — the one place a library would genuinely earn its keep is cgroup **v1** (eight separate
hierarchies, per-controller path discovery) and **systemd delegation over D-Bus** — and D1 is right
that we want neither.

## 5.5 netlink: `rtnetlink` / `netlink-packet-route` / `netlink-sys` / `nftnl` / `nftables`

`[M]` V5 owns whether these hide the mechanism. I own health, API stability, `unsafe` surface and
advisories. One finding below is **behavioural and I am handing it to V5 explicitly** because it
changes what V5 is judging.

`[O]` HIGH — GitHub + crates.io, today:

| Crate | Version / date | Stars / issues / pushed | Advisories | Non-optional deps |
|---|---|---|---|---|
| `rtnetlink` | 0.23.0, 2026-08-18 | 190 / 36 / 2026-09-18 | none | `futures-channel`, `futures-util`, `log`, `netlink-packet-core 0.9`, `netlink-packet-route 0.33`, `netlink-proto 0.13`, `netlink-sys 0.9`, **`nix ^0.30.0`**, `thiserror 2` |
| `netlink-packet-route` | 0.33.0, 2026-08-18 | 77 / 17 / 2026-10-15 | none | `bitflags 2`, `libc`, `log`, `netlink-packet-core`, **`zerocopy 0.8`** |
| `netlink-sys` | 0.9.0, 2026-08-18 | 22 / 4 / 2026-09-13 | none | `bytes`, `libc`, `log` (async backends all optional) |
| `nftnl` | 0.9.4, 2026-08-11 | 110 / 6 / 2026-09-05 | **RUSTSEC-2025-0126** (patched ≥ 0.9.0) | `bitflags`, `log`, **`nftnl-sys` (FFI to C `libnftnl` + `libmnl`)**, **`nix ^0.31.2`** |
| `nftables` | 0.6.3, 2025-08-15 | — / — / — | none | `schemars`, `serde`, `serde_json`, `serde_path_to_error`, `strum`, `thiserror` |

`[R]` HIGH — **`rtnetlink` cannot go in the core crate, on two independent grounds, and this is a
substrate verdict not a behavioural one.** (a) It depends on **`nix ^0.30.0` non-optionally** —
which means taking `rtnetlink` re-introduces the exact dependency §1 and §12.1.7 spent their length
declining, at a two-minor lag. (b) `futures-channel` and `futures-util` are **non-optional**: the
crate's entire API is `async fn`. The `tokio` and `async-global-executor` features choose *which*
executor, not *whether*. D2 §8.3's table puts networking in the supervisor's sync world; `rtnetlink`
is structurally incompatible with that.

`[R]` HIGH — **the sync path exists and should be named: `netlink-sys` + `netlink-packet-route`,
without `rtnetlink`.** `netlink-sys`'s only non-optional deps are `bytes`, `libc` and `log`; every
async backend (`tokio`, `async-io`, `mio`, `futures-util`) is optional. `netlink-packet-route` is
pure encode/decode with no I/O at all. **That pair gives a synchronous, executor-free, `nix`-free
netlink stack** — which is what the project wants and what `rtnetlink` is not. Handing this to V5:
*the behavioural question is whether `netlink-packet-route`'s typed messages hide the RTM\_\*
mechanism; the substrate answer is that you can have the types without the executor.*

`[O]` MED — `netlink-packet-route` pulls **`zerocopy 0.8`**, which has a historical advisory
(RUSTSEC-2023-0074, *"Some `Ref` methods are unsound with some type parameters"*, patched ≥ 0.7.31).
0.8 is far past it. Noted for completeness, not a concern.

`[R]` HIGH — **`nftnl`: avoid.** It is FFI to the C `libnftnl` + `libmnl`, so it carries the exact
static-musl collision as `libseccomp` (§5.2.4, D2 §12.5). It depends on `nix ^0.31.2`. And it has
the only **memory-corruption advisory in this whole crate set**: **RUSTSEC-2025-0126** (2025-10-18),
*"Heap-buffer-overflow in `nftnl::Batch::with_page_size`"* — the C side allocates an insufficient
buffer on a small or malformed page size, causing out-of-bounds writes. Patched in 0.9.0 by adding
`batch_page_size.checked_add(nft_nlmsg_maxsize()).expect(…)`; **0.9.4 is safe**, and 61 K downloads
in 90 days means very few eyes on it.

`[R]` HIGH — **`nftables`: avoid, and it is not what it looks like.** `[O]` HIGH —
`nftables-0.6.3/src/helper.rs:13` is `const NFT_EXECUTABLE: &str = "nft"; // search in PATH`, and
lines 97, 171, 285 and 416 are each `Command::new(program)`. **It is not a netlink library at all;
it is a JSON-over-subprocess wrapper around the `nft(8)` binary.** Zero `unsafe` in 3,281 lines
because it does no syscalls. That is directly contrary to the project charter (*"we own the runtime
core, we do not shell out"*), it adds a runtime dependency on an external binary and its version, and
it hides the mechanism completely. **Handing to V5 as a behavioural finding, because it changes the
question from "does this abstraction hide the mechanism" to "is this an abstraction at all".**

## 5.6 tar / compression / hashing / HTTP

`[M]` V4 owns whether the `tar` crate's *defaults* are safe for untrusted layers. I own health,
API and advisories — and the advisory half turns out to be load-bearing, so §7 carries the detail.

| Crate | Version / date | Licence / MSRV | 90-day dl | Advisories | Verdict |
|---|---|---|---|---|---|
| `tar` | 0.4.46, 2026-05-18 | MIT/Apache-2.0, 1.63 | 67.7 M | **4** — see §7.3 | **Take, pin ≥ 0.4.45** |
| `flate2` | 1.1.10, 2026-08-28 | MIT/Apache-2.0, 1.67 | 197 M | none | **Take**; pure-Rust `miniz_oxide` backend is the default — no C, musl-clean |
| `zstd` | 0.14.3, 2026-10-16 | **BSD-3-Clause**, 1.64 | 103 M | none | **Take, with a build caveat** — `zstd-sys` compiles vendored C. Static musl works (it is `cc`-built, not `pkg-config`-found) but it does put a C compiler in the build requirements. |
| `sha2` | 0.11.1, 2026-10-12 | MIT/Apache-2.0, **1.88** | 326 M | RUSTSEC-2021-0100 (AVX2 miscompute, patched ≥ 0.9.8) | **Take** |
| `ring` | 0.17.14, 2025-03-11 | Apache-2.0 AND ISC, 1.66 | 195 M | **RUSTSEC-2025-0007: `ring` is unmaintained** | **Avoid** |
| `reqwest` | 0.13.5, 2026-09-08 | MIT/Apache-2.0, 1.85 | 231 M | none | Pull-process only (§6) |
| `ureq` | 3.5.0, 2026-10-21 | MIT/Apache-2.0, 1.85 | 74.9 M | none | **The sync option** — no `tokio`, no futures, anywhere in its non-optional deps |
| `hyper` | 1.12.0, 2026-10-06 | MIT, 1.63 | 249 M | 7, all ≤ 2022 and ancient | Only if we need HTTP/2 ourselves |

`[R]` HIGH — **`sha2` over `ring`, and the reason has changed since D3 framed the question.**
**RUSTSEC-2025-0007 (2025-02-20) declares `ring` unmaintained**, with no patched version: *"\*ring\*
is unmaintained"*. `briansmith/ring` was last pushed 2026-07-23 and 0.17.14 has not moved since
2025-03-11. It also carries RUSTSEC-2025-0009 (AES panic with overflow checks, patched ≥ 0.17.12).
`sha2` is RustCrypto, actively released (0.11.1, **2026-10-12**), 326 M downloads, pure Rust with
optional asm. The one cost to record: **`sha2 0.11`'s MSRV is 1.88**, which is *higher than `clap`'s
1.85* — so if we take `sha2 0.11` it, not `clap`, sets the project MSRV. D2 §12.5 made a point of
naming what sets the MSRV; this updates that note.

`[R]` HIGH — **`ureq` over `reqwest` for the registry client, and this reverses the natural reading
of D2 §8.3.** D2 §8.3 says the pull path *may* be async and names `reqwest`; §12.6 defers "an async
HTTP stack (`reqwest`/`hyper`/`tokio`)". Both are fine *because the pull path is a separate process*
(§6.2). But `ureq 3.5.0`'s non-optional dependency list is **`base64`, `log`, `percent-encoding`,
`ureq-proto`, `utf8-zero`** — no `tokio`, no `futures`, no `mio`. It is a genuinely synchronous HTTP
client with TLS behind a feature (`rustls` or `native-tls`). If the pull path is `ureq` + a
`std::thread` pool, **the "separate process" rule stops being load-bearing and becomes merely
tidy** — which is strictly better, because a structural rule you no longer depend on is a rule that
cannot be broken by accident. `[R]` MED — D3 owns the parallel-blob-download design; the substrate
note is that **sync HTTP with N threads is a real option here and it collapses the biggest
rewrite-risk decision in the project (D2 §8.3) into a non-decision.** Record it; don't decide it.

## 5.7 The micro-crates — brief verdicts

The question for each: does it earn a dependency over ~20 lines of our own? All figures crates.io /
GitHub, today.

| Crate | Latest / date | 90-day dl | Verdict | Why |
|---|---|---|---|---|
| **`pentacle`** | 1.1.0, **2024-10-03** | 56.7 K | **Take** — unchanged from D2 §12.6 | Deps are `libc` + optional `log`. `ensure_sealed()` is the ~60 lines §7.3/§11.8 demand for CVE-2019-5736, and youki calls it as line 1 of `main()` (D2 §14 item 2). Two years without a release is *finished*, not abandoned, for a one-function crate. No advisories. The honest alternative is ~60 lines of `memfd_create` + `F_SEAL_*` + `fexecve` that we must then own forever. |
| **`memfd`** | 0.6.6, 2026-09-01 | 15.1 M | **Upgrade to Take, from D2's "defer"** | `[O]` HIGH — **its only dependency is `rustix ^1.0`.** D2 §12.6 deferred it at "~40 lines", costed as if it were a new dependency. It isn't: we already take rustix, so the marginal graph cost is **zero crates**. 15 M downloads, no advisories. If we want the seal in-tree rather than via `pentacle`, this is now free. |
| **`pathrs`** | 0.2.6, 2026-09-06 | 239 K | **Avoid for phase 1** | Built on `rustix ^1.1` (good), but also `itertools`, `memchr`, `once_cell`, `static_assertions`, `rustversion`. Licence is **MPL-2.0 OR LGPL-3.0-or-later** — D2 §12.6's *"the only copyleft licence in reach"* is CONFIRMED, but **CORRECTED in emphasis**: MPL-2.0 is *file-level* weak copyleft and does not reach our code, so the licence is an annoyance to record, not a blocker. The real reason to decline is D2's: `rustix::fs::openat2` + `ResolveFlags::{BENEATH, NO_MAGICLINKS, NO_SYMLINKS}` is the kernel resolving paths safely, and beats any userspace reimplementation. `cyphar/libpathrs` is healthy (148 stars, pushed today, 71 open issues). Revisit if we ever need safe resolution on a kernel without `openat2` (< 5.6). |
| **`safe-path`** | 0.1.0, **2022-04-22** | 105 K | **Avoid** | Four and a half years at 0.1.0, no release since. Kata's. Same job as `pathrs`, less maintained. Its only real consumer is youki. |
| **`procfs`** | 0.18.0, 2025-08-30 | 20.1 M | **Avoid now, take later — D2 §12.5 CONFIRMED exactly** | `[O]` HIGH — `default = ["chrono", "flate2", "procfs-core/default"]`, verbatim from `Cargo.toml`. D2's claim that a naive `procfs = "0.18"` pulls `chrono` *and* `flate2` for one integer is **exact**. Built on `rustix ^1.0.1`, 3 `unsafe` in 5,135 lines, no advisories. `eminence/procfs` pushed 2026-06-26 — the quietest of the healthy crates here. Take with `default-features = false` the day we need a second `/proc` file. |
| **`prctl`** | 1.0.0, **2015-12-15** | 1.89 M | **Avoid** | Eleven years without a release. The 1.9 M downloads are youki's `libcontainer` and its dependents, not independent adoption. Everything in it is in `rustix::thread::prctl` / `rustix::process::prctl` (§5.1.4). Zero reason. |
| **`nc`** | 0.9.8, 2026-06-03 | 208 K | **Avoid — D2 §12.4 finding 4 CONFIRMED** | Raw syscall wrappers including `clone3`. 208 K downloads is negligible for something that would own the single most important `unsafe` call in the project (§10.2). MSRV 1.85. Write the ~15 lines of §3.3 ourselves, with the two trap comments on them. |
| **`close_fds`** | 0.3.2, **2021-06-05** | 547 K | **Avoid — and D2's replacement was imaginary** | Five years stale. But §2.3.1: `rustix::io::close_range` **does not exist**, so D2 §12.6's one-line replacement is the §3.5 `libc::syscall(libc::SYS_close_range, …)` instead. Still one line. Still no crate. |
| **`command-fds`** | 0.3.3, 2026-04-10 | 3.3 M | **Avoid — D2 §12.5 CONFIRMED** | Google's, active, good at its job: mapping fds for `std::process::Command`. We do not spawn via `Command`; we `clone3` and the child arranges its own fds (D1 §10.1). Wrong shape. |
| **`sendfd`** | 0.4.5, 2026-09-26 | 2.9 M | **Avoid — D2 §12.5 CONFIRMED** | `libc` + optional `tokio`, healthy. But its API traffics in `RawFd` (`recv_with_fd(…, fds: &mut [RawFd])`), so the received pty master is owned by nothing until we wrap it — D2 §5.2's hang. `rustix::net`'s `RecvAncillaryMessage::ScmRights` yields `OwnedFd` directly. ~40 lines of `sendmsg`/`recvmsg` + `cmsg_space!` and the ownership is right by construction. |
| **`signal-hook`** | 0.4.5, 2026-10-04 | 65.9 M | **Avoid — D2 §12.5 CONFIRMED** | Deps `libc` + `signal-hook-registry`. Excellent crate, no advisories, very healthy. It exists to make *signal handlers* safe; D1 §8 and D2 §5.4 structurally avoid handlers in favour of `signalfd`/pidfd + `epoll`. Revisit only if phase 2's `SIGWINCH` forwarding needs one. |
| **`scopeguard`** | 1.2.0, **2023-07-17** | 240 M | **Defer — D2 §12.5 CONFIRMED** | Three releases in seven years at 240 M downloads/90 days is *finished*, not abandoned. No advisories. But D2 §9.3's guard wants a `.commit()` that disarms, named per resource so the rollback list can reuse one `Cleanup` enum — ~20 lines and better shaped. Take it the day the hand-rolled version grows a second axis. |


---

# Part 6 — the async/sync boundary

## 6.1 "tokio is fatal in the create path" — **CONFIRMED, mechanically, from tokio's own source**

D2 §8.2 asserts this `[F]` HIGH from reputation and from youki's dependency list. I verified the
mechanism, because the whole architecture rests on it.

`[O]` HIGH — `tokio-1.54.1`, the chain, three files:

```rust
// src/runtime/builder.rs:2184  fn build_threaded_runtime(&mut self) -> io::Result<Runtime> {
let worker_threads = self.worker_threads.unwrap_or_else(num_cpus);
let blocking_pool = blocking::create_blocking_pool(self, self.max_blocking_threads + worker_threads,
                                                   worker_threads);
let (scheduler, handle, launch) = MultiThread::new(worker_threads, …);
let _enter = handle.enter();
launch.launch();                                     // <-- :2236, inside build()

// src/runtime/scheduler/multi_thread/worker.rs:512
impl Launch {
    pub(crate) fn launch(mut self) {
        for worker in self.0.drain(..) { runtime::spawn_blocking(move || run(worker)); }
    }
}

// src/runtime/blocking/pool.rs:508
fn spawn_thread(&self, …) -> io::Result<thread::JoinHandle<()>> {
    let mut builder = thread::Builder::new().name((self.inner.thread_name)());
    …
    builder.spawn(move || { … })
}
```

`[R]` HIGH — **CONFIRMED and now `[F]` HIGH on mechanism, not reputation.** `Runtime::build()` for
the multi-threaded flavour calls `launch()` **inside `build()`**, which spawns one real
`std::thread` per worker through the blocking pool, `num_cpus` of them. `#[tokio::main]` is
`new_multi_thread` by default. So the threads exist **before the first line of `main`'s body runs**,
and a `clone3` from that process is a clone from a process with N poisoned-lock-capable thread
stacks and — per §4.2 — an inherited `header.multiple_threads` that a raw `clone3` does not reset.
D2 §8.2's `[R]` HIGH ("must run in a process that has **never had a second thread**") is correct and
its reason is now mechanical.

`[O]` HIGH — **one precision D2 does not have, and it matters for the "discipline vs structure"
argument.** `build_current_thread_runtime` (`builder.rs:1729`) does **not** call `launch()`. The
current-thread scheduler runs on the calling thread, and `create_blocking_pool` only *configures*
the pool — `spawn_thread` is called lazily on the first `spawn_blocking`. So a bare
`#[tokio::main(flavor = "current_thread")]` process genuinely has one thread **at construction**.

`[R]` HIGH — **and that is exactly why D2 §8.3's rejection of the "one binary, `#[tokio::main]` on
`pull` only" option is right, and why the precision strengthens rather than weakens it.** The
current-thread runtime is single-threaded *until something calls `spawn_blocking`* — and
`tokio::fs::*` is **implemented entirely in terms of `spawn_blocking`**, as is
`tokio::net::lookup_host` and therefore every `TcpStream::connect("host:443")`. So the invariant
holds only until the first file read or the first DNS lookup, at which point a thread appears and
never goes away. **"Never had a second thread" cannot be maintained by choosing a runtime flavour.**
D2 §8.3: *"this works but it is a discipline invariant rather than a structural one… Structural
beats disciplined."* CONFIRMED, and the failure mode is more ordinary than D2 implies — it is not a
careless `lazy_static`, it is `tokio::fs::read`.

## 6.2 Where async *is* appropriate — **DISPUTED in scope: D2 §8.3's table is right, but §5.6 removes the need**

`[R]` HIGH — D2 §8.3's boundary table is correct and I endorse every row:
create/start strictly sync; supervisor event loop sync `epoll`; layer extraction a thread pool, not
async; CLI read paths sync; registry pull the one place where real I/O parallelism pays.

`[R]` MED — **but the recommendation that follows from it should be weakened, in a good direction.**
§5.6 found that **`ureq 3.5.0` has no `tokio`, no `futures`, and no `mio` in its non-optional
dependency list** — it is a genuinely synchronous HTTP client with TLS behind a feature flag. Six
parallel blob downloads over `std::thread` + `ureq` is six threads and a `mpsc` channel; it gets the
same 6× wall-clock win D2 §8.3 is buying with async, and it needs no executor anywhere in the
project.

`[R]` MED — so the ordering of D2 §8.3's conclusions should invert:

1. **First choice: sync `ureq` + a `std::thread` pool in the pull path.** Then "no async runtime
   anywhere in the project" is simply true, and the separate-process rule becomes a nice-to-have
   rather than the enforcement mechanism for the biggest rewrite-risk decision in the project.
2. **Second choice, if D3 finds `ureq` insufficient** (HTTP/2 multiplexing to a registry, or
   connection-pool behaviour that matters at scale): **then** D2 §8.3's separate-process rule, in
   full, with `reqwest` + `tokio` in the `pull` binary only.

`[R]` HIGH — **D2 §8.3's structural rule stays in the spec either way**, because it costs nothing
when it is not needed and it is the only thing that saves us if choice 2 happens later. I am
disputing the *framing* ("async is genuinely the right tool"), not the *mechanism*. D3 owns the
decision; the substrate contribution is that the sync option is real and D2 did not price it.

## 6.3 Settling D2 §14 item 5 — does `tracing-subscriber`'s `fmt` layer deadlock post-fork? — **CORRECTED: no, not by itself. The lock belongs to the `MakeWriter`, and with the default writer it is `std::io::Stdout`'s.**

`[O]` HIGH — `tracing-subscriber-0.3.23/src/fmt/fmt_layer.rs:1017`, the entire event path:

```rust
fn on_event(&self, event: &Event<'_>, ctx: Context<'_, S>) {
    thread_local! { static BUF: RefCell<String> = const { RefCell::new(String::new()) }; }
    BUF.with(|buf| {
        let borrow = buf.try_borrow_mut();
        let mut a; let mut b;
        let mut buf = match borrow {
            Ok(buf) => { a = buf; &mut *a }
            _       => { b = String::new(); &mut b }      // re-entrancy fallback
        };
        let ctx = self.make_ctx(ctx, event);
        if self.fmt_event.format_event(&ctx, format::Writer::new(&mut buf)…, event).is_ok() {
            let mut writer = self.make_writer.make_writer_for(event.metadata());
            let res = io::Write::write_all(&mut writer, buf.as_bytes());
            if self.log_internal_errors { if let Err(e) = res { eprintln!("[tracing-subscriber] …"); } }
        }
        …
```

`[R]` HIGH — **three findings, and the first is the answer to item 5.**

1. **The formatting buffer is a `thread_local! { RefCell<String> }`, not a `Mutex`.** There is **no
   process-wide lock** in the format step, and the `try_borrow_mut()` + fresh-`String` fallback
   handles re-entrancy. After a `clone`, the child's TLS is its own private copy (§4.3), and it is
   un-borrowed unless the cloning thread was itself inside `on_event`. **So `tracing-subscriber`'s
   `fmt` layer does not, on its own, deadlock post-fork.** D2 §14 item 5 answered: **no.**
2. **The lock risk is entirely the `MakeWriter`'s**, and `make_writer_for` is called *inside*
   `on_event`:
   - default (`io::stdout` / `io::stderr` as `MakeWriter`) → `Stdout::write_all` → the
     **`ReentrantLock`** of `std::io::stdio` (§4.6) plus the inherited **`LineWriter`** buffer.
   - `Mutex<File>` (the documented way to log to a file, `src/fmt/writer.rs:808-816`) →
     `MakeWriter::make_writer` is `MutexGuardWriter(self.lock().expect("lock poisoned"))` — a real
     `std::sync::Mutex`, which a dead thread can leave locked, and which **panics** on poison.
   So D2 §8.3's hedge — *"confidence MED only on whether `tracing` specifically deadlocks post-fork;
   it depends on the subscriber"* — is **CONFIRMED verbatim and for exactly the right reason.**
3. **It allocates unconditionally.** The buffer is a `String` that `format_event` grows, and the
   fallback branch is a fresh `String::new()`. So even with a lock-free writer, `tracing` in the
   child is forbidden by §4.1 regardless.

`[R]` HIGH — **what changes and what does not.** Nothing structural: D2 §8.3's `[R]` ("the child
gets a raw fd and `write(2)`s pre-formatted bytes") and §12.5's `[R]` HIGH are correct and stay.
What changes is the **supervisor-side** advice, which is what item 5 was actually asking about:

- D2 §8.3 says *"never [log] between the last log call and the `clone()`"*, implying a
  tracing-specific lock. There isn't one. The real hazard immediately before `clone3` is
  **`std::io::Stdout`'s `ReentrantLock` and its `LineWriter` buffer** — the same hazard any
  `println!` creates, tracing or not.
- The mitigation is therefore **§4.6's two lines** (`io::stdout().flush()`, `io::stderr().flush()`
  immediately before the clone, beside the `Threads == 1` assertion), **not** a rule about tracing
  call sites. A rule about call sites is unenforceable; two statements are.
- `[R]` MED — and if the supervisor ever logs to a **file** via `with_writer(Mutex::new(file))`,
  that *is* a real `Mutex` with a `.expect("lock poisoned")` and the pre-clone flush does not help.
  Use a dedicated append-only fd and `write(2)`, or `tracing-appender`'s non-blocking writer — but
  note `tracing-appender`'s non-blocking writer **spawns a thread**, which §6.1 forbids outright in
  the supervisor. Record both; the safe answer is a raw fd.

---

# Part 7 — the advisory and build audit §2.8 promised

## 7.0 Method and the "nothing new" sweep

`[M]` `git clone --depth 1 https://github.com/RustSec/advisory-db`, head **`84d943a6`, committed
2026-10-23 16:42 +0200** — i.e. today. I enumerated `crates/<name>/RUSTSEC-*.md` for all 45 crates
in or adjacent to the recommended set and parsed each advisory's TOML front matter for `date`,
`patched`, `informational`, `categories` and `affected.functions`.

`[O]` HIGH — **the sweep the brief asked for: 95 advisories were filed DB-wide on or after
2026-09-04, and exactly 9 touch our crate set.** They are listed in §7.4. The other 86 are
`wasmtime`, `ammonia`, `pest`, `anchor-lang` and similar — nothing adjacent.

`[O]` HIGH — **crates in our set with zero advisories, ever:** `rustix`, `libc`, `caps`, `oci-spec`,
`cgroups-rs`, `seccompiler`, `libseccomp`, `flate2`, `zstd`, `ureq`, `reqwest`, `serde`,
`serde_json`, `clap`, `thiserror`, `procfs`, `pathrs`, `memfd`, `pentacle`, `nc`, `prctl`,
`close_fds`, `command-fds`, `sendfd`, `signal-hook`, `scopeguard`, `rtnetlink`,
`netlink-packet-route`, `netlink-sys`, `nftables`, `bitflags`, `linux-raw-sys`.

## 7.1 The named advisories, one at a time

| Advisory | Crate | Filed | Kind | Patched | Our exposure |
|---|---|---|---|---|---|
| **RUSTSEC-2021-0119** | `nix` | 2021-09-27 | memory-corruption | `^0.20.2, ^0.21.2, ^0.22.2, >= 0.23.0` | **None.** §2.8's claim that `nix` has exactly one advisory ever is **CONFIRMED**. Out-of-bounds write in `nix::unistd::getgrouplist` — a stack buffer sized from `sysconf(_SC_NGROUPS_MAX)` that the kernel is allowed to exceed. Ancient, patched nine minor versions ago, and we are not taking `nix`. Keep it in the record only because D2 §12.1.5's table cites it. |
| **RUSTSEC-2026-0190** | `anyhow` | 2026-06-25 | `informational = "unsound"`, memory-corruption | `>= 1.0.103` | **None at 1.0.104** (current, 2026-07-18). *"Unsoundness in `Error::downcast_mut()`"* — violates borrow rules, UB, when you `Error::context(…)` then `downcast_mut` the result. `affected.functions` names exactly `anyhow::Error::downcast_mut`. D2 §12.5's *"patched ≥ 1.0.103, current release is past it"* is **CONFIRMED exactly**. We never call `downcast_mut`; and §12.6 confines `anyhow` to the CLI crate anyway. |
| **RUSTSEC-2023-0078** | `tracing` | 2023-10-19 | `unsound`, memory-corruption | `>= 0.1.40` | **None at 0.1.44.** *"Potential stack use-after-free in `Instrumented::into_inner`"* — an async-instrumentation API we will not use. **CONFIRMED historical**, as D2 §12.5 says. |
| **RUSTSEC-2025-0055** | `tracing-subscriber` | 2025-08-29 | **format-injection** (not memory) | `>= 0.3.20` | **None at 0.3.23.** Aliases **CVE-2025-58160**, GHSA-xwfj-jgwm-7wp5 — D2 §12.5's CVE number is **CONFIRMED**. *"Logging user input may result in poisoning logs with ANSI escape sequences."* `[R]` MED — **this one is not purely historical for us**, because a container engine logs attacker-controlled strings routinely: image names, OCI annotation values, `config.json` `process.args`, container names. 0.3.23's fix is the `with_ansi_sanitization` path visible in §6.3's `on_event` quote — **make sure it stays enabled**, and do not reach for `with_ansi(false)` thinking it is the same control. |
| **RUSTSEC-2021-0072** | `tokio` | 2021-07-07 | memory-corruption | `>= 1.8.1` (et al.) | None. *"Task dropped in wrong thread when aborting `LocalSet` task."* |
| **RUSTSEC-2021-0124** | `tokio` | 2021-11-16 | memory-corruption, thread-safety | `>= 1.13.1` | None. *"Data race when sending and receiving after closing a `oneshot` channel."* |
| **RUSTSEC-2023-0001** | `tokio` | 2023-01-04 | — | `>= 1.23.1` | None. `reject_remote_clients` config corruption — **Windows named pipes only.** Irrelevant on Linux; D2's table should say so. |
| **RUSTSEC-2023-0005** | `tokio` | 2023-01-11 | `unsound`, memory-exposure | `>= 1.24.2` | None. *"`tokio::io::ReadHalf<T>::unsplit` is Unsound."* |
| **RUSTSEC-2025-0023** | `tokio` | 2025-04-07 | `unsound`, memory-corruption | `>= 1.44.2` | None. *"Broadcast channel calls clone in parallel, but does not require `Sync`."* |

`[R]` HIGH — **the five named `tokio` advisories are all ancient and all patched. D2's table was
accurate and is still accurate. But it is now incomplete — see §7.4.**

## 7.2 §2.8's rustix claim — **CONFIRMED: zero advisories, and my hedge stands**

`[O]` HIGH — `crates/rustix/` **does not exist** in the advisory DB at head `84d943a6`. Zero
RUSTSEC advisories in rustix's lifetime, including the seven weeks since the research.
**D2's claim CONFIRMED.**

`[R]` HIGH — and §2.8's deflation stands unchanged: **1,792 occurrences of `unsafe` in 71,950 lines
across 315 files, plus per-architecture inline `asm!`. Zero advisories on that surface is mildly
surprising, not reassuring.** For calibration against the rest of the set, measured this session:

```
rustix 1.1.5            1792 unsafe / 71,950 lines / 315 files
libseccomp 0.4.0          52 unsafe / 10,275 lines
tracing-subscriber 0.3.23 28 unsafe / 23,184 lines
cgroups-rs 0.5.1          15 unsafe / 12,052 lines
caps 0.5.6                10 unsafe /  1,044 lines
seccompiler 0.5.0          5 unsafe /  3,815 lines
procfs 0.18.0              3 unsafe /  5,135 lines
oci-spec 0.10.0            3 unsafe / 10,462 lines
nftables 0.6.3             0 unsafe /  3,281 lines   (because it shells out — §5.5)
```

The structural argument (no libc buffer contracts to misread on the Linux path) is the one that
carries weight; the advisory count is decoration.

## 7.3 The advisories D2's table does not have, because D2 did not cover these crates

`[O]` HIGH — four that a specification session must see:

| Advisory | Crate | Filed | Verdict |
|---|---|---|---|
| **RUSTSEC-2025-0007** | `ring` | 2025-02-20 | **`informational = "unmaintained"`, no patched version: *"\*ring\* is unmaintained."*** `briansmith/ring` last pushed 2026-07-23; 0.17.14 has not moved since 2025-03-11. Plus **RUSTSEC-2025-0009** (AES panic with overflow checks, patched ≥ 0.17.12) and **RUSTSEC-2025-0010** (pre-0.17 unmaintained). `[R]` HIGH — **this settles D3's `sha2`-vs-`ring` question: `sha2`.** §5.6. |
| **RUSTSEC-2026-0067** | `tar` | 2026-03-19 | CVE-2026-33056. *"`unpack_in` can chmod arbitrary directories by following symlinks"* — `unpack_dir` used `fs::metadata()` (which follows symlinks) to test whether an existing path is a directory, so a crafted tarball with a symlink entry followed by a same-named directory entry gets the **symlink target** chmod'd, outside the extraction root. `affected.functions`: `tar::Entry::unpack`, `tar::Entry::unpack_in`, `< 0.4.45`. Patched **≥ 0.4.45**. |
| **RUSTSEC-2026-0068** | `tar` | 2026-03-19 | CVE-2026-33055. *"tar-rs incorrectly ignores PAX size headers if header size is nonzero"* — tar-rs was the **outlier**; Go's `archive/tar` and others unconditionally honour the PAX size override. A size discrepancy between parsers is a tar-smuggling primitive: the same archive unpacks differently under different tools. Patched **≥ 0.4.45**. |
| **RUSTSEC-2025-0126** | `nftnl` | 2025-10-18 | *"Heap-buffer-overflow in `nftnl::Batch::with_page_size`"* — the C side allocates an insufficient buffer for a small/malformed page size, OOB writes during batch init. Patched ≥ 0.9.0 by `checked_add(...).expect(...)`. 0.9.4 is safe. §5.5 declines the crate anyway. |

`[R]` HIGH — **the two `tar` advisories matter even though 0.4.46 is already past them.** They were
filed **2026-03-19, six months before the research date**, and the research corpus does not mention
them. `tar 0.4.46` (2026-05-18) satisfies `>= 0.4.45`, so the *version* is fine — but the two
findings describe exactly the attack surface **V4 owns** ("are the `tar` crate's defaults safe for
untrusted layers?"), and the second one is not a bug so much as a **statement that tar parsing is
not canonical**. Handing to V4: *pin `tar >= 0.4.45` explicitly rather than `"0.4"`; and the
PAX-size divergence means an image whose layer digest is computed by one parser and whose contents
are extracted by another can disagree — which is D3's content-addressing problem, not just a
hardening note.*

## 7.4 Filed since 2026-09-04 — **the drift sweep, 9 hits**

`[O]` HIGH — ordered by date:

| Filed | Advisory | Crate | What | Status for us |
|---|---|---|---|---|
| 2026-09-14 | RUSTSEC-2026-0285 | `rustls` | TLS 1.3 handshake messages accepted across encryption-level boundaries | patched ≥ 0.23.45 |
| **2026-10-13** | **RUSTSEC-2026-0386** | **`tokio`** | `informational = "unsound"`. *"`AsyncFd::new` and related constructors violate I/O safety"* — they take any `T: AsRawFd`, so safe code can build an `AsyncFd` from a closed or arbitrary fd; and a dup'd-then-closed fd whose number is reused leads to a **use-after-free** when a readiness event fires. `new`/`with_interest`/`try_new`/`try_with_interest` are now deprecated in favour of `unsafe` `register`/`register_with_interest`. Patched `>= 1.51.6, < 1.52.0` or `>= 1.53.3`. | **Relevant in a way D2 could not have known.** `AsyncFd` is precisely the API a tokio-based supervisor would use to wrap a `signalfd`, a `pidfd` or the exec FIFO. This is a direct, dated argument for D2 §8.3's sync `epoll` loop. |
| 2026-10-14 | RUSTSEC-2026-0346/-0347/-0348/-0349 | `rustls` | `Debug` impls leak TLS secrets; buffered app data sent to outer server after ECH rejection; brotli cert decompression can allocate **1 GiB pre-authentication**; unauthenticated ServerHello can downgrade later connections | all patched ≥ **0.23.46** |
| **2026-10-16** | **RUSTSEC-2026-0370** | **`zvariant`** | `informational = "unmaintained"`, **no patch**. Companions: RUSTSEC-2026-0371 `zbus_names` unmaintained, RUSTSEC-2026-0372 `zvariant_derive` unmaintained. | **These are `zbus`'s own core dependencies, and `zbus` is `cgroups-rs`'s unconditional dependency (§5.4).** Taking `cgroups-rs` now means importing three crates the advisory DB marks unmaintained, seven days ago. |
| 2026-10-19 | RUSTSEC-2026-0381 | `webpki-roots` | bundled roots trust distrusted emSign intermediates | patched ≥ 1.0.10 (current 1.0.11) |
| 2026-10-20 | RUSTSEC-2026-0382 | `rustls-webpki` | certificate verification may accept distrusted intermediates | patched ≥ 0.103.16 (current 0.103.16) |
| **2026-10-21** | **RUSTSEC-2026-0384** | **`tokio`** | *"Race condition in `tokio::time::Interval` on Tokio v1.54.0"* — a data race and **use-after-free on the timer waker in safe Rust** between `Interval::poll_tick` and the time driver on the multi-threaded runtime. **Tokio v1.54.0 has been yanked.** Patched ≥ **1.54.1** (released the same day). | **Two days old.** If the pull path uses tokio, `tokio = "1"` must resolve to ≥ 1.54.1 and 1.54.0 must not be in `Cargo.lock`. |

`[R]` HIGH — **the registry-client pins, as a block, because D3 will need them and they are all
less than six weeks old:** `rustls >= 0.23.46`, `rustls-webpki >= 0.103.16`, `webpki-roots >= 1.0.10`,
`tokio >= 1.54.1` (if tokio at all). `ureq 3.5.0` and `reqwest 0.13.5` both resolve `rustls`
transitively, so this applies whichever HTTP client D3 picks.

`[R]` MED — RUSTSEC-2026-0348 (brotli certificate decompression allocating up to 1 GiB **before
authentication**) is worth one line in D3's threat model: a hostile or MITM'd registry endpoint is
an unauthenticated remote attacker against our TLS stack, and `pull` is the one place our process
talks to one.

## 7.5 Settling D2 §14 item 6 — will `rustix`(`linux_raw`) + no `libseccomp` build a static musl binary on stable? — **WEAK-EVIDENCE → determined from build requirements; yes, and the test is named**

I cannot compile for Linux here. But the question decomposes into facts that *are* checkable, and
all of them check out.

`[O]` HIGH — **1. The target is built into stable rustc, and `crt-static` is its default.** Run
locally, `rustc 1.96.0` (stable, darwin/aarch64):

```
$ rustc --print cfg --target x86_64-unknown-linux-musl   |  $ rustc --print cfg --target x86_64-unknown-linux-gnu
target_env="musl"                                        |  target_env="gnu"
target_feature="crt-static"      <-- default ON          |  (no crt-static)
target_feature="fxsr" "sse" "sse2"                       |  target_feature="fxsr" "sse" "sse2"
```

So static linking is the **default** for that target, not a flag, and naming the target needs no
nightly. `rustup target add x86_64-unknown-linux-musl` ships a prebuilt `libc.a`
("self-contained" linking), so no external `musl-gcc` is required **provided no crate runs a C
compiler**.

`[O]` HIGH — **2. `rustix` picks `linux_raw` on musl, and its `build.rs` compiles no C.**
`rustix-1.1.5/build.rs:101-127` (unchanged from the 1.1.4 reading in §2.4) selects the libc backend
only if `feature_use_libc || cfg_use_libc || os != "linux" || !inline_asm_name_present ||
is_unsupported_abi || miri || (ppc/s390x/mips && !experimental_asm)`. On
`x86_64-unknown-linux-musl` every disjunct is false → **`linux_raw`**. The backend decision is on
`target_os`, not on `target_env`, so musl is indistinguishable from gnu to it. `grep -c "cc::\|pkg_config"`
over `rustix-1.1.5/build.rs` → **0**.

`[O]` HIGH — **3. No crate in D2 §12.6's recommended core set runs a C compiler or `pkg-config`.**
Measured:

```
rustix-1.1.5       build.rs=yes   build-dependencies: (none)
libc-0.2.191       build.rs=yes   build-dependencies: (none)
seccompiler-0.5.0  build.rs=no
oci-spec-0.10.0    build.rs=no
libseccomp-0.4.0   build.rs=yes   build-dependencies: pkg-config   <-- the only one
```

`serde`, `serde_json`, `thiserror`, `tracing`, `clap`, `anyhow`, `pentacle` are pure Rust.
**`libseccomp` is the single crate in the whole candidate set that needs a C library**, and §5.2
declines it in favour of `seccompiler` (deps: `libc` only, no `build.rs`).

`[R]` HIGH — **4. Why youki needs nightly + `-Zbuild-std` and we do not.** D2 §12.3 records
youki's README requirement. The reason is its dependency set, not musl: `libcontainer` lists
`libseccomp` (C, §5.2.4), `nc`, `caps`, `nix`, `prctl`, `rust-criu` and `netlink-packet-*`. A C
library that must itself be musl-built, plus `rust-criu`'s FFI, is the thing that forces
`-Zbuild-std`. **D2 §12.1.4's "musl becomes uninteresting" is therefore CONFIRMED in substance and
should be restated in the form that is actually checkable: *the musl obstacle is C dependencies, and
our recommended set has exactly one candidate C dependency (`libseccomp`), which §5.2 declines.***

`[R]` HIGH — **5. The one residual risk I cannot discharge from here**, and it should be the first
thing the dev VM checks: `zstd` (D3's layer decompression) depends on `zstd-sys`, which **compiles
vendored C with the `cc` crate**. That is not a `pkg-config` lookup and it generally works for musl,
but it does mean a C cross-compiler must be present for the *pull/extract* binary. The **core**
binary is unaffected, because §5.6 puts compression in the pull path and §8.3 puts the pull path in
its own process. `[R]` MED — if that turns out to be painful, `ruzstd` (pure Rust decompressor) is
the escape hatch; D3's call.

`[O]` **UNVERIFIABLE-HERE — the exact test, for the dev VM:**

```bash
rustup target add x86_64-unknown-linux-musl
cargo build -p core --target x86_64-unknown-linux-musl --release
ldd  target/x86_64-unknown-linux-musl/release/mydocker   # expect: "not a dynamic executable"
file target/x86_64-unknown-linux-musl/release/mydocker   # expect: "statically linked"
cargo tree -p core --target x86_64-unknown-linux-musl -e normal --prefix depth   # expect: no tokio, no nix, no zbus, no *-sys
# and, because §6.1 is the invariant everything rests on:
cargo tree -p core -e normal -i tokio   # expect: "error: package ID specification `tokio` did not match any packages"
```

`[R]` HIGH — **make the last two `cargo tree` lines a CI step, not a one-off.** §12.6's two-crate
split is the structural rule; a `cargo tree -i tokio` that must fail is how the rule announces
itself when someone breaks it.

## 7.6 Settling D2 §14 item 7 — `nix`: maintenance decline, or between releases? — **CORRECTED. Both D2 and my own §1.4 are now out of date, and the issue tracker has the maintainer's own words.**

`[O]` HIGH — **first, a correction to my own file.** This document's header says *"Date of
verification: 2026-09-04"* and §1.4 computes *"from 2026-05-19 to today (2026-09-04) is 15.4 weeks."*
**Today is 2026-10-23.** The header date is wrong and §1.4's arithmetic is anchored to it. Per the
contract I am appending rather than rewriting, so: **§1.4's figures are as-of 2026-09-04; §7.6 is
as-of today and supersedes them.**

`[O]` HIGH — **`nix` has resumed committing.** GitHub API today: `nix-rust/nix`, stars **3082**,
open issues **370** (was 354), `pushed_at` **2026-10-18**, not archived. The commit log:

```
2026-10-17  89339c61  xtqqczze  fix: update aio feature to use pin-project-lite (#2843)
2026-10-14  3eae6d68  xtqqczze  fix: allow usage of deprecated pin_utils (#2839)
2026-10-14  439941ed  xtqqczze  ci: fix environment variable reference for MSRV (#2844)
2026-10-14  f9ba3ed0  xtqqczze  fix: mark macOS-only items (#2821)
2026-09-29  e35c0089  xtqqczze  fix rustdoc lints (#2818)
2026-09-29  ddd57012  xtqqczze  fix: bump cfg_aliases minimum version to 0.2.2 (#2808)
-- 19-week gap --
2026-05-19  fb799660  wngr      feat(socket): decode (SOL_SOCKET, SO_MARK) cmsg …
```

`[R]` HIGH — **the answer to item 7 is "neither, precisely": it was a 19-week stall, now broken by
six commits from a single new contributor doing lint, CI and docs hygiene.** Not feature work. Not
`sched`. Not a release — crates.io still shows **0.31.3, 2026-05-11** as the newest version, five
and a half months ago. So §1.4's conclusion ("a crate with contributors and no maintainer bandwidth")
is **CONFIRMED and sharpened**: the contributors rotate, the steering does not exist.

`[O]` HIGH — **the maintainer capacity statement D2 asked for, found.** `nix-rust/nix` issue
**#1304, "Nix needs more maintainers"** — opened **2020-10-03**, **still open**, last updated
**2026-09-24**, 10 comments. The maintainer's own words:

> *"You may have noticed that my responsiveness has been lagging the past few months. COVID has
> given me less time to work on Nix, though TBH I've been burning out, too. Would somebody
> volunteer to help me with this crate? Old maintainers are welcome, as are outside contributors
> with good track records."*

`[R]` HIGH — **a six-year-old open "we need maintainers" issue, still being updated last month, is
a stronger and more citable signal than any commit-frequency statistic.** Issue #2132 *"New
maintainers wanted"* was opened and **closed** (2025-03-24) in between. Item 7: **settled.** It is
decline, it is self-reported, it has been self-reported since 2020, and §1.1's technical grounds
remain the reason to decline `nix` regardless.

`[O]` HIGH — **two more open issues that strengthen §1.1 and §1.2, which neither D-file has:**

1. **#2797, "Nix 0.31.2 Security & Soundness Audit findings"** — opened 2026-05-18, **open**, last
   updated 2026-08-20. A third-party manual audit of all `src/` unsafe code (`cargo geiger`:
   46/2039 unsafe expressions) filed findings graded **CRITICAL**, including:
   - *C1. `dup2_raw` missing error check — wraps `-1` in `OwnedFd`* (`src/unistd.rs:625-639`):
     `libc::dup2` returns −1 on error, unchecked, then `OwnedFd::from_raw_fd(-1)`, so `Drop` calls
     `close(-1)`. *"Impact: UB via invalid `OwnedFd`; potential close of wrong fd in multithreaded
     programs."* The audit notes `dup3_raw` next door does it correctly.
   - *C2. `SigSet::wait()` uses the wrong error convention for `sigwait(3)`* — `sigwait` returns a
     positive errno, never −1, so `Errno::result(res)` silently treats every failure as success.
   **Open and unhandled five months later.** `[R]` HIGH — this is not an advisory (nothing has been
   filed with RUSTSEC) but it is exactly the evidence D2 §12.1.5's "caution flag" needed, and it
   lands in `unistd` and `sys::signal` — two of the four modules D1 §11.1 wanted `nix` *for*.
2. **#2802, "`fcntl::Flock` cannot be unlocked without potentially panicking"** — opened
   2026-06-18, open, 0 comments. `[R]` HIGH — **this is a correction to my own §1.6**, where I wrote
   *"Flagging it because nobody did."* **Someone did: upstream, four months ago.** My finding is
   independently confirmed, but the credit is wrong and the claim should read "flagged upstream in
   nix#2802 and not flagged in D4."
   And the issue carries a hazard I did not have, which **D4 needs**: `File::try_clone()` breaks the
   RAII wrapper entirely, because `flock(2)` locks the **open file description**, not the file
   descriptor —
   ```rust
   let lock1 = Flock::lock(f1, LockExclusive).unwrap();
   let lock2 = Flock::lock(f1.try_clone().unwrap(), LockExclusive).unwrap(); // two "exclusive" locks
   drop(lock1);                                                              // unlocks BOTH
   ```
   `[R]` HIGH — **for D4's state-directory locking that is a correctness bug, not a nit.** Any code
   path that `try_clone()`s the state-dir file — passing it to a helper, storing a second handle —
   silently drops the lock on the first `Drop`. The mitigation is the same either way: **one owner
   of the lock fd, never cloned**, and the `Flock` strictly parent-side (§1.6).
3. **#919, "`sched::clone` doesn't pass ownership of closure to new thread"** — open since
   **2021-08-11**. A *third* open issue on the §1.2 defect, alongside D1's #360 (2016) and #2803
   (2026). Three issues, ten years, one unsound function.

## 7.7 Settling D2 §14 item 8 — rustix's bus factor — **CONFIRMED as a fact; the mitigation, stated to be acted on**

`[O]` HIGH — `bytecodealliance/rustix` today: **stars 2097, open issues 146, `pushed_at` 2026-09-16,
archived = false**, and **1.1.5 shipped 2026-09-16** — i.e. a release landed *after* the research
date and the project is plainly alive. `Cargo.toml` still lists exactly two authors
(`Dan Gohman <dev@sunfishcode.online>`, `Jakub Konka`). 315 M downloads in 90 days.

`[O]` MED — I still **could not** independently re-derive D2's *"sunfishcode authored 22 of the last
61 commits"*; `/stats/contributors` returns 202 while GitHub computes it. **WEAK-EVIDENCE on the
arithmetic, as in §2.7.** The qualitative claim is not in doubt.

`[R]` HIGH — **item 8: it is a risk to accept knowingly, and §2.7 already states the right reason.
What was missing is the mitigation written as something to *do*, so here it is, as three concrete
commitments:**

1. **Keep the written-down raw-syscall list from §3.6 current.** Seven calls today. The list is not
   bookkeeping — it is the inventory of what we would have to write if rustix vanished, and it only
   stays small if it is maintained.
2. **Confine rustix to a named internal module (`sys::`) that re-exports only our subset.** Our use
   is ~40 thin wrappers. If every call site says `crate::sys::openat2` rather than
   `rustix::fs::openat2`, replacing rustix is editing one file; if call sites name rustix directly,
   it is editing the project. This costs ~60 lines now and is the difference between a mechanical
   migration and an architectural one.
3. **Pin exactly and read `not_implemented.rs` on every bump.** §2.1's `libc_internals` /
   `quite_yet` / `yet` partition is rustix's own statement of its boundary, and it is where
   `mount_setattr` and `close_range` will appear if they ever do. (They have not: **both still
   absent in 1.1.5**, verified §3.6.)

`[R]` MED — and §2.7's cost I would still raise above D2's ranking stands: **`linux_raw` is
per-architecture inline `asm!`, and it is where our architecture portability actually lives.**
A release landing on schedule does not change that.


---

## Corrections that change a recommendation

Covering Parts 1–7. One line each: *what the D-file said → what is actually true → which `[R]` it
invalidates or shifts.*

**Substrate facts that are simply wrong**

1. D2 §12.5/§12.6 (×3): *"`rustix::io::close_range` covers it, one syscall, no crate"* → **`close_range` does not exist in rustix, verified absent in 1.1.4 and again in 1.1.5** → shifts D2 §12.6's `close_fds` row and makes D1 §11.2 step 30 a hand-written call; the `libc` surface grows by one (§2.3.1, §3.5).
2. D2 §12.1.2 table and §12.1.7 justification 2: *"`nix` lacks `openat2`"* → **`nix-0.31.3/src/fcntl.rs:389` has `openat2` with a full `ResolveFlag` set** → does not change the verdict on `nix` but removes one of §12.1.7's four legs; the claim is checkable and wrong (§1.5).
3. D2 §12.6's `libc` row: *"five calls"* → **the written-down list is seven calls and one hand-defined constant** (`clone3`, `execve`, `_exit`, `sigprocmask`, `signalfd`, `close_range`, `mount_setattr`, plus `CLONE_INTO_CGROUP`) → §12.6's take-table must gain two rows (§3.6).
4. D1 §11: *"~200 lines of our own for `clone3`, `mount_setattr`, `close_range`, `capget`/`capset`, `prctl(NO_NEW_PRIVS)`"* → **correct for D1's `libc`+`nix` stack; ~180 of those lines are `capget`/`capset`, which `rustix::thread::libcap` already supplies** → the two documents' "5 calls" and "200 lines" are not in conflict, they price different stacks, and this is a better argument for rustix than any in §12.1.7 (§3.1.4, §3.6).
5. D2 §12.5 `caps` row: *"five sets, one more than rustix's `capset`-shaped API exposes"* → **rustix covers all five** (`capabilities`/`set_capabilities` + 4 bounding/ambient prctls) **and two more of D5 §8.5's steps (`set_no_new_privs`, `set_keep_capabilities`) that `caps` does not cover at all** → strike the parenthetical; the coverage runs the other way (§5.1.4).
6. D2 §14 item 10: *"`oci-spec` 0.10.0's `runtime::VERSION` says `1.1.0-dev`"* → **it says `"1.1.0"`; `VERSION_DEV` is the empty string** → the rest of the item (`Spec::default()` emits `"1.0.2-dev"`, structs carry 1.3.0) is confirmed, and no newer release exists (§5.3.1).

**A live landmine neither document could have known about**

7. **`libc::CLONE_INTO_CGROUP` is `c_int = 0x200000000` on every glibc target — it truncates to `0` — and is not defined at all on musl** (libc 0.2.191, `#[deprecated]` since 0.2.188, issue #3584, fixed only on `main`) → **D5 §1's race-free cgroup placement silently no-ops**: `clone3` succeeds and the child lands in the parent's cgroup, with no error. Define it ourselves as `const CLONE_INTO_CGROUP: u64 = 1 << 33;` and `#![deny(deprecated)]` in the core crate. Same for `CLONE_CLEAR_SIGHAND` (§3.4).

**Mechanism corrections that change what the rule should say**

8. D1 §11.2: *"the child's first allocation deadlocks"* (unconditional) → **glibc's `fork()` takes the malloc arena and libio locks before the clone and resets them in the child; a raw `clone3` runs none of that** → the rule is right but the reason must be restated: *a raw `clone` is strictly more dangerous than `fork()`, because `fork()` has a recovery path and `clone3` has none* (§4.1).
9. D1 §11.2 `[C]` HIGH: *"glibc's `setxid` broadcasts and therefore hangs"* → **the broadcast is conditional on `!SINGLE_THREAD_P`, a TCB flag a raw `clone3` inherits and does not reset; and `__nptl_setxid`'s *first* statement takes `dl_stack_cache_lock`, so it can hang before signalling anything** → same `[R]` (raw syscalls), better reason, and it explains exactly why youki's "only one thread" comment is the justification it is (§4.2).
10. D1 §11.2 `[F]` **MED** on the Rust allocator mapping → **`rust-lang/rust@1.90.0/library/std/src/sys/alloc/unix.rs`: `System::alloc` is `libc::malloc`, `alloc_zeroed` is `libc::calloc`** — the exact file D1 named as what would settle it → **upgrade to `[F]` HIGH** (§4.1).
11. D1 §11.2's `[O]` DROPLET duplicated-output observation → **it was a Python parent (fully buffered stdio); Rust's `Stdout` is a `LineWriter` unconditionally, so it would not reproduce** → the conclusion holds, the evidence should be re-labelled as corroborating the mechanism in C, and the real Rust vectors are our own `BufWriter`s, `print!` without `\n`, and `tracing`'s buffer (§4.6).
12. D2 §14 item 9 (*"load-bearing for the whole rustix-not-nix decision"*) → **SETTLED: `rustix::thread::set_thread_{uid,gid,res_uid,res_gid,groups}` issue the raw syscall on *both* backends** (`linux_raw`: `syscall_readonly!(__NR_setuid, …)`; libc backend: `syscall! { … via SYS_setuid }` → `libc::syscall`, never `libc::setuid`) → **rustix *is* youki's hand-written mitigation, packaged.** `nix::unistd::setuid` by contrast calls the glibc wrapper, i.e. the broadcast path (§4.4).
13. D2 §14 item 5 → **SETTLED: no.** `fmt_layer::on_event`'s buffer is a `thread_local! RefCell<String>` with a re-entrancy fallback, not a `Mutex`. The lock belongs to the `MakeWriter`: default → `std::io::Stdout`'s `ReentrantLock` + inherited `LineWriter`; `Mutex<File>` → a real `Mutex` that `.expect("lock poisoned")`-panics → the pre-`clone` mitigation is **two `flush()` statements**, not a rule about `tracing` call sites (§6.3).
14. D2 §14 items 3 and 4 → **CLOSED.** Item 3: `Arg for &CStr/&CString/CString` is the identity (`f(self)`), so pre-built `CString`s are an *elimination*, not a mitigation — but the slow path at **≥ 256 bytes** (`SMALL_PATH_BUFFER_SIZE`) is live in D2's own recommended feature set, so the discipline is mandatory and the test must use a ≥ 256-byte path. Item 4: `errno` is well-defined in a non-`CLONE_VM` child, and `linux_raw` has zero TLS accesses, so the question does not arise in our subset (§4.5, §4.3).

**Crate verdicts that flip or shift**

15. **D5 §19.2: *"use `caps`"* → avoid it, take `rustix`.** Three grounds: `CapsHashSet = HashSet<Capability>` allocates in the child (D2's argument, confirmed); **every `caps` error path is `format!("…: {}", io::Error::last_os_error())` — allocation *and* an `errno` TLS read, on the failure path** (the decisive one, and only D1 spotted it); and `caps::set` is a per-set `capget`+`capset` read-modify-write (6 syscalls, non-atomic) where `rustix::thread::set_capabilities` is one `capset`. D5 §19.2's `[R]` HIGH — that we own the ordering explicitly — survives untouched (§5.1).
16. **D5 §19.1 / D2 §12.5: `seccompiler` vs `libseccomp` is no longer a genuine either/or → take `seccompiler`**, with two facts recorded: its repo was **archived 2026-09-23** (code moved to the live `rust-vmm/rust-vmm` monorepo — a move, not a death), and **it cannot express a multi-architecture OCI filter** (`TargetArch` is one value; there is no `seccomp_arch_add`). Phase 1 accepts single-arch; phase 3 must revisit (§5.2).
17. **D1 §11.1's rejection of `cgroups-rs` is right for a much smaller reason than the one available** → `zbus 5.8` is a **non-optional** dependency whose default features enable `async-io` and **`blocking` ("a thread pool for isolating blocking I/O")**, so `cgroups-rs` in the core crate puts an executor and a thread pool in the process that calls `clone3` — a direct structural violation of D2 §8.2. It also brings `nix ^0.30` *transitively* on top of its own `nix 0.25.0` (D1's "two versions of nix", now named), and three of `zbus`'s core crates (`zvariant`, `zbus_names`, `zvariant_derive`) were marked **unmaintained on 2026-10-16** (§5.4, §7.4).
18. **D3's `sha2` vs `ring`: take `sha2`** → **RUSTSEC-2025-0007 declares `ring` unmaintained, with no patched version.** Cost to record: `sha2 0.11`'s MSRV is **1.88**, higher than `clap`'s 1.85, so `sha2` — not `clap` — would set the project MSRV (§5.6, §7.3).
19. **D2 §12.6 defers `memfd` at "~40 lines" → take it if wanted; its only dependency is `rustix ^1.0`**, so the marginal graph cost is zero crates (§5.7).
20. **`rtnetlink` cannot go in the core crate**: `futures-channel`/`futures-util` and **`nix ^0.30`** are all non-optional. The sync path that does exist and should be named is **`netlink-sys` + `netlink-packet-route` without `rtnetlink`** — executor-free and `nix`-free (§5.5, handed to V5).
21. **`nftables` is not a netlink library**: `src/helper.rs:13` is `const NFT_EXECUTABLE: &str = "nft"` and it `Command::new`s the binary four times. Contrary to the project charter; hands V5 a different question than the one it was asked (§5.5).
22. **`nftnl` carries the only memory-corruption advisory in the set** (RUSTSEC-2025-0126, heap-buffer-overflow in `Batch::with_page_size`, patched ≥ 0.9.0) **and is FFI to C `libnftnl`**, so it collides with static musl exactly as `libseccomp` does (§5.5).
23. **D2 §12.6's `pathrs` note — *"the only copyleft licence in reach"* → MPL-2.0 OR LGPL-3.0-or-later**; MPL-2.0 is file-level weak copyleft and does not reach our code. Decline `pathrs` for D2's *other*, better reason (`openat2` + `ResolveFlags` is the kernel doing it), not for the licence (§5.7).
24. **V4, pin `tar >= 0.4.45` explicitly, not `"0.4"`**: RUSTSEC-2026-0067 (`unpack_in` chmods arbitrary directories through a symlink) and RUSTSEC-2026-0068 (tar-rs is the **outlier** in ignoring PAX size headers — a tar-smuggling primitive, and a content-addressing hazard for D3). Both filed 2026-03-19, six months before the research, and absent from the corpus (§7.3).
25. **Registry-client pins, all less than six weeks old:** `rustls >= 0.23.46`, `rustls-webpki >= 0.103.16`, `webpki-roots >= 1.0.10`, and `tokio >= 1.54.1` if tokio is used at all — **tokio 1.54.0 has been yanked** (RUSTSEC-2026-0384, use-after-free in `Interval` on the multi-thread runtime, filed 2026-10-21). Also RUSTSEC-2026-0386 (2026-10-13): `AsyncFd::new` violates I/O safety and is now deprecated in favour of `unsafe` constructors — a dated argument for D2 §8.3's sync `epoll` loop (§7.4).

**Framing shifts**

26. **D2 §8.3's "async is genuinely the right tool" for the registry pull → weaken it.** `ureq 3.5.0` has no `tokio`, no `futures` and no `mio` in its non-optional deps; sync HTTP + a `std::thread` pool buys the same parallelism and makes D2 §8.3's separate-process rule a nice-to-have rather than the enforcement mechanism for the project's biggest rewrite-risk decision. Keep the rule either way; D3 owns the choice (§6.2).
27. **D2 §8.2's `[F]` HIGH on tokio is now mechanical, and one precision matters:** `Builder::build_threaded_runtime` calls `launch()` *inside* `build()` → `spawn_blocking` → `thread::Builder::new().spawn`, so threads exist before `main`'s body. `build_current_thread_runtime` does **not** — but `tokio::fs::*` and DNS resolution route through `spawn_blocking`, so "never had a second thread" cannot be preserved by choosing a flavour. D2 §8.3's "structural beats disciplined" is confirmed, with a more ordinary failure mode than D2 imagines (§6.1).
28. **D2 §14 item 7 settled:** `nix` resumed on 2026-09-29 after a **19-week** stall — six commits by one new contributor, all lint/CI/docs, no release since 0.31.3 (2026-05-11). The maintainer's own capacity statement is **nix#1304, open since 2020-10-03, last updated 2026-09-24**: *"my responsiveness has been lagging… I've been burning out… Would somebody volunteer to help me with this crate?"* And **nix#2797** (open, 2026-05-18) is a third-party audit filing **CRITICAL** findings in `unistd` (`dup2_raw` wraps `-1` in an `OwnedFd`) and `sys::signal` (`SigSet::wait` uses the wrong `sigwait(3)` error convention) — two of the four modules D1 §11.1 wanted `nix` for (§7.6).
29. **D4, on `nix::fcntl::Flock`:** besides the panicking `Drop` (§1.6, and flagged upstream in **nix#2802**, which I wrongly said nobody had flagged), **`File::try_clone()` breaks the RAII wrapper** — `flock(2)` locks the open file description, so two `Flock`s over cloned descriptors are both "exclusive" and dropping either unlocks both. For state-directory locking that is a correctness bug: **one owner of the lock fd, never cloned** (§7.6).
30. **D2 §14 item 6 determined (not run):** the musl obstacle is C dependencies, and the recommended set has exactly one candidate (`libseccomp`, which §5.2 declines). `crt-static` is **on by default** for `x86_64-unknown-linux-musl` on stable (verified locally with `rustc --print cfg --target`), the target needs no nightly, and neither `rustix`'s nor `libc`'s `build.rs` compiles C. Youki's nightly + `-Zbuild-std` requirement is a property of *its* dependency set, not of musl. Residual risk: `zstd-sys` compiles vendored C — in the pull binary, not the core (§7.5).
31. **D2 §14 item 8 (rustix bus factor): accept the risk, and act on it in three ways** — keep §3.6's raw-call list current; confine rustix behind an internal `sys::` module that re-exports only our ~40 wrappers, so replacing it is one file; pin exactly and re-read `not_implemented.rs` on every bump (`close_range` and `mount_setattr` are still absent in 1.1.5). rustix 1.1.5 shipped 2026-09-16, after the research (§7.7).
32. **Housekeeping on this file:** its header says *"Date of verification: 2026-09-04"* and §1.4's "15.4 weeks" is anchored to that date. **Today is 2026-10-23.** Parts 1–2 are as-of 2026-09-04; Parts 3–7 are as-of today, and §7.6 supersedes §1.4's figures.

---

## Still unverifiable, and the exact test

One line each: *the claim → the host and the exact command that would settle it.*

1. **Static musl build of the core crate** (D2 §14 item 6) → **dev VM**, after `rustup target add x86_64-unknown-linux-musl`: `cargo build -p core --target x86_64-unknown-linux-musl --release && ldd target/x86_64-unknown-linux-musl/release/mydocker` — expect *"not a dynamic executable"*; and `file …` — expect *"statically linked"*. No Rust toolchain exists on `my-droplet` or `uc-arcc`, and the local toolchain is darwin/aarch64.
2. **No rustix function in the child subset allocates** (universal form of §2.6/§4.5; I read `path/arg.rs` exhaustively but not the bodies of all ~40 functions) → **dev VM or CI**: `cargo test -p core --test no_alloc_in_child`, the `#[global_allocator]` counting shim of §4.5, **with a `rootfs` path of ≥ 256 bytes** — a test built from `/tmp/x` passes whether or not the discipline is held.
3. **`rustix::thread::set_thread_res_uid`/`set_thread_groups` give the `execve`'d program the right identity** (D2 §14 item 9, empirical half — the source half is settled in §4.4) → **dev VM**, as root: a test binary that `clone3`s, calls `set_thread_groups(&[])` + `set_thread_res_gid(g,g,g)` + `set_thread_res_uid(u,u,u)`, then `execve`s `/bin/sh -c 'id -u; id -g; id -G'`, asserting the child reports the target ids. Cheap; do it first.
4. **`CLONE_INTO_CGROUP`'s truncation has the predicted behavioural effect** (§3.4 proves the constant is `0`; the consequence is inferred) → **dev VM**: `clone3` with `flags = CLONE_NEWPID | (libc::CLONE_INTO_CGROUP as u64)` and `args.cgroup = <fd>`, then `cat /proc/<child>/cgroup` — expect the **parent's** cgroup; repeat with our own `1u64 << 33` and expect the target cgroup.
5. **`clone3` rejects the exit signal in the flags word** (§3.3, read from `kernel/fork.c@v6.11`, not run) → **dev VM**: `clone3` with `flags |= SIGCHLD` and `exit_signal = 0` — expect `EINVAL`; then `flags` clean and `exit_signal = SIGCHLD` — expect success.
6. **`CLONE_INTO_CGROUP` requires `usize >= CLONE_ARGS_SIZE_VER2`** (§3.3) → same host: the same call with `size = 64` — expect `EINVAL`; with `size = size_of::<clone_args>()` (88) — expect success.
7. **Docker's default seccomp profile fits `seccompiler`'s 4096-instruction ceiling** (§5.2.3) → **any host with the toolchain**: `seccompiler::compile_from_json` over `/etc/docker/seccomp.json` (or the moby default profile), then assert `program.len() < 4096`. On `my-droplet` the profile can be *read* (`docker info`/`cat`), but compiling needs Rust.
8. **`oci-spec` + the key-diff auditor really costs less than hand-writing** (D2 §14 item 11; §5.3.2 answers the line-count half at 10,462 lines) → **dev VM**: write the ~30-line `serde_json::Value` auditor first, run it against the 9,907-byte nginx `config.json` from `my-droplet`, and count unhandled keys. Large count ⇒ the conversion layer is doing more than advertised.
9. **`cgroups-rs`/`zbus` actually spawns threads in a real build** (§5.4 argues from `blocking`'s and `async-io`'s own descriptions, not from a running process) → **dev VM**: a binary that links `cgroups-rs`, does one cgroup write, then reads `Threads:` from `/proc/self/status` — expect `> 1`.
10. **`tracing-subscriber` with `with_writer(Mutex::new(file))` deadlocks a post-`clone` child** (§6.3 infers it from `MutexGuardWriter`; the default-stdout case is settled) → **dev VM**: a parent with that subscriber, a second thread looping on log events, then `clone3` and one `tracing::info!` in the child — expect a hang or a poison panic.
11. **nix#2797's CRITICAL findings are still live in 0.31.3** (§7.6; I read the audit report in the issue, not the 0.31.3 source for those two functions) → **anywhere**: read `nix-0.31.3/src/unistd.rs:625-639` for a missing `Errno::result` before `OwnedFd::from_raw_fd`, and `src/sys/signal.rs:587-596` for `Errno::result` applied to `sigwait(3)`'s positive-errno return. Not load-bearing — we decline `nix` on §1.1 — but it would firm up §7.6.
12. **sunfishcode's share of recent rustix commits** (D2's 22-of-61; §2.7 and §7.7 both failed to re-derive it) → **nothing available from here**: GitHub's `/stats/contributors` returns `202` while it computes. A full `git clone` of `bytecodealliance/rustix` + `git shortlog -sn --since=…` would settle it; not worth the bandwidth for a claim nothing rests on.
13. **FIFO pipe capacity behind D2 §2.3's blocking claim** (carried forward, D2 §14 item 12) → still blocked: it needs `fcntl(fd, F_GETPIPE_SZ)` on a live shim's FIFO fd, and `my-droplet` is production. Record it for the dev VM's first `create`.
