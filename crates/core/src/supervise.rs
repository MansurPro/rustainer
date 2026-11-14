//! The supervisor: one process per container, outliving the CLI.
//!
//! ```text
//! rustainer run                              CLI - transient, exits
//!   mkdir <dir>; write config.json
//!   open <dir>/lock O_CLOEXEC; flock         <- O_CLOEXEC is load-bearing
//!   fork/exec self "supervise <id>"          with a handshake pipe on fd 3
//!   read handshake: OK + init_pid, or an error
//!   print the id; exit                       CLI leaves the ancestry
//!   |
//!   '--> rustainer supervise <id>            SUPERVISOR - outlives the CLI
//!          setsid(); PR_SET_CHILD_SUBREAPER
//!          mkfifo exec.fifo; open it O_PATH
//!          stdio pipes, holding both ends
//!          clone3(... | CLONE_PIDFD)
//!          |
//!          '--> container init               PID 1 in its own pidns
//!                 mounts; pivot_root; hostname
//!                 dup2 stdio
//!                 open(exec.fifo, O_WRONLY)  <- BLOCKS until started
//!                 execve(user program)
//!          write state (created); reply OK
//!          poll { exec.fifo, pidfd }         <- never a blocking open
//!          write state (running)
//!          poll { pidfd, stdout, stderr }
//!          on exit: drain and flush the logs, THEN write (exited, code)
//! ```
//!
//! The CLI is never in the container's process ancestry. That single
//! structural fact decides whether detach, `ps` after the CLI exits,
//! exit-code collection and restart policies are features or rewrites, and
//! process ancestry is the one thing in a runtime that cannot be changed
//! incrementally (RND.md §18.2).

use std::io::Write;
use std::os::fd::{FromRawFd, OwnedFd};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::cgroup::{self, Cgroup, Limits};
use crate::firewall::{self, Publish};
use crate::net::{self, Lease};
use crate::proxy::Forwarder;
use crate::security::{self, CapabilitySet, Security, UserSpec};
use crate::state::{self, Lock, Paths, State, Status};
use crate::{Exit, MountPlan, Namespaces, Privileges, Spec, mount, sys};

/// The container's bundle, written by the CLI and read by the supervisor.
///
/// Deliberately our own shape rather than an OCI `config.json`: the schema
/// is a **defer safely** item, while the directory layout around it is not
/// (RND.md §18.11). Converting this into the OCI document later touches one
/// file.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Config {
    pub id: String,
    pub exe: String,
    pub argv: Vec<String>,
    pub envp: Vec<String>,
    pub hostname: String,
    pub rootfs: Option<String>,
    #[serde(default)]
    pub memory: Option<u64>,
    #[serde(default)]
    pub cpus: Option<f64>,
    #[serde(default)]
    pub pids: Option<u64>,
    /// Capabilities kept in the bounding set, by name. `None` means the
    /// default set.
    #[serde(default)]
    pub capabilities: Option<Vec<String>>,
    #[serde(default)]
    pub user: Option<UserSpec>,
    /// A seccomp profile as JSON, or `None` for the default one.
    #[serde(default)]
    pub seccomp: Option<serde_json::Value>,
    /// Install no filter at all.
    ///
    /// A separate flag rather than `seccomp: Some(Null)`, because
    /// `Option<Value>` round-trips a JSON `null` back to `None` - so the
    /// marker for "explicitly unconfined" was indistinguishable from
    /// "unspecified" the moment it passed through the config file.
    #[serde(default)]
    pub seccomp_unconfined: bool,
    #[serde(default)]
    pub workdir: Option<String>,
    /// Extracted layer directories, lowest first. Empty means `rootfs` is
    /// used directly.
    #[serde(default)]
    pub layers: Vec<String>,
    /// No writable upper: the container's filesystem is read-only.
    #[serde(default)]
    pub read_only: bool,
    /// Where the container's writable upper and work directories go.
    ///
    /// Beside the image layers rather than in the runtime state directory,
    /// because the state directory is often a tmpfs or - inside a
    /// container - an overlayfs, and **overlayfs cannot be an upperdir**.
    /// The kernel says so in as many words, which is now surfaced.
    #[serde(default)]
    pub overlay_dir: Option<String>,
    /// A network namespace of the container's own.
    #[serde(default)]
    pub private_net: bool,
    /// Attach to the bridge and get an address. Implies `private_net`.
    #[serde(default)]
    pub bridge: bool,
    /// The address handed to this container, recorded so `ps` and teardown
    /// do not have to recompute it.
    #[serde(default)]
    pub ip: Option<String>,
    /// Ports published to the host.
    #[serde(default)]
    pub ports: Vec<Publish>,
    /// Run in a user namespace of our own, mapping our effective ids to
    /// root inside it (stage 11).
    ///
    /// Decided once by the CLI and recorded here rather than probed again
    /// in the supervisor: the two processes must not be able to disagree
    /// about it, because the mount plan is built from one answer and the
    /// clone flags from the other.
    #[serde(default)]
    pub userns: bool,
}

impl Config {
    pub fn path(paths: &Paths, id: &str) -> std::path::PathBuf {
        paths.dir(id).join("config.json")
    }
    pub fn store(&self, paths: &Paths) -> std::io::Result<()> {
        let bytes = serde_json::to_vec_pretty(self).map_err(std::io::Error::other)?;
        std::fs::write(Self::path(paths, &self.id), bytes)
    }
    pub fn load(paths: &Paths, id: &str) -> std::io::Result<Self> {
        let bytes = std::fs::read(Self::path(paths, id))?;
        serde_json::from_slice(&bytes).map_err(std::io::Error::other)
    }

    pub fn to_spec(&self) -> Spec {
        Spec {
            exe: self.exe.clone().into(),
            argv: self.argv.iter().map(Into::into).collect(),
            envp: self.envp.iter().map(Into::into).collect(),
            hostname: Some(self.hostname.clone().into()),
            workdir: self.workdir.clone().map(Into::into),
            net_messages: match (self.bridge, &self.ip) {
                (true, Some(ip)) => ip
                    .parse()
                    .map(|ip| {
                        net::container_messages(&Lease {
                            ip,
                            prefix: net::SUBNET_PREFIX,
                            gateway: net::GATEWAY,
                        })
                    })
                    .unwrap_or_default(),
                _ => Vec::new(),
            },
            namespaces: Namespaces {
                mount: self.rootfs.is_some(),
                net: self.private_net || self.bridge,
                user: self.userns,
                ..Namespaces::STAGE1
            },
        }
    }

    pub fn limits(&self) -> Limits {
        Limits {
            memory: self.memory,
            cpus: self.cpus,
            pids: self.pids,
        }
    }

    /// The resolved security posture, compiled parent-side.
    pub fn security(&self) -> Result<Security, Error> {
        let keep = match &self.capabilities {
            None => security::default_capabilities(),
            Some(names) => {
                let mut set = CapabilitySet::empty();
                for n in names {
                    match security::capability_by_name(n) {
                        Some(c) => set |= c,
                        None => {
                            return Err(Error::Io(std::io::Error::other(format!(
                                "unknown capability {n:?}"
                            ))));
                        }
                    }
                }
                set
            }
        };
        let profile = if self.seccomp_unconfined {
            None
        } else {
            match &self.seccomp {
                None => Some(security::default_profile()),
                Some(v) => Some(
                    serde_json::from_value(v.clone())
                        .map_err(|e| security::SeccompError::Parse(e.to_string()))?,
                ),
            }
        };
        let filter = match profile {
            None => Vec::new(),
            Some(p) => security::compile(&p)?,
        };
        Ok(Security::build(keep, self.user.clone(), filter))
    }

    /// `/etc/resolv.conf` for a container, written to the state directory.
    ///
    /// The host's file cannot simply be copied. **`127.0.0.53` is
    /// meaningless inside a container network namespace**: each namespace
    /// has its own loopback, so a container sending to `127.0.0.53:53`
    /// reaches *its own* loopback, where nothing is listening. Since
    /// systemd-resolved is the default on Ubuntu, Fedora and Arch, copying
    /// naively produces a container whose DNS silently never answers -
    /// which every user misdiagnoses as "networking is broken".
    fn write_resolv_conf(&self, dir: &Path) -> std::io::Result<PathBuf> {
        let mut servers: Vec<String> = std::fs::read_to_string("/etc/resolv.conf")
            .unwrap_or_default()
            .lines()
            .filter_map(|l| l.strip_prefix("nameserver"))
            .map(|s| s.trim().to_owned())
            .filter(|s| !s.starts_with("127.") && !s.is_empty())
            .collect();
        if servers.is_empty() {
            // Every nameserver the host has is a local stub. Fall back to
            // a public resolver and say so in the file, rather than
            // writing something that cannot work.
            servers.push("1.1.1.1".into());
            servers.push("8.8.8.8".into());
        }
        let mut body = String::new();
        if servers.len() == 2 && servers[0] == "1.1.1.1" {
            body.push_str(
                "# The host's resolvers are all on 127.0.0.0/8 (systemd-resolved or similar),\n\
                 # which is a different loopback inside this container's network namespace.\n",
            );
        }
        for s in &servers {
            body.push_str(&format!("nameserver {s}\n"));
        }
        let path = dir.join("resolv.conf");
        std::fs::write(&path, body)?;
        Ok(path)
    }

    fn write_hosts(&self, dir: &Path) -> std::io::Result<PathBuf> {
        let mut body =
            String::from("127.0.0.1\tlocalhost\n::1\tlocalhost ip6-localhost ip6-loopback\n");
        if let Some(ip) = &self.ip {
            body.push_str(&format!("{ip}\t{}\n", self.hostname));
        }
        let path = dir.join("hosts");
        std::fs::write(&path, body)?;
        Ok(path)
    }

    pub fn mount_plan(
        &self,
        cgroup_dir: Option<&Path>,
        state_dir: &Path,
    ) -> Result<Option<MountPlan>, Error> {
        let Some(r) = &self.rootfs else {
            return Ok(None);
        };
        let overlay = if self.layers.is_empty() {
            None
        } else {
            // `upper` and `work` are created as SIBLINGS in one directory.
            // OverlayFS requires them on the same **mount**, and a st_dev
            // comparison accepts configurations the kernel rejects - so
            // the guarantee is structural rather than checked.
            // The upper is created even when read-only: OverlayFS needs
            // two lower layers when there is no upper at all, so leaving
            // it out makes a single-layer image unmountable read-only.
            let (upper, work) = {
                let base = self
                    .overlay_dir
                    .as_ref()
                    .map(PathBuf::from)
                    .unwrap_or_else(|| state_dir.to_path_buf());
                let u = base.join("upper");
                let w = base.join("work");
                std::fs::create_dir_all(&u)?;
                std::fs::create_dir_all(&w)?;
                (Some(u), Some(w))
            };
            Some(mount::Overlay {
                layers: self.layers.iter().map(PathBuf::from).collect(),
                upper,
                work,
                read_only: self.read_only,
            })
        };
        std::fs::create_dir_all(r)?;

        // Thirty lines of file generation that are a PREREQUISITE of
        // bridge networking, not a successor to it. A container with an
        // address, a route and working NAT but no nameserver looks broken
        // in a way every user misdiagnoses.
        let mut binds: Vec<(PathBuf, &str)> = Vec::new();
        if self.bridge {
            binds.push((self.write_resolv_conf(state_dir)?, "etc/resolv.conf"));
            binds.push((self.write_hosts(state_dir)?, "etc/hosts"));
        }

        // Built against the CHILD's privileges, not ours: when we are
        // about to clone it into a user namespace of its own, it will have
        // CAP_SYS_ADMIN there and we do not have it here.
        let privs = if self.userns {
            Privileges::probe().in_new_user_namespace()
        } else {
            Privileges::probe()
        };
        Ok(Some(mount::plan(
            Path::new(r),
            privs,
            cgroup_dir,
            overlay.as_ref(),
            &binds,
        )?))
    }
}

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error(transparent)]
    Core(#[from] crate::Error),
    #[error(transparent)]
    Plan(#[from] mount::PlanError),
    #[error(transparent)]
    Cgroup(#[from] cgroup::Error),
    #[error(transparent)]
    Seccomp(#[from] security::SeccompError),
    #[error(transparent)]
    Firewall(#[from] firewall::Error),
    #[error(
        "IPv4 forwarding is off, so a bridged container can reach the bridge and nothing \
         beyond it. Turning it on here would be a destructive change - the kernel resets \
         every other forwarding parameter to its default when ip_forward is written - so \
         run this yourself instead:\n\n    sysctl -w net.ipv4.ip_forward=1\n"
    )]
    ForwardingOff,
    #[error("{0}")]
    Io(#[from] std::io::Error),
    #[error("syscall: {0}")]
    Sys(#[source] sys::Error),

    #[error(
        "bridged networking needs CAP_NET_ADMIN in the user namespace that owns the \
         HOST's network namespace, and this process is in a nested one, which can never \
         have it. Degraded: no bridge, no address, no NAT, so a container cannot reach \
         the network or any other container.\n\n\
         Run `--network none` for a stack with only loopback, or run as root for a \
         bridge. Rootless networking by way of a setuid helper or a userspace TCP/IP \
         stack is deliberately not implemented."
    )]
    BridgeUnprivileged,
    #[error(
        "the container's init process died before it reached the start rendezvous; \
         it never ran the command"
    )]
    DiedBeforeStart,

    #[error("container {id} is {status}, not running")]
    NotRunning { id: String, status: &'static str },
    #[error("joining the container's {ns} namespace: {source}")]
    JoinNamespace {
        ns: &'static str,
        #[source]
        source: sys::Error,
    },
    #[error(
        "{cmd:?} is not in the container's PATH directories. Give an absolute path, \
         or check the image actually contains it."
    )]
    NotFoundInContainer { cmd: String },
}

impl From<sys::Error> for Error {
    fn from(e: sys::Error) -> Self {
        Error::Sys(e)
    }
}

/// The handshake record the supervisor writes back to the CLI on fd 3.
///
/// Fixed-size and positional, because the CLI must be able to tell "the
/// supervisor started and here is the pid" from "it failed" without parsing.
pub const HANDSHAKE_LEN: usize = 5;
const HANDSHAKE_OK: u8 = 1;
const HANDSHAKE_ERR: u8 = 2;
/// The descriptor the CLI puts the handshake pipe on before `execve`.
pub const HANDSHAKE_FD: i32 = 3;

fn handshake_ok(pid: i32) -> [u8; HANDSHAKE_LEN] {
    let p = pid.to_ne_bytes();
    [HANDSHAKE_OK, p[0], p[1], p[2], p[3]]
}

pub fn decode_handshake(record: &[u8]) -> Option<Result<i32, ()>> {
    if record.len() != HANDSHAKE_LEN {
        return None;
    }
    let pid = i32::from_ne_bytes([record[1], record[2], record[3], record[4]]);
    match record[0] {
        HANDSHAKE_OK => Some(Ok(pid)),
        HANDSHAKE_ERR => Some(Err(())),
        _ => None,
    }
}

/// Run one container to completion. Returns how it ended.
///
/// Called in a freshly `execve`'d process, which is what guarantees the
/// single-thread invariant `clone3` depends on.
pub fn supervise(paths: &Paths, id: &str) -> Result<Exit, Error> {
    // Our own session, so a terminal hangup on the shell that launched the
    // CLI cannot reach us: SIGHUP goes to the session leader of the session
    // that terminal controls. `setsid` fails EPERM when we are already a
    // process-group leader - which happens when `supervise` is run straight
    // from an interactive shell - and that is harmless, so tolerate it
    // rather than refusing to start.
    // Called `exe` otherwise, for every container on the machine. The
    // supervisor is started by executing `/proc/self/exe` - which is what
    // makes the CVE-2019-5736 seal work - and the kernel names a process
    // after the final component of the path it executed.
    let _ = sys::set_process_name("rustainer-sup");

    let _ = sys::setsid();

    // For descendants only. The search in `find_new_reaper` never crosses a
    // PID namespace boundary, so this does NOT make us the reaper for
    // anything inside the container - the container's own PID 1 is. It
    // matters for processes we later place in the container from outside,
    // which is `exec`'s problem, not this stage's.
    let _ = sys::set_child_subreaper(Some(rustix::process::getpid()));

    let config = Config::load(paths, id)?;
    let spec = config.to_spec();

    let mut st = State::load(paths, id).unwrap_or_else(|_| {
        State::new(
            id.to_owned(),
            std::iter::once(config.exe.clone())
                .chain(config.argv.iter().skip(1).cloned())
                .collect(),
        )
    });
    st.supervisor_pid = Some(std::process::id() as i32);
    st.supervisor_start_time = state::start_time(std::process::id() as i32);
    st.rootfs.clone_from(&config.rootfs);
    st.hostname = Some(config.hostname.clone());
    st.status = Status::Creating;
    st.store(paths)?;

    // --- the start rendezvous ------------------------------------------
    let fifo_path = paths.fifo(id);
    if !fifo_path.exists() {
        // 0622, deliberately: the container's init opens this for writing
        // *after* dropping to a non-root user, so a 0600 FIFO owned by the
        // engine would be unopenable and every container would hang at the
        // rendezvous. What contains it is the mode of the state directory,
        // which is asserted 0700 on every startup (RND.md §5.5).
        sys::mkfifo(&cpath(&fifo_path)?, 0o622)?;
    }
    // O_PATH, not O_WRONLY: a write-open would block here, before the child
    // even exists.
    let fifo_opath = sys::open_path_file(&cpath(&fifo_path)?)?;

    // --- stdio ----------------------------------------------------------
    let (out_r, out_w) = sys::pipe_with(sys::PipeFlags::CLOEXEC)?;
    let (err_r, err_w) = sys::pipe_with(sys::PipeFlags::CLOEXEC)?;
    let devnull = sys::open_devnull()?;
    let stdio = crate::child::Stdio {
        stdin: devnull,
        stdout: out_w.try_clone()?,
        stderr: err_w.try_clone()?,
    };

    // Resource limits, before the child exists. A requested limit that
    // cannot be applied fails here, loudly, rather than producing a
    // container that silently has none.
    let limits = config.limits();
    let cg = Cgroup::prepare(id, &limits)?;
    if let Some(c) = &cg {
        c.apply(&limits)?;
    }
    let mounts = config.mount_plan(cg.as_ref().map(|c| c.dir()), &paths.dir(id))?;

    let mut spawned = crate::spawn(
        &spec,
        mounts,
        crate::Attach {
            security: Some(config.security()?),
            stdio: Some(stdio),
            exec_fifo: Some(fifo_opath),
            cgroup_fd: cg.as_ref().map(|c| c.fd()),
            ..Default::default()
        },
    )?;

    // Our copies of the child's write ends must go, or the reads below can
    // never see EOF.
    drop(out_w);
    drop(err_w);

    // The host side of the network, before the child is let go. The
    // container's interface has to exist before the container tries to
    // configure it, and the go-ahead is what guarantees the ordering.
    if config.bridge
        && let Err(e) = plumb_network(spawned.pid)
    {
        let _ = sys::kill_process(
            rustix::process::Pid::from_raw(spawned.pid).unwrap(),
            sys::Signal::KILL,
        );
        let _ = crate::reap_pid(spawned.pid);
        return Err(e);
    }

    st.init_pid = Some(spawned.pid);
    st.init_start_time = state::start_time(spawned.pid);
    st.status = Status::Created;
    st.store(paths)?;

    // Published ports: the firewall rules and the userland forwarder for
    // the loopback case. Before the go-ahead, so a container is never
    // briefly running with a port published and unauthorised.
    let _published = match publish_ports(&config) {
        Ok(p) => p,
        Err(e) => {
            let _ = sys::kill_process(
                rustix::process::Pid::from_raw(spawned.pid).unwrap(),
                sys::Signal::KILL,
            );
            let _ = crate::reap_pid(spawned.pid);
            return Err(e);
        }
    };

    // Everything the parent owed this child is done.
    spawned.release();

    // Tell the CLI it may leave. Everything after this point happens with
    // nobody waiting on us.
    reply_ok(spawned.pid);

    // --- wait for the start byte, WITHOUT a blocking open ----------------
    //
    // The naive version opens the FIFO read side blocking and reads a byte.
    // That deadlocks forever whenever the init process died between create
    // and start, which is trivially reachable: the user's binary does not
    // exist. Polling the FIFO together with a pidfd turns that into a real
    // error instead of a hang, with no background thread and no PID-reuse
    // window.
    let fifo_read = sys::open_read_nonblocking(&cpath(&fifo_path)?)?;
    let started = wait_for_start(&fifo_read, &spawned.pidfd)?;
    if !started {
        let err = spawned.start_failure().map(Error::Core);
        let status = reap_into(&mut st, spawned.pid);
        st.status = Status::Exited;
        st.exited_at = Some(state::now());
        st.store(paths)?;
        let _ = status;
        return Err(err.unwrap_or(Error::DiedBeforeStart));
    }

    st.status = Status::Running;
    st.started_at = Some(state::now());
    st.store(paths)?;

    // --- the steady state ------------------------------------------------
    let mut out_log = log_file(&paths.stdout_log(id))?;
    let mut err_log = log_file(&paths.stderr_log(id))?;
    pump(&out_r, &err_r, &spawned.pidfd, &mut out_log, &mut err_log)?;

    // Drain and flush the logs BEFORE writing the terminal status. Writing
    // `exited` first and then failing to flush produces a container
    // reported as cleanly exited whose last few KiB of output - the panic
    // message - is gone (RND.md §5.7).
    out_log.flush()?;
    err_log.flush()?;

    // Now that init is gone its `O_CLOEXEC` error pipe is closed, so this
    // cannot block. A failure recorded here means the child reached the
    // start rendezvous and then failed - execve of a missing binary is the
    // common case, and it is the difference between "exited 127" and
    // "exited 127 because /bin/nonexistent does not exist".
    let failure = spawned.start_failure();
    let exit = reap_into(&mut st, spawned.pid);

    // `memory.events`'s `oom_kill` is the only key that means a kill
    // happened; `max` and `oom` do not. Read before the cgroup is removed.
    if let Some(c) = &cg {
        st.oom_killed = c.oom_killed();
    }
    if let Some(e) = &failure {
        eprintln!("rustainer: {e}");
        st.error = Some(e.to_string());
    }
    st.status = Status::Exited;
    st.exited_at = Some(state::now());
    st.store(paths)?;
    if let Some(c) = &cg {
        c.remove();
    }
    Ok(exit)
}

fn cpath(p: &Path) -> Result<std::ffi::CString, Error> {
    use std::os::unix::ffi::OsStrExt;
    std::ffi::CString::new(p.as_os_str().as_bytes())
        .map_err(|_| Error::Io(std::io::Error::other("path contains an interior NUL")))
}

fn log_file(p: &Path) -> std::io::Result<std::fs::File> {
    use std::os::unix::fs::OpenOptionsExt;
    std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .mode(0o600)
        .open(p)
}

fn reply_ok(pid: i32) {
    // fd 3 is the handshake pipe when the CLI started us; when a human runs
    // `supervise` by hand it is simply absent, and that is not an error.
    let fd = unsafe { OwnedFd::from_raw_fd(HANDSHAKE_FD) };
    if rustix::fs::fstat(&fd).is_ok() {
        let _ = sys::write(&fd, &handshake_ok(pid));
    }
    drop(fd);
}

/// `true` if the start byte arrived, `false` if init died first.
fn wait_for_start(fifo: &OwnedFd, pidfd: &OwnedFd) -> Result<bool, Error> {
    loop {
        let mut fds = [
            sys::PollFd::new(fifo, sys::PollFlags::IN),
            sys::PollFd::new(pidfd, sys::PollFlags::IN),
        ];
        match sys::poll(&mut fds, None) {
            Ok(_) => {}
            Err(sys::Error::INTR) => continue,
            Err(e) => return Err(e.into()),
        }
        if !fds[0].revents().is_empty() {
            let mut b = [0u8; 1];
            match sys::read(fifo, &mut b) {
                // A writer opened and closed without writing can only mean
                // the init process died at exactly that point.
                Ok(0) => return Ok(false),
                Ok(_) => return Ok(true),
                Err(sys::Error::AGAIN) | Err(sys::Error::INTR) => continue,
                Err(e) => return Err(e.into()),
            }
        }
        if !fds[1].revents().is_empty() {
            return Ok(false);
        }
    }
}

/// Copy the container's output until it exits, then drain what is left.
fn pump(
    out_r: &OwnedFd,
    err_r: &OwnedFd,
    pidfd: &OwnedFd,
    out_log: &mut std::fs::File,
    err_log: &mut std::fs::File,
) -> Result<(), Error> {
    let mut out_open = true;
    let mut err_open = true;
    let mut exited = false;

    while !exited || out_open || err_open {
        let mut fds = Vec::with_capacity(3);
        if out_open {
            fds.push(sys::PollFd::new(out_r, sys::PollFlags::IN));
        }
        if err_open {
            fds.push(sys::PollFd::new(err_r, sys::PollFlags::IN));
        }
        if !exited {
            fds.push(sys::PollFd::new(pidfd, sys::PollFlags::IN));
        }
        if fds.is_empty() {
            break;
        }
        match sys::poll(&mut fds, None) {
            Ok(_) => {}
            Err(sys::Error::INTR) => continue,
            Err(e) => return Err(e.into()),
        }

        let mut i = 0;
        if out_open {
            if !fds[i].revents().is_empty() {
                out_open = copy_once(out_r, out_log, 1)?;
            }
            i += 1;
        }
        if err_open {
            if !fds[i].revents().is_empty() {
                err_open = copy_once(err_r, err_log, 2)?;
            }
            i += 1;
        }
        if !exited && !fds[i].revents().is_empty() {
            exited = true;
            // Do NOT break here. The pipes may still hold output the
            // container wrote just before dying, and dropping it would make
            // a crash look like a silent exit.
        }
    }
    Ok(())
}

/// One read; `false` once the pipe is at EOF.
fn copy_once(src: &OwnedFd, log: &mut std::fs::File, passthrough: i32) -> Result<bool, Error> {
    let mut buf = [0u8; 8192];
    match sys::read(src, &mut buf) {
        Ok(0) => Ok(false),
        Ok(n) => {
            log.write_all(&buf[..n])?;
            // Also to our own stdout/stderr. Detached, those are /dev/null
            // and this costs a write to nowhere; in the foreground they are
            // the user's terminal. One path, no branch.
            let mut out = unsafe { std::mem::ManuallyDrop::new(OwnedFd::from_raw_fd(passthrough)) };
            let _ = sys::write(&mut *out, &buf[..n]);
            Ok(true)
        }
        Err(sys::Error::AGAIN) | Err(sys::Error::INTR) => Ok(true),
        Err(_) => Ok(false),
    }
}

fn reap_into(st: &mut State, pid: i32) -> Exit {
    match crate::reap_pid(pid) {
        Ok(Exit::Code(c)) => {
            st.exit_code = Some(c);
            st.signal = None;
            Exit::Code(c)
        }
        Ok(Exit::Signal(s)) => {
            // Kept as a signal, never flattened into 128+N: the distinction
            // is the difference between "the program chose to fail" and
            // "something killed it", and the OOM case later needs it.
            st.exit_code = None;
            st.signal = Some(s);
            Exit::Signal(s)
        }
        Err(_) => {
            st.exit_code = None;
            st.signal = None;
            Exit::Code(-1)
        }
    }
}

/// Everything a container published, undone when this is dropped.
///
/// RAII rather than a call at the end of `supervise`, because everything
/// between the publish and the teardown can return early with `?`. This is
/// the only place in the project where leftover state is a **security
/// hole** rather than a failure: a DNAT without its filter accept merely
/// does not work, whereas rules outliving their container are inherited by
/// whichever container is handed that address next (RND.md §17).
struct Published {
    /// Dropped first, so the listener stops before the rules go.
    forwarders: Vec<Forwarder>,
    ports: Vec<Publish>,
}

impl Drop for Published {
    fn drop(&mut self) {
        // Stop listening before removing the rules, so there is no window
        // in which the port is reachable but no longer authorised.
        self.forwarders.clear();
        unpublish_ports(&self.ports);
    }
}

/// Publish every requested port: rules first, then the listener.
///
/// Partial failure is undone by dropping the half-built [`Published`], so
/// there is one rollback path rather than one per failure point.
fn publish_ports(config: &Config) -> Result<Published, Error> {
    let mut out = Published {
        forwarders: Vec::new(),
        ports: Vec::new(),
    };
    if config.ports.is_empty() {
        return Ok(out);
    }
    let fw = firewall::detect().ok_or(firewall::Error::NoBackend)?;
    for p in &config.ports {
        // Recorded BEFORE the rules are written. `publish` writes two
        // rules and can fail between them, so a failed publish has still
        // mutated the host and the rollback has to know about it.
        out.ports.push(*p);
        fw.publish(p, net::BRIDGE)?;
        // The forwarder covers `curl localhost:<port>`, which no firewall
        // rule can: loopback is deliberately excluded from destination NAT.
        out.forwarders
            .push(Forwarder::start(*p).map_err(Error::Io)?);
    }
    Ok(out)
}

/// Idempotent: removing a rule that is not there is not an error, which is
/// what lets both the rollback and the teardown call it blindly.
fn unpublish_ports(ports: &[Publish]) {
    if ports.is_empty() {
        return;
    }
    if let Some(fw) = firewall::detect() {
        for p in ports {
            fw.unpublish(p, net::BRIDGE);
        }
    }
}

/// Create the bridge if needed, make a veth pair, and put the peer
/// straight into the container's namespace.
///
/// The largest irreversible host mutation in the project is here: a bridge
/// holding an address and an on-link route reserves a subnet as far as any
/// future allocator is concerned. It is created once and reused, never per
/// container.
fn plumb_network(pid: i32) -> Result<(), Error> {
    // Stage 11: rootless, every one of these operations fails with EPERM
    // and the bare errno names nothing. The gate is CAP_NET_ADMIN in the
    // user namespace that owns the TARGET namespace - here the host's,
    // which is owned by the initial one - so a nested namespace cannot
    // reach it however much capability it holds inside itself.
    //
    // Translated at the boundary rather than guarded at the top, because
    // "are we allowed to do this" has no honest answer short of trying:
    // under `unshare --user --net` the host side is our own new namespace
    // and these calls genuinely do succeed.
    match plumb(pid) {
        Err(Error::Sys(sys::Error::PERM)) => Err(Error::BridgeUnprivileged),
        other => other,
    }
}

fn plumb(pid: i32) -> Result<(), Error> {
    let nl = net::Netlink::open()?;

    // The bridge, once. `EEXIST` is the normal case after the first
    // container and is not an error.
    if net::if_index(net::BRIDGE).is_none() {
        match nl.request(&net::create_bridge(net::BRIDGE)) {
            Ok(()) | Err(sys::Error::EXIST) => {}
            Err(e) => return Err(e.into()),
        }
        let idx = net::if_index(net::BRIDGE).ok_or(sys::Error::NODEV)?;
        match nl.request(&net::add_address(idx, net::GATEWAY, net::SUBNET_PREFIX)) {
            Ok(()) | Err(sys::Error::EXIST) => {}
            Err(e) => return Err(e.into()),
        }
        nl.request(&net::link_up(net::BRIDGE))?;
    }
    let bridge_idx = net::if_index(net::BRIDGE).ok_or(sys::Error::NODEV)?;

    // The container's namespace, as a descriptor. The peer goes straight
    // into it, so its interface never appears on the host even briefly.
    let ns = std::fs::File::open(format!("/proc/{pid}/ns/net")).map(std::os::fd::OwnedFd::from)?;
    // Named by pid: short enough for IFNAMSIZ and unique while the
    // container lives, which is exactly as long as the interface does.
    let host_if = format!("rtv{pid}");
    nl.request(&net::create_veth(&host_if, "eth0", &ns))?;
    nl.request(&net::set_master(&host_if, bridge_idx))?;
    nl.request(&net::link_up(&host_if))?;

    // Checked here rather than first, because of which refusal is useful.
    // Forwarding being off is something the operator can fix in one
    // command; not having CAP_NET_ADMIN over the host's namespace is
    // something they cannot fix at all without becoming root. Reporting
    // the fixable one first sends a rootless user off to run a sysctl
    // that changes nothing about their problem. So the operations that
    // need the capability go first, and whichever genuinely blocks us is
    // the one we name.
    if !net::ip_forward_enabled() {
        return Err(Error::ForwardingOff);
    }

    // Source NAT, so replies can find their way back. Without it a
    // container sends packets and never receives one: the rest of the
    // internet has no route to RFC 1918 space, so the SYN-ACK is dropped
    // and the failure looks like "everything times out".
    let fw = firewall::detect().ok_or(firewall::Error::NoBackend)?;
    fw.ensure_masquerade(net::GATEWAY, net::SUBNET_PREFIX, net::BRIDGE)?;
    // And the forward path. A FORWARD policy of DROP - set by Kubernetes,
    // libvirt, firewalld and most cloud images - silently blackholes every
    // container's egress, with a correct address, route and NAT rule in
    // place. The symptom is "everything times out".
    if fw.foreign_drop_policy() {
        eprintln!(
            "rustainer: this host has a FORWARD policy of DROP that something else set. \
             Accept rules for {} have been added to our own chain; if another ruleset \
             drops the packet first, containers will have no egress.",
            net::BRIDGE
        );
    }
    fw.ensure_forward(net::BRIDGE)?;
    Ok(())
}

/// Stop a container: `SIGTERM`, a grace period, then `SIGKILL`.
///
/// The grace period is real, not decorative. A container whose PID 1 never
/// installed a `SIGTERM` handler **ignores it** — a namespace's init has no
/// default disposition. Worse, it cannot even kill itself: `SIGNAL_UNKILLABLE`
/// drops `SIGKILL` too when the sender is inside the namespace with
/// `SIG_DFL`. Only a signal from an ancestor namespace gets through, which
/// is why this runs here and not in the container.
pub fn stop(paths: &Paths, id: &str, grace: std::time::Duration) -> Result<(), Error> {
    let st = State::load(paths, id)?;
    if st.observed_status() != Status::Running && st.observed_status() != Status::Created {
        return Ok(());
    }
    let Some(pid) = st.init_pid else {
        return Ok(());
    };
    if !state::alive(pid, st.init_start_time) {
        return Ok(());
    }
    let rpid = rustix::process::Pid::from_raw(pid).ok_or(Error::DiedBeforeStart)?;
    let _ = sys::kill_process(rpid, sys::Signal::TERM);

    let deadline = std::time::Instant::now() + grace;
    while std::time::Instant::now() < deadline {
        if !state::alive(pid, st.init_start_time) {
            return wait_for_supervisor(&st);
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    let _ = sys::kill_process(rpid, sys::Signal::KILL);
    wait_for_supervisor(&st)
}

/// Wait for the supervisor to record the exit, so `ps` right after `stop`
/// shows the truth rather than a stale `running`.
fn wait_for_supervisor(st: &State) -> Result<(), Error> {
    let Some(sup) = st.supervisor_pid else {
        return Ok(());
    };
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    while std::time::Instant::now() < deadline {
        if !state::alive(sup, st.supervisor_start_time) {
            return Ok(());
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    Ok(())
}

/// Remove a container's directory.
///
/// Refuses a running container rather than orphaning its supervisor.
pub fn remove(paths: &Paths, id: &str, force: bool) -> Result<(), Error> {
    let _lock = Lock::acquire(&paths.lock_file(id))?;
    if let Ok(st) = State::load(paths, id) {
        let observed = st.observed_status();
        if matches!(observed, Status::Running | Status::Created) {
            if !force {
                return Err(Error::Io(std::io::Error::other(format!(
                    "container {id} is {observed}; stop it first, or pass --force"
                ))));
            }
            stop(paths, id, std::time::Duration::from_secs(5))?;
        }
    }
    // The writable layer lives outside the state directory, so removing
    // the container has to remove it too or every `rm` leaks a tree.
    if let Ok(cfg) = Config::load(paths, id) {
        if let Some(d) = &cfg.overlay_dir {
            let _ = std::fs::remove_dir_all(d);
        }
        // The one case RAII cannot reach: a supervisor killed with
        // SIGKILL runs no destructor, so its rules outlive it. They are
        // idempotent to remove, so sweep them here rather than leave a
        // DNAT pointing at an address the next container will be handed.
        unpublish_ports(&cfg.ports);
    }
    drop(_lock);
    std::fs::remove_dir_all(paths.dir(id))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_handshake_round_trips() {
        assert_eq!(decode_handshake(&handshake_ok(4321)), Some(Ok(4321)));
        assert_eq!(
            decode_handshake(&[HANDSHAKE_ERR, 0, 0, 0, 0]),
            Some(Err(()))
        );
        assert_eq!(decode_handshake(&[9, 0, 0, 0, 0]), None);
        assert_eq!(decode_handshake(&[1, 2]), None, "short record");
    }

    #[test]
    fn a_config_round_trips_and_picks_its_namespaces() {
        let paths = Paths::new(std::env::temp_dir().join("rustainer-cfg-test"));
        let _ = std::fs::remove_dir_all(std::env::temp_dir().join("rustainer-cfg-test"));
        paths.ensure_root().unwrap();
        std::fs::create_dir_all(paths.dir("c1")).unwrap();

        let mut c = Config {
            id: "c1".into(),
            exe: "/bin/sh".into(),
            argv: vec!["sh".into()],
            envp: vec!["PATH=/bin".into()],
            hostname: "box".into(),
            rootfs: None,
            memory: None,
            cpus: None,
            pids: None,
            capabilities: None,
            user: None,
            seccomp: None,
            seccomp_unconfined: false,
            workdir: None,
            layers: Vec::new(),
            read_only: false,
            overlay_dir: None,
            private_net: false,
            bridge: false,
            ip: None,
            ports: Vec::new(),
            userns: false,
        };
        c.store(&paths).unwrap();
        let back = Config::load(&paths, "c1").unwrap();
        assert_eq!(back.exe, "/bin/sh");
        assert!(!back.to_spec().namespaces.mount, "no rootfs: no mount ns");

        c.rootfs = Some("/tmp".into());
        assert!(
            c.to_spec().namespaces.mount,
            "a rootfs without a mount namespace would pivot the host's root"
        );
        let _ = std::fs::remove_dir_all(std::env::temp_dir().join("rustainer-cfg-test"));
    }
}
