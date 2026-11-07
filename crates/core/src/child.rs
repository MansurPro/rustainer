//! The forbidden window: everything that runs between `clone3` and `execve`.
//!
//! This module is small and separate so it can be audited as a unit. The rule
//! it exists to enforce, in the form that actually explains itself:
//!
//! > After a raw `clone`, the child inherits the parent's address space
//! > including every lock, and — unlike `fork(3)` — glibc has had no
//! > opportunity to quiesce or reset any of them. The rules below are not
//! > best practice; they are the only thing standing between us and an
//! > unreproducible hang. (RND.md §15.4.1)
//!
//! Concretely, **nothing reachable from [`child_main`] may**:
//!
//! - allocate — `format!`, `String`, `Vec` growth, `to_owned`, `Box::new`,
//!   `CString::new`, any collection insert. Rust's `System::alloc` is
//!   `libc::malloc`, the same arena and the same lock as the parent's.
//! - `println!` or `eprintln!` — both take a `ReentrantLock` that may already
//!   be held, and `Stdout` sits behind a `OnceLock` that may be mid-init.
//! - panic, unwrap an `Err`, index a slice out of bounds, or overflow in a
//!   debug build — the panic machinery formats and allocates.
//! - use `std::io::Error` — it allocates for custom errors. `rustix`'s
//!   `Errno(u16)` is a plain integer and does not.
//! - log through any framework. They allocate unconditionally.
//!
//! What it *may* do: raw syscalls, `rustix` calls taking pre-built `CString`
//! paths, and `write(2)` of a pre-built byte array to a pre-opened fd. That
//! last one is how the child reports anything at all.

use core::ffi::{CStr, c_char};
use std::ffi::{CString, OsStr, OsString};
use std::os::unix::ffi::OsStrExt;

use crate::mount::{self, PlannedOp};
use crate::sys::{self, Error as Errno, OwnedFd};

/// Which step failed, so the parent can say so without guessing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum Step {
    AwaitParent = 1,
    SetHostname = 2,
    Execve = 3,
    Mount = 4,
    PivotRoot = 5,
}

impl Step {
    fn from_u8(v: u8) -> Option<Self> {
        Some(match v {
            1 => Step::AwaitParent,
            2 => Step::SetHostname,
            3 => Step::Execve,
            4 => Step::Mount,
            5 => Step::PivotRoot,
            _ => return None,
        })
    }

    pub fn describe(self) -> &'static str {
        match self {
            Step::AwaitParent => "waiting for the parent's go-ahead",
            Step::SetHostname => "setting the hostname in the new UTS namespace",
            Step::Execve => "executing the container command",
            Step::Mount => "building the container's mount set",
            Step::PivotRoot => "pivoting onto the container's root filesystem",
        }
    }
}

/// The child's failure report: a fixed-size record, written with one
/// `write(2)` of a stack array. Nothing here allocates or formats.
///
/// `detail` is an index into the mount plan for [`Step::Mount`], and unused
/// otherwise — the child cannot build a message, so it sends a number the
/// parent looks up in the plan it still holds.
pub const RECORD_LEN: usize = 8;

fn encode(step: Step, detail: u8, errno: Errno) -> [u8; RECORD_LEN] {
    let e = errno.raw_os_error().to_ne_bytes();
    [step as u8, detail, 0, 0, e[0], e[1], e[2], e[3]]
}

pub fn decode(record: &[u8]) -> Option<(Step, u8, Errno)> {
    if record.len() != RECORD_LEN {
        return None;
    }
    let step = Step::from_u8(record[0])?;
    let errno = i32::from_ne_bytes([record[4], record[5], record[6], record[7]]);
    Some((step, record[1], Errno::from_raw_os_error(errno)))
}

/// Everything the child touches, pre-encoded parent-side.
///
/// **No `&str`, no `PathBuf`, no `String` may appear in this struct or in any
/// function reachable from [`child_main`].** `rustix`'s `path::Arg` uses a
/// stack buffer of `SMALL_PATH_BUFFER_SIZE = 256` bytes and falls into a
/// `CString::new` heap allocation at `bytes.len() >= 256`
/// (rustix 1.1.5, `src/path/mod.rs:9`) — in the one window where an
/// allocation can deadlock forever. A content-addressed store layout makes
/// 256-byte paths the norm rather than the exception, so this is not
/// theoretical. The 256 is written down here so nobody simplifies it away.
pub struct ChildPlan {
    /// Owns the bytes that `exe`, `argv` and `envp` point into. Never read
    /// directly; never mutated, because that would move the allocations.
    _storage: Vec<CString>,
    exe: *const c_char,
    argv: Vec<*const c_char>,
    envp: Vec<*const c_char>,
    hostname: Option<CString>,
    /// Empty when the container keeps the host's filesystem view.
    mount_ops: Vec<PlannedOp>,
    rootfs: Option<CString>,
    proceed_rx: OwnedFd,
    err_tx: OwnedFd,
}

#[derive(Debug, thiserror::Error)]
pub enum PlanError {
    #[error("{what} contains an interior NUL byte, which an execve argument cannot")]
    InteriorNul { what: &'static str },
}

impl ChildPlan {
    #[allow(clippy::too_many_arguments)]
    pub fn build(
        exe: &OsStr,
        argv: &[OsString],
        envp: &[OsString],
        hostname: Option<&OsStr>,
        mounts: Option<mount::MountPlan>,
        proceed_rx: OwnedFd,
        err_tx: OwnedFd,
    ) -> Result<Self, PlanError> {
        fn cstr(s: &OsStr, what: &'static str) -> Result<CString, PlanError> {
            CString::new(s.as_bytes()).map_err(|_| PlanError::InteriorNul { what })
        }

        // All of this allocation is parent-side and therefore fine. It is
        // also the *point*: by the time the child exists, there is nothing
        // left to build.
        let mut storage = Vec::with_capacity(1 + argv.len() + envp.len());
        storage.push(cstr(exe, "the command path")?);
        for a in argv {
            storage.push(cstr(a, "a command argument")?);
        }
        for e in envp {
            storage.push(cstr(e, "an environment entry")?);
        }

        let exe_ptr = storage[0].as_ptr();
        let mut argv_ptrs: Vec<*const c_char> =
            storage[1..=argv.len()].iter().map(|c| c.as_ptr()).collect();
        argv_ptrs.push(core::ptr::null());
        let mut envp_ptrs: Vec<*const c_char> = storage[1 + argv.len()..]
            .iter()
            .map(|c| c.as_ptr())
            .collect();
        envp_ptrs.push(core::ptr::null());

        let hostname = match hostname {
            Some(h) => Some(cstr(h, "the hostname")?),
            None => None,
        };

        let (mount_ops, rootfs) = match mounts {
            Some(p) => (p.ops, Some(p.rootfs)),
            None => (Vec::new(), None),
        };

        Ok(Self {
            _storage: storage,
            exe: exe_ptr,
            argv: argv_ptrs,
            envp: envp_ptrs,
            hostname,
            mount_ops,
            rootfs,
            proceed_rx,
            err_tx,
        })
    }

    pub fn exe(&self) -> &CStr {
        // SAFETY: `exe` points into `_storage[0]`, which is alive and never
        // mutated for as long as `self` is.
        unsafe { CStr::from_ptr(self.exe) }
    }
}

/// The child's entire life before `execve`.
///
/// # Safety
///
/// May only be called on the child branch of [`crate::sys::clone3`], in a
/// process that has not allocated since the clone.
pub unsafe fn child_main(plan: &ChildPlan) -> ! {
    // 1. Wait for the parent to finish the setup only it can do.
    //
    // This is where uid_map writing and cgroup placement go, and both must
    // happen before the child runs anything. Establishing the protocol now
    // is what keeps those from being a change to the process topology later.
    let mut byte = [0u8; 1];
    loop {
        match sys::read(&plan.proceed_rx, &mut byte) {
            Ok(1) => break,
            // EOF: the parent died between the clone and the go-ahead.
            Ok(_) => fail(plan, Step::AwaitParent, 0, Errno::PIPE),
            Err(Errno::INTR) => continue,
            Err(e) => fail(plan, Step::AwaitParent, 0, e),
        }
    }

    // 2. Build the mount set, then pivot onto it.
    //
    // Both belong here rather than in the parent. The mount namespace is
    // ours, and /proc in particular binds its pid namespace at mount time
    // from the mounting process — mounted by the supervisor, the container
    // would see host PIDs.
    if let Some(rootfs) = &plan.rootfs {
        if let Err((index, e)) = mount::apply(&plan.mount_ops) {
            fail(plan, Step::Mount, index, e);
        }
        if let Err(e) = mount::pivot(rootfs) {
            fail(plan, Step::PivotRoot, 0, e);
        }
    }

    // 3. Name ourselves. This is the child's job, not the parent's: the new
    //    UTS namespace is ours and the parent is not in it.
    if let Some(h) = &plan.hostname
        && let Err(e) = sys::sethostname(h.to_bytes())
    {
        fail(plan, Step::SetHostname, 0, e);
    }

    // 4. Become the container command. `err_tx` is O_CLOEXEC, so a successful
    //    execve closes it and the parent sees EOF — that, and not a success
    //    message, is how the parent learns this worked.
    let e = unsafe { sys::execve(plan.exe, plan.argv.as_ptr(), plan.envp.as_ptr()) };
    fail(plan, Step::Execve, 0, e)
}

/// Report and leave. One `write(2)` of a stack array, then `_exit`.
fn fail(plan: &ChildPlan, step: Step, detail: u8, errno: Errno) -> ! {
    let record = encode(step, detail, errno);
    // A partial write cannot be retried meaningfully here and a failed write
    // leaves us nothing to say, so the result is deliberately discarded: the
    // parent falls back to reporting the exit status.
    let _ = sys::write(&plan.err_tx, &record);
    // 127 is the shell's convention for "command not found", and the closest
    // thing to a conventional meaning for "the child never started".
    sys::exit_immediately(127)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plan_for(exe: &str, args: &[&str]) -> ChildPlan {
        let (rx, tx) = sys::pipe_with(sys::PipeFlags::CLOEXEC).unwrap();
        ChildPlan::build(
            OsStr::new(exe),
            &args.iter().map(OsString::from).collect::<Vec<_>>(),
            &[OsString::from("PATH=/usr/bin:/bin")],
            Some(OsStr::new("box")),
            None,
            rx,
            tx,
        )
        .unwrap()
    }

    #[test]
    fn argv_and_envp_are_null_terminated() {
        let plan = plan_for("/bin/sh", &["sh", "-c", "true"]);
        assert_eq!(plan.argv.len(), 4, "3 args plus the NULL");
        assert!(plan.argv.last().unwrap().is_null());
        assert_eq!(plan.envp.len(), 2, "1 entry plus the NULL");
        assert!(plan.envp.last().unwrap().is_null());
        assert_eq!(plan.exe().to_bytes(), b"/bin/sh");
    }

    /// The pointers must survive the plan being moved, because it is built in
    /// one function and used in another.
    #[test]
    fn pointers_survive_a_move() {
        let plan = plan_for("/bin/sh", &["sh"]);
        let boxed = Box::new(plan);
        assert_eq!(boxed.exe().to_bytes(), b"/bin/sh");
        // SAFETY: argv[0] points into storage that moved with the Box.
        let arg0 = unsafe { CStr::from_ptr(boxed.argv[0]) };
        assert_eq!(arg0.to_bytes(), b"sh");
    }

    #[test]
    fn interior_nul_is_rejected_not_truncated() {
        let (rx, tx) = sys::pipe_with(sys::PipeFlags::CLOEXEC).unwrap();
        let result = ChildPlan::build(OsStr::new("/bin/sh\0extra"), &[], &[], None, None, rx, tx);
        assert!(matches!(result, Err(PlanError::InteriorNul { .. })));
    }

    #[test]
    fn failure_record_round_trips() {
        for step in [
            Step::AwaitParent,
            Step::SetHostname,
            Step::Execve,
            Step::Mount,
            Step::PivotRoot,
        ] {
            let encoded = encode(step, 7, Errno::NOENT);
            let (got_step, detail, got_errno) = decode(&encoded).expect("decode");
            assert_eq!(got_step, step);
            assert_eq!(
                detail, 7,
                "the mount-plan index must survive the round trip"
            );
            assert_eq!(got_errno, Errno::NOENT);
        }
        assert!(decode(&[9, 0, 0, 0, 0, 0, 0, 0]).is_none(), "unknown step");
        assert!(decode(&[1, 2, 3]).is_none(), "short record");
    }

    #[test]
    fn every_step_describes_itself() {
        for step in [
            Step::AwaitParent,
            Step::SetHostname,
            Step::Execve,
            Step::Mount,
            Step::PivotRoot,
        ] {
            assert!(!step.describe().is_empty());
        }
    }
}
