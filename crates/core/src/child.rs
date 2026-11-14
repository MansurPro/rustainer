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
use std::os::fd::AsRawFd;
use std::os::unix::ffi::OsStrExt;

use crate::mount::{self, PlannedOp};
use crate::security::{self, Security};
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
    Stdio = 6,
    AwaitStart = 7,
    CgroupNamespace = 8,
    Security = 9,
    WorkingDirectory = 10,
    Loopback = 11,
    Interface = 12,
    JoinNamespace = 13,
}

impl Step {
    fn from_u8(v: u8) -> Option<Self> {
        Some(match v {
            1 => Step::AwaitParent,
            2 => Step::SetHostname,
            3 => Step::Execve,
            4 => Step::Mount,
            5 => Step::PivotRoot,
            6 => Step::Stdio,
            7 => Step::AwaitStart,
            8 => Step::CgroupNamespace,
            9 => Step::Security,
            10 => Step::WorkingDirectory,
            11 => Step::Loopback,
            12 => Step::Interface,
            13 => Step::JoinNamespace,
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
            Step::Stdio => "attaching the container's standard streams",
            Step::AwaitStart => "waiting at the start rendezvous",
            Step::CgroupNamespace => "unsharing the cgroup namespace",
            Step::Security => "applying the security posture",
            Step::WorkingDirectory => "entering the image's working directory",
            Step::Loopback => "bringing up the loopback interface",
            Step::Interface => "configuring the container's network interface",
            Step::JoinNamespace => "joining an existing container's namespaces",
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

/// Room for the kernel's own explanation, appended after the record.
pub const DETAIL_LEN: usize = crate::mount::DETAIL_LEN;

fn encode(step: Step, detail: u8, errno: Errno) -> [u8; RECORD_LEN] {
    let e = errno.raw_os_error().to_ne_bytes();
    [step as u8, detail, 0, 0, e[0], e[1], e[2], e[3]]
}

pub fn decode(record: &[u8]) -> Option<(Step, u8, Errno)> {
    if record.len() != RECORD_LEN {
        return None;
    }
    let step = Step::from_u8(record[0])?;
    let raw = i32::from_ne_bytes([record[4], record[5], record[6], record[7]]);
    // rustix's `from_raw_os_error` panics on a value that is not a real
    // errno, and this record arrives over a pipe. A corrupt or truncated
    // one must not take the supervisor down with it.
    let errno = if (1..=4095).contains(&raw) {
        Errno::from_raw_os_error(raw)
    } else {
        Errno::INVAL
    };
    Some((step, record[1], errno))
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
    /// The image's `WorkingDir`, entered after the pivot.
    workdir: Option<CString>,
    /// Namespaces to **join**, in order, before anything else (`exec`).
    ///
    /// Empty for a container being created. For `exec` this is where the
    /// mount namespace is entered, and it is entered by the child rather
    /// than the parent on purpose: `setns(CLONE_NEWNS)` repoints the
    /// caller's root and cwd at the container, which would take
    /// `/proc/self/stat` away from the parent - and that file is how
    /// [`crate::spawn`] checks the single-thread invariant. The parent
    /// would be checking an invariant it could no longer read.
    join: Vec<OwnedFd>,
    /// Unshare the cgroup namespace once placement has happened.
    cgroup_ns: bool,
    /// Bring `lo` up, which a new network namespace does not do for us.
    loopback: bool,
    /// Pre-built rtnetlink messages, with the offset of the interface
    /// index each one needs patched into it.
    net_messages: Vec<crate::net::Message>,
    /// Empty when the container keeps the host's filesystem view.
    mount_ops: Vec<PlannedOp>,
    rootfs: Option<CString>,
    /// Capabilities, user and seccomp. Applied last, because every step of
    /// it gives something away.
    security: Option<Security>,
    /// Descriptors to put on 0, 1 and 2 immediately before `execve`.
    stdio: Option<Stdio>,
    /// Those descriptors are a pty slave, and must become this process's
    /// controlling terminal.
    tty: bool,
    /// The start rendezvous.
    ///
    /// The `OwnedFd` is an `O_PATH` reference held only to keep its number
    /// valid; the `CString` is `/proc/self/fd/<n>`, built parent-side,
    /// because after `pivot_root` the FIFO has no pathname the container can
    /// reach and the child cannot build one anyway.
    exec_fifo: Option<(OwnedFd, CString)>,
    proceed_rx: OwnedFd,
    err_tx: OwnedFd,
}

/// What the container's standard streams are attached to.
pub struct Stdio {
    pub stdin: OwnedFd,
    pub stdout: OwnedFd,
    pub stderr: OwnedFd,
}

#[derive(Debug, thiserror::Error)]
pub enum PlanError {
    #[error("{what} contains an interior NUL byte, which an execve argument cannot")]
    InteriorNul { what: &'static str },
}

/// Everything `ChildPlan::build` needs, by name.
///
/// A struct rather than a positional argument list: this grew to fourteen
/// parameters as the ladder went on, and every new one silently shifted
/// the meaning of the ones after it at every call site.
#[derive(Default)]
pub struct ChildSpec<'a> {
    pub exe: &'a OsStr,
    pub argv: &'a [OsString],
    pub envp: &'a [OsString],
    pub hostname: Option<&'a OsStr>,
    pub workdir: Option<&'a OsStr>,
    pub mounts: Option<mount::MountPlan>,
    pub join: Vec<OwnedFd>,
    pub cgroup_ns: bool,
    pub loopback: bool,
    pub net_messages: &'a [crate::net::Message],
    pub security: Option<Security>,
    pub stdio: Option<Stdio>,
    pub tty: bool,
    pub exec_fifo: Option<OwnedFd>,
}

impl<'a> ChildSpec<'a> {
    pub fn new(exe: &'a OsStr) -> Self {
        Self {
            exe,
            ..Default::default()
        }
    }
}

impl ChildPlan {
    pub fn build(
        spec: ChildSpec<'_>,
        proceed_rx: OwnedFd,
        err_tx: OwnedFd,
    ) -> Result<Self, PlanError> {
        let ChildSpec {
            exe,
            argv,
            envp,
            hostname,
            workdir,
            mounts,
            join,
            cgroup_ns,
            loopback,
            net_messages,
            security,
            stdio,
            tty,
            exec_fifo,
        } = spec;
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
        let workdir = match workdir {
            Some(w) => Some(cstr(w, "the working directory")?),
            None => None,
        };

        let (mount_ops, rootfs) = match mounts {
            Some(p) => (p.ops, Some(p.rootfs)),
            None => (Vec::new(), None),
        };

        let exec_fifo = match exec_fifo {
            Some(fd) => {
                let path =
                    CString::new(format!("/proc/self/fd/{}", fd.as_raw_fd())).map_err(|_| {
                        PlanError::InteriorNul {
                            what: "the fifo path",
                        }
                    })?;
                Some((fd, path))
            }
            None => None,
        };

        Ok(Self {
            _storage: storage,
            exe: exe_ptr,
            argv: argv_ptrs,
            envp: envp_ptrs,
            hostname,
            workdir,
            join,
            cgroup_ns,
            loopback,
            net_messages: net_messages.to_vec(),
            security,
            mount_ops,
            rootfs,
            stdio,
            tty,
            exec_fifo,
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

    // 1a. Join an existing container's namespaces (`exec` only).
    //
    // Before everything, because every step below is supposed to happen
    // inside the container rather than beside it. The mount namespace goes
    // last within this list: `setns(CLONE_NEWNS)` calls `set_fs_root` and
    // `set_fs_pwd`, so the moment it succeeds this process's root and cwd
    // are the container's - and the host paths it might still have wanted
    // are gone. The kernel doing that for us is also why there is no
    // lingering host-directory reference to close here.
    for (i, fd) in plan.join.iter().enumerate() {
        if let Err(e) = sys::setns(fd, 0) {
            fail(plan, Step::JoinNamespace, i as u8, e);
        }
    }

    // 2. Take our own cgroup namespace, now that we are already placed.
    //
    // This cannot be a clone3 flag: `copy_cgroup_ns()` roots the new
    // namespace at the PARENT's cgroup, so the container would see the
    // supervisor's path instead of `/`. CLONE_INTO_CGROUP has already put
    // us where we belong, so unsharing here anchors the view correctly.
    if plan.cgroup_ns
        && let Err(e) = sys::unshare_cgroup_namespace()
    {
        fail(plan, Step::CgroupNamespace, 0, e);
    }

    // 3. Bring up loopback, before anything can want it.
    //
    // A fresh network namespace has `lo` DOWN and no 127.0.0.1 at all -
    // the kernel installs the address in response to the device coming up,
    // not at creation. Software that binds a local listener for its own
    // IPC fails in confusing ways without it.
    if plan.loopback
        && let Err(e) = sys::bring_up_loopback()
    {
        fail(plan, Step::Loopback, 0, e);
    }

    // 4. Configure our own interface, if one was put here for us.
    //
    // The messages were built in the parent: the child cannot allocate, so
    // it opens one netlink socket and writes bytes that are already
    // correct. The peer was placed directly into this namespace by the
    // same RTM_NEWLINK that created it, so there was never a moment when
    // the container's interface existed on the host.
    if !plan.net_messages.is_empty() {
        // Resolved here, not in the parent: "eth0 is index 2" is only true
        // in a namespace with nothing else in it, and a kernel with the
        // tunnel modules loaded puts sit0, tunl0 and gre0 there too.
        let Some(index) = crate::net::index_of(c"eth0") else {
            fail(plan, Step::Interface, 0, Errno::NODEV);
        };
        match crate::net::Netlink::open() {
            Err(e) => fail(plan, Step::Interface, 0, e),
            Ok(nl) => {
                // A stack buffer, because patching the index means writing,
                // and the plan is shared read-only with the parent's copy.
                let mut buf = [0u8; 256];
                for (i, (m, at)) in plan.net_messages.iter().enumerate() {
                    if m.len() > buf.len() {
                        fail(plan, Step::Interface, i as u8, Errno::MSGSIZE);
                    }
                    buf[..m.len()].copy_from_slice(m);
                    buf[*at..*at + 4].copy_from_slice(&index.to_ne_bytes());
                    if let Err(e) = nl.request(&buf[..m.len()]) {
                        fail(plan, Step::Interface, i as u8, e);
                    }
                }
            }
        }
    }

    // 5. Build the mount set, then pivot onto it.
    //
    // Both belong here rather than in the parent. The mount namespace is
    // ours, and /proc in particular binds its pid namespace at mount time
    // from the mounting process — mounted by the supervisor, the container
    // would see host PIDs.
    if let Some(rootfs) = &plan.rootfs {
        let mut detail = [0u8; mount::DETAIL_LEN];
        if let Err((index, e, n)) = mount::apply(&plan.mount_ops, &mut detail) {
            fail_with(plan, Step::Mount, index, e, &detail[..n]);
        }
        if let Err(e) = mount::pivot(rootfs) {
            fail(plan, Step::PivotRoot, 0, e);
        }
    }

    // 6. Name ourselves. This is the child's job, not the parent's: the new
    //    UTS namespace is ours and the parent is not in it.
    if let Some(h) = &plan.hostname
        && let Err(e) = sys::sethostname(h.to_bytes())
    {
        fail(plan, Step::SetHostname, 0, e);
    }

    // 11. Attach the standard streams. After this point the child has no
    //    way to report anything except through `err_tx`, which is why the
    //    failure record exists at all.
    if let Some(io) = &plan.stdio {
        for (fd, target) in [(&io.stdin, 0), (&io.stdout, 1), (&io.stderr, 2)] {
            if let Err(e) = sys::dup_onto(fd, target) {
                fail(plan, Step::Stdio, target as u8, e);
            }
        }
    }

    // 11a. Take the pty as our controlling terminal.
    //
    // Two syscalls, both required, neither obvious. `setsid` leaves the
    // caller's session - without it a Ctrl-C typed at the outer shell
    // reaches both - and `TIOCSCTTY` attaches this terminal, without which
    // the pty has no foreground process group and the kernel never turns
    // `^C` into SIGINT at all.
    if plan.tty
        && let Err(e) = unsafe { crate::pty::make_controlling() }
    {
        fail(plan, Step::Stdio, 3, e);
    }

    // 7. Enter the image's working directory, while we can still see it.
    //
    // After the pivot and before the privilege drop: a WorkingDir that only
    // root can enter is common in real images.
    if let Some(w) = &plan.workdir
        && let Err(e) = sys::chdir(w)
    {
        fail(plan, Step::WorkingDirectory, 0, e);
    }

    // 8. Open the start rendezvous - but do not wait on it yet.
    //
    // The open must happen while we are still root. After the uid change
    // the process is non-dumpable, which makes `/proc/<pid>/fd` root-owned
    // and mode 0500, so re-opening an `O_PATH` descriptor through it fails
    // EACCES for the container's user. The symptom is every non-root
    // container hanging at startup with nothing in the logs.
    //
    // `open(O_WRONLY)` on a FIFO blocks until a reader appears, so this is
    // where a created-but-not-started container waits - holding only our
    // init's credentials, having exec'd nothing.
    let mut started: Option<OwnedFd> = None;
    if let Some((_fd, proc_path)) = &plan.exec_fifo {
        match sys::open_write_blocking(proc_path) {
            Ok(w) => started = Some(w),
            Err(e) => fail(plan, Step::AwaitStart, 0, e),
        }
    }

    // 9. Give away privilege: bounding set, user, no_new_privs, seccomp.
    //
    // As close to execve as possible, so as few of our own syscalls as
    // possible run under the filter. Everything above this line needed
    // privilege; nothing below it does.
    if let Some(sec) = &plan.security
        && let Err((step, e)) = unsafe { security::apply(sec) }
    {
        fail(plan, Step::Security, step as u8, e);
    }

    // 10. Announce that we are started, now that the posture is in place.
    if let Some(w) = &started {
        let _ = sys::write(w, &[0u8]);
    }

    // 12. Become the container command. `err_tx` is O_CLOEXEC, so a successful
    //    execve closes it and the parent sees EOF — that, and not a success
    //    message, is how the parent learns this worked.
    let e = unsafe { sys::execve(plan.exe, plan.argv.as_ptr(), plan.envp.as_ptr()) };
    fail(plan, Step::Execve, 0, e)
}

/// Report and leave. One `write(2)` of a stack array, then `_exit`.
fn fail(plan: &ChildPlan, step: Step, detail: u8, errno: Errno) -> ! {
    fail_with(plan, step, detail, errno, &[])
}

/// As [`fail`], with the kernel's own words appended.
///
/// `extra` is written straight after the fixed record, so the parent reads
/// eight bytes of structure and then whatever explanation exists. Nothing
/// here formats or allocates: the bytes came from a `read(2)` into a stack
/// buffer.
fn fail_with(plan: &ChildPlan, step: Step, detail: u8, errno: Errno, extra: &[u8]) -> ! {
    let record = encode(step, detail, errno);
    // A partial write cannot be retried meaningfully here and a failed write
    // leaves us nothing to say, so the result is deliberately discarded: the
    // parent falls back to reporting the exit status.
    let _ = sys::write(&plan.err_tx, &record);
    if !extra.is_empty() {
        let _ = sys::write(&plan.err_tx, extra);
    }
    // 127 is the shell's convention for "command not found", and the closest
    // thing to a conventional meaning for "the child never started".
    sys::exit_immediately(127)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plan_for(exe: &str, args: &[&str]) -> ChildPlan {
        let (rx, tx) = sys::pipe_with(sys::PipeFlags::CLOEXEC).unwrap();
        let argv: Vec<OsString> = args.iter().map(OsString::from).collect();
        let envp = [OsString::from("PATH=/usr/bin:/bin")];
        ChildPlan::build(
            ChildSpec {
                argv: &argv,
                envp: &envp,
                hostname: Some(OsStr::new("box")),
                ..ChildSpec::new(OsStr::new(exe))
            },
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
        let result = ChildPlan::build(ChildSpec::new(OsStr::new("/bin/sh\0extra")), rx, tx);
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
            Step::Stdio,
            Step::AwaitStart,
            Step::CgroupNamespace,
            Step::Security,
            Step::WorkingDirectory,
            Step::Loopback,
            Step::Interface,
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
        assert!(
            decode(&[200, 0, 0, 0, 0, 0, 0, 0]).is_none(),
            "unknown step"
        );
        // A corrupt errno must not panic: rustix refuses to build an Errno
        // from a value that is not one.
        assert_eq!(
            decode(&[1, 0, 0, 0, 0, 0, 0, 0]).map(|(_, _, e)| e),
            Some(Errno::INVAL)
        );
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
            Step::Stdio,
            Step::AwaitStart,
            Step::CgroupNamespace,
            Step::Security,
            Step::WorkingDirectory,
            Step::Loopback,
            Step::Interface,
        ] {
            assert!(!step.describe().is_empty());
        }
    }
}
