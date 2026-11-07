//! The one module in this crate that names a syscall.
//!
//! Everything else goes through here, so the whole `rustix` dependency is
//! replaceable by editing one file (RND.md §15.1.5). The hand-written `libc`
//! surface is deliberately tiny and listed in full below; if it grows, the
//! list grows with it in the same commit.
//!
//! # The hand-written surface
//!
//! | Call | Why `rustix` declines it |
//! |---|---|
//! | `clone3` | `not_implemented::libc_internals` |
//! | `execve` | `not_implemented::quite_yet` |
//! | `_exit` | `not_implemented::quite_yet` |
//!
//! Everything else in the create path — `sethostname`, `pipe2`, `read`,
//! `write`, `waitpid` — `rustix` provides, and `rustix`'s versions issue the
//! raw syscall rather than calling through glibc's wrappers. That matters
//! more than it looks: glibc routes the whole `set*id` family through
//! `__nptl_setxid`, which takes a lock and futex-waits for threads that do
//! not exist in a `clone3` child, hanging it forever (RND.md §15.1.1).

use core::ffi::c_char;

use rustix::io::Errno;

// ---------------------------------------------------------------------------
// Kernel constants
// ---------------------------------------------------------------------------

// linux/sched.h. These are defined here rather than imported, because a flags
// constant that silently truncates is the worst available failure shape: it
// type-checks, compiles, runs, returns success, and does the wrong thing with
// no diagnostic anywhere.
//
// Specifically, do NOT use `libc::CLONE_INTO_CGROUP`. It is typed `c_int` on
// every glibc target, its value is 2^33, and the deny-by-default
// `overflowing_literals` lint is suppressed in place with `#[allow]` - so it
// compiles and evaluates to **0**. `clone3` then succeeds, returns a pid, and
// the child lands in the *parent's* cgroup with every limit silently
// inapplicable. It does not exist at all on musl. (libc 0.2.191, issue #3584,
// fixed only on `main` in e9abac9ac2. RND.md §15.3.)
// Not every one of these is used yet. They are defined together anyway,
// because the whole point is that the right constant is already here when a
// later stage reaches for it - the failure this guards against is someone
// typing `libc::CLONE_INTO_CGROUP` at stage 4 and getting a silent zero.
#[allow(dead_code)]
pub const CLONE_NEWNS: u64 = 0x0002_0000;
#[allow(dead_code)]
pub const CLONE_NEWCGROUP: u64 = 0x0200_0000;
#[allow(dead_code)]
pub const CLONE_NEWUTS: u64 = 0x0400_0000;
#[allow(dead_code)]
pub const CLONE_NEWIPC: u64 = 0x0800_0000;
#[allow(dead_code)]
pub const CLONE_NEWUSER: u64 = 0x1000_0000;
#[allow(dead_code)]
pub const CLONE_NEWPID: u64 = 0x2000_0000;
#[allow(dead_code)]
pub const CLONE_NEWNET: u64 = 0x4000_0000;
#[allow(dead_code)]
pub const CLONE_PIDFD: u64 = 0x0000_1000;
#[allow(dead_code)]
pub const CLONE_CLEAR_SIGHAND: u64 = 1 << 32;
#[allow(dead_code)]
pub const CLONE_INTO_CGROUP: u64 = 1 << 33;

/// `clone3` reports the kernel's `struct clone_args` size back to us at
/// compile time. The kernel asserts `sizeof(struct clone_args) ==
/// CLONE_ARGS_SIZE_VER2` (`linux@v6.11/kernel/fork.c:2937`); if `libc` ever
/// disagrees, the size argument we pass is wrong and this build should fail
/// rather than hand the kernel a struct it will misread.
const _: () = assert!(core::mem::size_of::<libc::clone_args>() == 88);

// ---------------------------------------------------------------------------
// clone3
// ---------------------------------------------------------------------------

/// What `clone3` returned, from the point of view of whoever is asking.
///
/// This is an enum rather than a `pid_t` because the raw call returns *in two
/// processes* and a bare integer makes that too easy to forget.
pub enum Cloned {
    /// We are the parent. The child's pid in our pid namespace.
    Parent(libc::pid_t),
    /// We are the child. Nothing in this process may allocate, log, take a
    /// lock, or panic until `execve` — see [`crate::child`].
    Child,
}

/// Create a child process with `clone3`.
///
/// # Safety
///
/// The caller must have verified that this process is single-threaded
/// ([`thread_count`]) and must have flushed stdio. On the child branch the
/// caller must reach `execve` or `_exit` without allocating.
pub unsafe fn clone3(args: &libc::clone_args) -> Result<Cloned, Errno> {
    // The exit signal goes in `args.exit_signal`, never ORed into the flags
    // word - that is `clone(2)`'s legacy convention and `clone3` rejects it.
    let rc = unsafe {
        libc::syscall(
            libc::SYS_clone3,
            args as *const libc::clone_args,
            core::mem::size_of::<libc::clone_args>(),
        )
    };
    match rc {
        0 => Ok(Cloned::Child),
        n if n > 0 => Ok(Cloned::Parent(n as libc::pid_t)),
        _ => Err(last_errno()),
    }
}

/// Build a zeroed `clone_args`.
///
/// Zero-initialised, never `MaybeUninit::uninit()`. The kernel's
/// size-versioning rule (`copy_struct_from_user`) checks the tail of a
/// larger-than-known struct for zero and returns `-E2BIG` otherwise — and on
/// a 5.5-vintage kernel a partially filled struct is *accepted and
/// interpreted*, which is worse (RND.md §15.2).
pub fn clone_args_zeroed() -> libc::clone_args {
    // SAFETY: `clone_args` is a `#[repr(C)]` struct of eleven integers, for
    // which the all-zero bit pattern is valid and is what the kernel expects
    // for "field not supplied".
    unsafe { core::mem::zeroed() }
}

// ---------------------------------------------------------------------------
// The two calls that end the child
// ---------------------------------------------------------------------------

/// Replace this process image.
///
/// Returns only on failure, and then only the errno — building an
/// `io::Error` here would allocate.
///
/// # Safety
///
/// `path`, `argv` and `envp` must be valid, NUL-terminated, and `argv`/`envp`
/// must themselves be NULL-terminated arrays that outlive the call.
pub unsafe fn execve(
    path: *const c_char,
    argv: *const *const c_char,
    envp: *const *const c_char,
) -> Errno {
    unsafe { libc::execve(path, argv, envp) };
    last_errno()
}

/// Leave immediately, running no destructors, no atexit handlers and no
/// buffered-stream flush.
///
/// `std::process::exit` is wrong here: it runs libc's exit handlers, which
/// would flush stdio buffers the child inherited from the parent and emit
/// their contents a second time.
pub fn exit_immediately(code: i32) -> ! {
    unsafe { libc::_exit(code) }
}

fn last_errno() -> Errno {
    Errno::from_raw_os_error(unsafe { *libc::__errno_location() })
}

// ---------------------------------------------------------------------------
// The single-thread invariant
// ---------------------------------------------------------------------------

/// How many threads this process has.
///
/// This is the detector for the invariant the whole design rests on: **the
/// process that calls `clone3` must never have had a second thread.** A raw
/// `clone` is more dangerous than `fork()`, not less — `fork(3)` runs
/// `__malloc_fork_lock_parent`, `fresetlockfiles`, `_IO_list_resetlock` and
/// `reclaim_stacks` to make the child survivable, and a raw `clone3` runs
/// none of it. The child inherits every lock in whatever state it was in
/// (RND.md §15.4.1).
///
/// Field 20 of `/proc/self/stat`, counting from the field after `comm`.
/// Split after the **last** `)`, because a process can be named `foo) 0 R`.
pub fn thread_count() -> Result<u64, Errno> {
    let stat = read_small(c"/proc/self/stat")?;
    let stat = core::str::from_utf8(&stat).map_err(|_| Errno::INVAL)?;
    parse_num_threads(stat).ok_or(Errno::INVAL)
}

/// Field 20 of a `/proc/<pid>/stat` line, counting from the field after
/// `comm`.
///
/// Split after the **last** `)`, never the first: `comm` is the executable
/// name, it is attacker-influenced, and a process named `foo) 0 R 1` shifts
/// every field after it if you split on the first one.
fn parse_num_threads(stat: &str) -> Option<u64> {
    let tail = stat.rsplit_once(')')?.1;
    // After `)`: state, ppid, pgrp, session, tty_nr, tpgid, flags, minflt,
    // cminflt, majflt, cmajflt, utime, stime, cutime, cstime, priority,
    // nice, num_threads. That is index 17 zero-based.
    tail.split_ascii_whitespace().nth(17)?.parse().ok()
}

fn read_small(path: &core::ffi::CStr) -> Result<Vec<u8>, Errno> {
    use rustix::fs::{Mode, OFlags, open};
    let fd = open(path, OFlags::RDONLY | OFlags::CLOEXEC, Mode::empty())?;
    let mut buf = vec![0u8; 1024];
    let n = rustix::io::read(&fd, &mut buf)?;
    buf.truncate(n);
    Ok(buf)
}

// ---------------------------------------------------------------------------
// Thin re-exports, so no other module imports `rustix` directly
// ---------------------------------------------------------------------------

pub use rustix::io::{Errno as Error, read, write};
pub use rustix::pipe::{PipeFlags, pipe_with};
pub use rustix::process::{Pid, WaitOptions, waitpid};
pub use rustix::system::sethostname;
pub use std::os::fd::OwnedFd;

#[cfg(test)]
mod tests {
    use super::*;

    /// The hazard this parser exists for, tested deterministically: a
    /// process can be named so that splitting on the *first* `)` shifts
    /// every later field.
    #[test]
    fn num_threads_survives_a_hostile_comm() {
        // A normal line: num_threads is 4.
        let normal = "42 (bash) S 1 42 42 0 -1 4194304 500 0 0 0 1 2 3 4 20 0 4 0 900 0 0";
        assert_eq!(parse_num_threads(normal), Some(4));

        // Same line, but the executable is named `evil) 0 R 1`. Splitting on
        // the first `)` would read a field from the comm itself.
        let hostile = "42 (evil) 0 R 1) S 1 42 42 0 -1 4194304 500 0 0 0 1 2 3 4 20 0 7 0 900 0 0";
        assert_eq!(
            parse_num_threads(hostile),
            Some(7),
            "split on the first ')' instead of the last"
        );

        assert_eq!(parse_num_threads("nonsense"), None);
        assert_eq!(parse_num_threads("42 (bash) S 1"), None);
    }

    /// And the live reading has to agree with the kernel's own answer.
    ///
    /// Deliberately not written as "spawn a thread, assert the count rose by
    /// one": `cargo test` runs tests on a thread pool, so the ambient count
    /// moves under you and that assertion fails on a loaded machine. Compare
    /// against `/proc/self/task` instead, retrying until two reads agree -
    /// a parser reading the wrong field never agrees.
    #[test]
    fn thread_count_agrees_with_proc_self_task() {
        for _ in 0..50 {
            let n = thread_count().expect("read /proc/self/stat");
            let tasks = std::fs::read_dir("/proc/self/task")
                .expect("read /proc/self/task")
                .count() as u64;
            if n == tasks {
                assert!(n >= 1);
                return;
            }
        }
        panic!("thread_count never agreed with /proc/self/task");
    }

    /// The constants are load-bearing and silently wrong is the failure mode
    /// we are defending against, so assert the values rather than trusting
    /// that they were transcribed correctly.
    #[test]
    fn clone_constants_match_linux_sched_h() {
        assert_eq!(CLONE_NEWNS, 0x0002_0000);
        assert_eq!(CLONE_PIDFD, 0x0000_1000);
        assert_eq!(CLONE_NEWCGROUP, 0x0200_0000);
        assert_eq!(CLONE_NEWUTS, 0x0400_0000);
        assert_eq!(CLONE_NEWIPC, 0x0800_0000);
        assert_eq!(CLONE_NEWUSER, 0x1000_0000);
        assert_eq!(CLONE_NEWPID, 0x2000_0000);
        assert_eq!(CLONE_NEWNET, 0x4000_0000);
        // The two that do not fit in a `c_int`, which is the whole point.
        assert_eq!(CLONE_CLEAR_SIGHAND, 0x1_0000_0000);
        assert_eq!(CLONE_INTO_CGROUP, 0x2_0000_0000);
    }
}
