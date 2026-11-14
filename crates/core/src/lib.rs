//! Container runtime core.
//!
//! Stages 1 and 2 of the ladder in `RND.md` §17: a process in namespaces of
//! its own, with its own root filesystem. No images, no layers, no limits, no
//! security controls, no networking.
//!
//! The ordering is deliberate. Everything later — cgroup placement, uid
//! mapping, seccomp — happens inside the window these two stages define, so
//! getting the window right first means writing it once.
//!
//! # What this crate may never acquire
//!
//! No async runtime, no thread pool, no lazily-initialised global that might
//! spawn one. Threads cannot be un-spawned, several namespace operations
//! require a single-threaded caller, and retrofitting sync into an async
//! codebase is a rewrite of every call path rather than a refactor
//! (RND.md §18.3). This is checked at runtime by [`sys::thread_count`]
//! immediately before every `clone3`, because a dependency can introduce a
//! thread pool transitively and a rule nobody can enforce is not a rule.

// A dependency constant that is silently zero is the worst available failure
// shape. `#[deprecated]` is the one signal libc gives us that a constant has
// gone wrong (RND.md §18.9), so a warning nobody reads becomes a build
// failure.
#![deny(deprecated)]

pub mod cgroup;
pub mod child;
pub mod exec;
pub mod firewall;
pub mod mount;
pub mod net;
pub mod proxy;
pub mod security;
pub mod state;
pub mod supervise;
pub mod sys;

use std::ffi::OsString;

use child::{ChildPlan, RECORD_LEN, Step};
use std::os::fd::AsRawFd;

use sys::{Cloned, Error as Errno};

pub use mount::{Degradation, MountPlan, Privileges};

/// Which namespaces the container gets.
///
/// Deliberately not a `bitflags` type: the cgroup namespace is **not** here
/// and must not be added, and an opaque flags word is the wrong place for
/// that rule to live.
///
/// `CLONE_NEWCGROUP` must never appear in the `clone3` flag word.
/// `copy_cgroup_ns()` roots a new cgroup namespace at the **parent's**
/// cgroup, so a child cloned with the flag is given a view anchored above
/// where we are about to place it. It has to be unshared by the child
/// *after* placement instead (RND.md §4.2). This is source-derived and not
/// yet measured; the test is named in `RND.md` §21.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Namespaces {
    pub pid: bool,
    pub uts: bool,
    pub ipc: bool,
    pub mount: bool,
    pub net: bool,
    /// A user namespace of our own, with a single-extent map of our own
    /// effective ids (stage 11).
    ///
    /// Set only when we do not already have `CAP_SYS_ADMIN`, so running as
    /// root takes exactly the path it always did. This is the whole of the
    /// rootless change at this layer: one more clone flag and a map the
    /// parent writes, because the privilege seam was expressed as "this
    /// operation, in this namespace, needs this capability" rather than
    /// "we are root" (RND.md §17, §18.6).
    pub user: bool,
}

impl Namespaces {
    /// Stage 1: the process-identity namespaces only. The container shares
    /// the host's filesystem view and network stack, and the CLI says so
    /// rather than implying isolation it does not have.
    pub const STAGE1: Self = Self {
        pid: true,
        uts: true,
        ipc: true,
        mount: false,
        net: false,
        user: false,
    };

    /// Stage 2: adds the mount namespace, which is what makes a private root
    /// filesystem possible.
    pub const STAGE2: Self = Self {
        mount: true,
        ..Self::STAGE1
    };

    /// Stage 8: a network stack of its own, containing only loopback.
    ///
    /// This is the default from here on. Host networking reaches every
    /// host-local service, including anything trusting 127.0.0.1 and the
    /// cloud metadata address, and defaults are the hardest thing to
    /// change later (RND.md §17).
    pub const STAGE8: Self = Self {
        net: true,
        ..Self::STAGE2
    };

    fn to_clone_flags(self) -> u64 {
        let mut f = 0u64;
        if self.pid {
            f |= sys::CLONE_NEWPID;
        }
        if self.uts {
            f |= sys::CLONE_NEWUTS;
        }
        if self.ipc {
            f |= sys::CLONE_NEWIPC;
        }
        if self.mount {
            f |= sys::CLONE_NEWNS;
        }
        if self.net {
            f |= sys::CLONE_NEWNET;
        }
        // Ordering inside one clone3 is the kernel's business, not ours: a
        // new user namespace is created first and the others are created
        // OWNED BY it, which is exactly what grants the capabilities the
        // rest of this list needs. Unsharing them separately afterwards
        // would not - the namespaces would belong to the old one.
        if self.user {
            f |= sys::CLONE_NEWUSER;
        }
        f
    }
}

/// What to run, and in what.
#[derive(Clone, Debug)]
pub struct Spec {
    /// Absolute path to the executable, **as resolved inside the container**.
    /// Not searched for on `PATH`: the search would have to happen in the
    /// child, where it cannot allocate.
    pub exe: OsString,
    /// `argv` in full, including `argv[0]`.
    pub argv: Vec<OsString>,
    /// `KEY=VALUE` entries. Not inherited implicitly — the caller decides.
    pub envp: Vec<OsString>,
    /// Hostname to set inside the UTS namespace. Ignored without `uts`.
    pub hostname: Option<OsString>,
    /// The image's `WorkingDir`, if it has one.
    pub workdir: Option<OsString>,
    /// Pre-built rtnetlink messages the container's init sends to
    /// configure its own interface. Built in the parent because the child
    /// cannot allocate.
    pub net_messages: Vec<net::Message>,
    pub namespaces: Namespaces,
}

/// How the container's init process ended.
///
/// There is no `Unknown` variant yet because at this stage the supervisor
/// *is* the waiting process and cannot miss the exit. Stage 3 introduces a
/// supervisor that can be killed, and with it the obligation to report an
/// unobserved exit as unknown rather than as `0`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Exit {
    Code(i32),
    Signal(i32),
}

impl Exit {
    /// The conventional shell encoding, for a process that must turn this
    /// into its own exit status.
    pub fn to_shell_status(self) -> i32 {
        match self {
            Exit::Code(c) => c,
            Exit::Signal(s) => 128 + s,
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error(
        "refusing to clone3: this process has {count} threads and must have exactly 1. \
         A raw clone inherits every lock in whatever state it was in, and glibc - unlike \
         after fork() - has had no chance to reset them. Something in the dependency \
         graph spawned a thread."
    )]
    MultiThreaded { count: u64 },

    #[error(
        "refusing to run: a mount plan was given without a mount namespace. Applying it \
         would mount over the HOST's filesystem, pivot the host's root, and leave the \
         damage behind after exit."
    )]
    MountsWithoutMountNamespace,

    #[error("preparing the child: {0}")]
    Plan(#[from] child::PlanError),

    #[error("creating the pipes the child reports through: {0}")]
    Pipe(#[source] Errno),

    #[error("clone3 failed: {0}")]
    Clone(#[source] Errno),

    #[error(
        "this process has no CAP_SYS_ADMIN, so it asked for a user namespace of its own \
         to get it - and the kernel refused it ({0}). Three separate settings can forbid \
         unprivileged user namespaces, each reporting a different error and none of them \
         naming itself, so check all three:\n\n    \
         sysctl user.max_user_namespaces                       # ENOSPC if 0\n    \
         sysctl kernel.unprivileged_userns_clone               # EPERM if 0 (Debian)\n    \
         sysctl kernel.apparmor_restrict_unprivileged_userns   # EPERM if 1 (Ubuntu)\n\n\
         A kernel built without CONFIG_USER_NS reports EINVAL and none of these exist. \
         Running as root needs none of them."
    )]
    // Carried as a plain field, not `#[source]`: the message interpolates
    // the errno where it reads naturally, and a source would make anyhow's
    // chain print it a second time at the end of eight lines of advice.
    UserNamespaceRefused(Errno),

    #[error(
        "the container's user namespace could not be given an id map, so every id in \
         it would read as the overflow id 65534: {0}. Only a single-extent map of \
         this process's own ids is supported - subuid-based multi-ID mapping and \
         the setuid helpers are not implemented."
    )]
    IdMap(#[source] std::io::Error),

    #[error("the container never started: {} failed with {errno}", describe(*step, *detail))]
    ChildFailed {
        step: Step,
        detail: Option<&'static str>,
        errno: Errno,
    },

    #[error("the container never started: {detail} failed: {reason} ({errno})")]
    ChildFailedWithReason {
        step: Step,
        detail: String,
        /// The kernel's own words, read from the `fs_context` descriptor.
        reason: String,
        errno: Errno,
    },

    #[error("waiting for the container: {0}")]
    Wait(#[source] Errno),

    #[error("the container process vanished without a wait status")]
    NoWaitStatus,
}

fn describe(step: Step, detail: Option<&'static str>) -> String {
    match detail {
        Some(d) => format!("{} ({d})", step.describe()),
        None => step.describe().to_owned(),
    }
}

/// A container process that exists but has not necessarily started.
pub struct Spawned {
    pub pid: i32,
    /// From `clone3(CLONE_PIDFD)` — no race at all, and no PID-reuse window.
    /// Pollable, so a death composes with the start rendezvous in one
    /// `poll` rather than a timer loop.
    pub pidfd: sys::OwnedFd,
    labels: Vec<&'static str>,
    err_rx: sys::OwnedFd,
    /// The go-ahead, held until the caller says the child may proceed.
    ///
    /// **Not** written when the clone returns. Everything the parent must
    /// do for the child - placing a veth peer in its namespace, writing a
    /// uid map - happens after the child exists and before it may run, and
    /// a go-ahead sent too early turns all of it into a race the child
    /// usually wins on a fast machine and loses on a loaded one.
    proceed_tx: Option<sys::OwnedFd>,
}

impl Spawned {
    /// Let the child proceed. Everything the parent owed it is done.
    pub fn release(&mut self) {
        if let Some(tx) = self.proceed_tx.take() {
            // EPIPE means the child is already gone, which the wait status
            // explains better than this write could. Rust sets SIGPIPE to
            // SIG_IGN at startup, so this returns an error rather than
            // killing us.
            let _ = sys::write(&tx, &[1u8]);
        }
    }

    /// The child's failure report, if it sent one.
    ///
    /// Reads until EOF, which a successful `execve` produces by closing the
    /// `O_CLOEXEC` write end. **Do not call this while the child may be
    /// parked at the start rendezvous** — it has not reached `execve` yet,
    /// so the pipe is still open and this would block. Call it after the
    /// pidfd says the child is gone.
    pub fn start_failure(&self) -> Option<Error> {
        let (step, index, errno, extra) = read_failure_record(&self.err_rx)?;
        if !extra.is_empty() {
            return Some(Error::ChildFailedWithReason {
                step,
                detail: self
                    .labels
                    .get(index as usize)
                    .copied()
                    .unwrap_or("")
                    .to_owned(),
                reason: extra,
                errno,
            });
        }
        let detail = match step {
            Step::Mount => self.labels.get(index as usize).copied(),
            // The security step sends which of its own thirteen sub-steps
            // failed; naming it is the difference between "permission
            // denied" and "the bounding-set drop was attempted after the
            // uid change".
            Step::Security => security::Step::from_u8(index).map(|s| s.describe()),
            _ => None,
        };
        Some(Error::ChildFailed {
            step,
            detail,
            errno,
        })
    }
}

/// Everything [`spawn`] attaches to the child that is not part of *what to
/// run*.
///
/// A struct and not five more positional parameters, for the reason
/// [`child::ChildSpec`] already gives: the list grew once per ladder stage,
/// and each addition silently changed the meaning of the arguments after it
/// at every call site.
#[derive(Default)]
pub struct Attach<'a> {
    /// Capabilities, user and seccomp, compiled parent-side.
    pub security: Option<security::Security>,
    /// Descriptors for 0, 1 and 2. `None` leaves the parent's in place.
    pub stdio: Option<child::Stdio>,
    /// The `create`/`start` rendezvous. `None` runs straight through.
    pub exec_fifo: Option<sys::OwnedFd>,
    /// Directory fd for `CLONE_INTO_CGROUP`.
    pub cgroup_fd: Option<&'a sys::OwnedFd>,
    /// Existing namespaces the **child** joins before anything else, in
    /// order. Non-empty only for `exec`.
    pub join: Vec<sys::OwnedFd>,
}

/// Create the container process. Returns in the parent only.
///
/// The caller decides the topology: stages 1–2 call [`run`], which waits
/// here; from stage 3 the supervisor calls this and keeps the pidfd.
pub fn spawn(spec: &Spec, mounts: Option<MountPlan>, attach: Attach<'_>) -> Result<Spawned, Error> {
    let Attach {
        security,
        stdio,
        exec_fifo,
        cgroup_fd,
        join,
    } = attach;
    // A mount plan without a mount namespace would be applied to the host:
    // every mount leaks, and `pivot_root` repoints the *host's* root. Refuse
    // rather than trust the caller to have set both.
    if mounts.is_some() && !spec.namespaces.mount {
        return Err(Error::MountsWithoutMountNamespace);
    }

    // The invariant, checked at the only moment it matters.
    let count = sys::thread_count().map_err(Error::Pipe)?;
    if count != 1 {
        return Err(Error::MultiThreaded { count });
    }

    let (proceed_rx, proceed_tx) = sys::pipe_with(sys::PipeFlags::CLOEXEC).map_err(Error::Pipe)?;
    let (err_rx, err_tx) = sys::pipe_with(sys::PipeFlags::CLOEXEC).map_err(Error::Pipe)?;

    // Keep the labels parent-side: the child can only send an index, and the
    // plan itself is about to move into the child's view.
    let labels = mounts.as_ref().map(|m| m.labels()).unwrap_or_default();

    let hostname = if spec.namespaces.uts {
        spec.hostname.as_deref()
    } else {
        None
    };
    let plan = ChildPlan::build(
        child::ChildSpec {
            exe: &spec.exe,
            argv: &spec.argv,
            envp: &spec.envp,
            hostname,
            workdir: spec.workdir.as_deref(),
            mounts,
            join,
            cgroup_ns: cgroup_fd.is_some(),
            loopback: spec.namespaces.net,
            net_messages: &spec.net_messages,
            security,
            stdio,
            exec_fifo,
        },
        proceed_rx,
        err_tx,
    )?;

    let mut pidfd_slot: i32 = -1;
    let mut args = sys::clone_args_zeroed();
    args.flags = spec.namespaces.to_clone_flags() | sys::CLONE_PIDFD;
    args.pidfd = &mut pidfd_slot as *mut i32 as u64;

    // Place the child in its cgroup at clone time, so there is no window in
    // which it exists outside its limits.
    //
    // `sys::CLONE_INTO_CGROUP` is our own constant, not `libc`'s: that one
    // is typed `c_int` with the value 2^33, truncates to **zero**, and
    // compiles under an `#[allow]`. ORing in zero makes clone3 succeed,
    // return a pid, and put the child in the PARENT's cgroup - unaccounted
    // and unlimited, with no error anywhere. The unit tests assert the
    // constant's value and the stage 4 success test reads the placement
    // back from inside the container (RND.md §15.3, §12.9).
    if let Some(fd) = cgroup_fd {
        args.flags |= sys::CLONE_INTO_CGROUP;
        args.cgroup = fd.as_raw_fd() as u64;
    }
    // Not ORed into `flags` - that is clone(2)'s convention and clone3
    // rejects it there. Without this the parent gets no SIGCHLD and the
    // child is not waitable as a child.
    args.exit_signal = libc::SIGCHLD as u64;

    // Both of these must happen immediately before the clone, and both are
    // the kind of thing a reviewer deletes as redundant.
    //
    // The flush is not about Rust's own buffering - `Stdout` is a
    // `LineWriter` unconditionally, so a Rust parent emitting whole lines has
    // nothing buffered. It is about anything else in the process: a
    // `BufWriter` we construct, a `print!` without a newline, any C library,
    // or a log subscriber buffering into a `Vec<u8>`. Flushing here makes the
    // whole duplicated-output class impossible regardless of what a
    // dependency did (RND.md §15.4.2).
    flush_stdio();

    match unsafe { sys::clone3(&args) } {
        // Three different settings forbid unprivileged user namespaces and
        // each reports a DIFFERENT errno, none of which names the cause.
        // ENOSPC is the memorable one: exceeding user.max_user_namespaces
        // is reported as "No space left on device", which sends everyone
        // off to check their disk.
        Err(e @ (Errno::PERM | Errno::NOSPC | Errno::INVAL)) if spec.namespaces.user => {
            Err(Error::UserNamespaceRefused(e))
        }
        Err(e) => Err(Error::Clone(e)),

        // SAFETY: this is the child branch, and nothing has allocated since
        // the clone returned.
        Ok(Cloned::Child) => unsafe { child::child_main(&plan) },

        Ok(Cloned::Parent(pid)) => {
            // The parent holds its own copies of the child's pipe ends.
            // Until they are closed, a read can never see EOF - and EOF is
            // precisely how a successful execve is reported. Dropping the
            // plan here is load-bearing, not tidiness.
            drop(plan);

            // SAFETY: the kernel wrote a valid descriptor here, because
            // CLONE_PIDFD was set and the clone succeeded.
            let pidfd =
                unsafe { <sys::OwnedFd as std::os::fd::FromRawFd>::from_raw_fd(pidfd_slot) };

            // The child exists with no mapping at all - every id in it
            // reads as the overflow id, 65534 - until this is written. It
            // is parked at the rendezvous, so it observes only the state
            // after the write, but the window is real and this is the
            // reason the rendezvous exists.
            if spec.namespaces.user
                && let Err(e) = write_single_id_map(pid)
            {
                let _ = sys::kill_process(
                    rustix::process::Pid::from_raw(pid).unwrap(),
                    sys::Signal::KILL,
                );
                let _ = reap_pid(pid);
                return Err(Error::IdMap(e));
            }

            Ok(Spawned {
                pid,
                pidfd,
                labels,
                err_rx,
                proceed_tx: Some(proceed_tx),
            })
        }
    }
}

/// Map our own effective ids to root inside the child's user namespace.
///
/// One extent, our own id, which is the whole of what an unprivileged
/// process may write for itself. **No helper and no intermediate
/// process**: the three-process topology rootless is assumed to need
/// applies to `subuid`-based multi-extent maps, which we do not support
/// and say so (RND.md §17).
///
/// Writing is the parent's job because the child cannot do it: a process
/// may not write its own map without `CAP_SETUID` in the parent namespace,
/// which is precisely what it does not have.
fn write_single_id_map(pid: i32) -> std::io::Result<()> {
    let uid = rustix::process::geteuid().as_raw();
    let gid = rustix::process::getegid().as_raw();

    std::fs::write(format!("/proc/{pid}/uid_map"), format!("0 {uid} 1\n"))?;

    // Order is load-bearing and the failure is silent if you get it wrong.
    // An unprivileged process must disable `setgroups(2)` BEFORE gid_map
    // may be written at all; afterwards the write returns EPERM and the
    // container runs with the parent's supplementary groups, which is the
    // privilege escalation the rule exists to prevent (RND.md §5).
    std::fs::write(format!("/proc/{pid}/setgroups"), "deny\n")?;
    std::fs::write(format!("/proc/{pid}/gid_map"), format!("0 {gid} 1\n"))?;
    Ok(())
}

/// Create the container, run it to completion, and report how it ended.
///
/// Used by stages 1–2 and by tests. From stage 3 the CLI goes through
/// [`supervise`] instead, because the CLI must never be in the container's
/// process ancestry (RND.md §18.2).
pub fn run(spec: &Spec, mounts: Option<MountPlan>) -> Result<Exit, Error> {
    let mut spawned = spawn(spec, mounts, Attach::default())?;
    // Nothing is owed to this child, so it may go at once.
    spawned.release();
    // Safe to read to EOF here: no start rendezvous was given, so the child
    // runs straight through to execve.
    let failure = spawned.start_failure();
    // Reap first, always: a child that failed is still a child, and leaving
    // it unreaped to report an error faster is how zombies accumulate.
    let status = reap_pid(spawned.pid);
    if let Some(e) = failure {
        return Err(e);
    }
    status
}

fn flush_stdio() {
    use std::io::Write;
    let _ = std::io::stdout().flush();
    let _ = std::io::stderr().flush();
}

/// Read the child's failure report, if there is one.
///
/// A successful `execve` closes the write end (it is `O_CLOEXEC`) and we see
/// EOF with nothing read. Anything else is a short, fixed-size record.
fn read_failure_record(err_rx: &sys::OwnedFd) -> Option<(Step, u8, Errno, String)> {
    let mut buf = [0u8; RECORD_LEN + child::DETAIL_LEN];
    let mut n = 0;
    while n < buf.len() {
        match sys::read(err_rx, &mut buf[n..]) {
            Ok(0) => break,
            Ok(got) => n += got,
            Err(Errno::INTR) => continue,
            Err(_) => break,
        }
    }
    if n == 0 {
        return None;
    }
    let (step, index, errno) = child::decode(&buf[..RECORD_LEN.min(n)])?;
    // Anything past the fixed record is the kernel's own explanation,
    // which for a mount failure is far more use than the errno.
    let extra = String::from_utf8_lossy(&buf[RECORD_LEN.min(n)..n])
        .trim()
        .trim_start_matches(['e', 'w', 'i', ' '])
        .to_owned();
    Some((step, index, errno, extra))
}

pub(crate) fn reap_pid(pid: libc::pid_t) -> Result<Exit, Error> {
    let pid = sys::Pid::from_raw(pid).ok_or(Error::NoWaitStatus)?;
    loop {
        match sys::waitpid(Some(pid), sys::WaitOptions::empty()) {
            // rustix hands back the pid alongside the status; we asked for
            // a specific pid, so only the status is news.
            Ok(Some((_, status))) => {
                return if let Some(code) = status.exit_status() {
                    Ok(Exit::Code(code))
                } else if let Some(sig) = status.terminating_signal() {
                    Ok(Exit::Signal(sig))
                } else {
                    Err(Error::NoWaitStatus)
                };
            }
            Ok(None) => return Err(Error::NoWaitStatus),
            Err(Errno::INTR) => continue,
            Err(e) => return Err(Error::Wait(e)),
        }
    }
}

impl Spec {
    /// A spec that runs `exe` with the current environment, in stage 1's
    /// namespace set.
    pub fn new(exe: impl Into<OsString>, argv: Vec<OsString>) -> Self {
        let exe = exe.into();
        let argv = if argv.is_empty() {
            vec![exe.clone()]
        } else {
            argv
        };
        Self {
            exe,
            argv,
            envp: inherited_env(),
            hostname: None,
            workdir: None,
            net_messages: Vec::new(),
            namespaces: Namespaces::STAGE1,
        }
    }
}

/// The parent's environment as `KEY=VALUE` entries.
fn inherited_env() -> Vec<OsString> {
    use std::os::unix::ffi::{OsStrExt, OsStringExt};
    std::env::vars_os()
        .map(|(k, v)| {
            let mut e = Vec::with_capacity(k.len() + v.len() + 1);
            e.extend_from_slice(k.as_bytes());
            e.push(b'=');
            e.extend_from_slice(v.as_bytes());
            OsString::from_vec(e)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The footgun this guard exists for: a mount plan applied without a
    /// mount namespace mounts over the host and pivots the host's root.
    #[test]
    fn a_mount_plan_without_a_mount_namespace_is_refused() {
        let dir = std::env::temp_dir().join("rustainer-guard-test");
        std::fs::create_dir_all(&dir).unwrap();
        let plan = mount::plan(&dir, Privileges::probe(), None, None, &[]).unwrap();

        let mut spec = Spec::new("/bin/true", vec![]);
        spec.namespaces = Namespaces::STAGE1; // mount: false
        assert!(matches!(
            run(&spec, Some(plan)),
            Err(Error::MountsWithoutMountNamespace)
        ));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn stage2_adds_only_the_mount_namespace() {
        const { assert!(!Namespaces::STAGE1.mount) };
        const { assert!(Namespaces::STAGE2.mount) };
        const { assert!(!Namespaces::STAGE2.net, "network is stage 8") };
        let only_mount_differs = Namespaces {
            mount: false,
            ..Namespaces::STAGE2
        };
        assert_eq!(only_mount_differs, Namespaces::STAGE1);
    }

    #[test]
    fn exit_encodes_signals_the_way_a_shell_does() {
        assert_eq!(Exit::Code(42).to_shell_status(), 42);
        assert_eq!(Exit::Signal(9).to_shell_status(), 137);
        assert_eq!(Exit::Signal(15).to_shell_status(), 143);
    }
}
