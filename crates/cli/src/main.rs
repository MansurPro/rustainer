//! The transient front end.
//!
//! This process parses arguments, starts a supervisor, and prints. It owns
//! no container state and no background work, because the engine is
//! daemonless by design: a per-container supervisor solves the one problem
//! the kernel actually forces — keeping a container's I/O attached after the
//! client detaches — and a daemon that owns containers cannot later have the
//! daemon removed, since process ancestry is not refactorable (RND.md §18.1).
//!
//! The CLI is **never** in the container's process ancestry, even in the
//! foreground. That is not an optimisation; it is the single structural fact
//! that decides whether detach, `ps` after the CLI exits and exit-code
//! collection are features or rewrites (§18.2).

use std::ffi::OsString;
use std::io::Read;
use std::os::fd::{AsRawFd, OwnedFd};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use clap::{Args, Parser, Subcommand};
use rustainer_core::firewall::Publish;
use rustainer_core::security::{self, UserSpec};
use rustainer_core::state::{Paths, State, Status};
use rustainer_core::supervise::{self, Config, HANDSHAKE_FD, HANDSHAKE_LEN};
use rustainer_core::{Privileges, sys};
use rustainer_image::Store;

#[derive(Parser)]
#[command(
    name = "rustainer",
    version,
    about = "A container engine written in Rust from first principles.",
    long_about = "A container engine written in Rust from first principles - owning the \
                  runtime core rather than shelling out to runc, crun or containerd.\n\n\
                  This is stage 3 of 11: lifecycle and supervision. There is no image, no \
                  layering, no resource limit, no security control and no network isolation \
                  yet, and the flags refuse rather than pretend."
)]
struct Cli {
    /// Directory holding runtime state. Defaults to /run/rustainer as root,
    /// or $XDG_RUNTIME_DIR/rustainer otherwise.
    #[arg(long, global = true)]
    root: Option<PathBuf>,

    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Run a command in a new container.
    //
    // Boxed: `run` carries far more options than the other subcommands, and
    // an enum is as large as its largest variant. One allocation in a
    // process that exits in milliseconds.
    Run(Box<RunArgs>),
    /// List containers.
    Ps(PsArgs),
    /// Stop a running container: SIGTERM, a grace period, then SIGKILL.
    Stop(StopArgs),
    /// Remove a container's state directory.
    Rm(RmArgs),
    /// Print a container's captured output.
    Logs(LogsArgs),
    /// Download an image and extract it into the local store.
    Pull(PullArgs),
    /// List images in the local store.
    Images,
    /// Remove an image from the local store.
    Rmi(RmiArgs),
    /// Internal: become the supervisor for an already-prepared container.
    #[command(hide = true)]
    Supervise { id: String },
}

#[derive(Args)]
struct RunArgs {
    /// Root filesystem: a directory tree you extracted yourself. `/` keeps
    /// the host's filesystem view. Omit it and the first positional
    /// argument is an image reference instead.
    #[arg(long)]
    rootfs: Option<PathBuf>,

    /// Pull the image again even if it is already in the store.
    #[arg(long)]
    pull: bool,

    /// Stack this directory as a layer, lowest first. Repeatable, and an
    /// alternative to naming an image.
    #[arg(long, value_name = "DIR")]
    layer: Vec<PathBuf>,

    /// No writable upper layer: the container's filesystem is read-only.
    #[arg(long)]
    read_only: bool,

    /// `bridge` (the default) attaches the container to a bridge and gives
    /// it an address, so it can reach the internet and other containers.
    /// `none` gives it a stack with nothing but loopback. `host` shares the
    /// host's, which reaches every host-local service including anything
    /// trusting 127.0.0.1 and the cloud metadata address.
    #[arg(long, default_value = "bridge")]
    network: String,

    /// Hostname inside the container's UTS namespace.
    #[arg(long, default_value = "container")]
    hostname: String,

    /// Memory limit, e.g. 512m, 2g. Enforced by cgroup v2 memory.max.
    #[arg(short = 'm', long)]
    memory: Option<String>,

    /// CPU limit as a fraction of one CPU, e.g. 1.5. Absolute, not a share:
    /// a container at --cpus=0.5 is throttled to half a CPU even on an idle
    /// 64-core machine.
    #[arg(long)]
    cpus: Option<f64>,

    /// Maximum number of tasks. Counts threads, not processes.
    #[arg(long)]
    pids_limit: Option<u64>,

    /// Run the container's process as this user. Numeric `uid` or
    /// `uid:gid`.
    #[arg(short = 'u', long)]
    user: Option<String>,

    /// Add a capability to the default set, e.g. --cap-add NET_ADMIN.
    #[arg(long, value_name = "CAP")]
    cap_add: Vec<String>,

    /// Drop a capability from the default set, or ALL.
    #[arg(long, value_name = "CAP")]
    cap_drop: Vec<String>,

    /// `seccomp=<file.json>` for an OCI seccomp profile, or
    /// `seccomp=unconfined` to install no filter at all.
    #[arg(long, value_name = "OPT")]
    security_opt: Vec<String>,

    /// Publish a container port to the host: `[host-ip:]host-port:container-port`.
    /// Repeatable. TCP only.
    #[arg(short = 'p', long = "publish", value_name = "SPEC")]
    publish: Vec<String>,

    /// Start the container and return immediately, printing its id.
    #[arg(short, long)]
    detach: bool,

    /// Name the container instead of generating an id.
    #[arg(long)]
    name: Option<String>,

    /// With `--rootfs`: the command, which must be an absolute path.
    /// Without it: an image reference, then an optional command overriding
    /// the image's own.
    #[arg(required = true, num_args = 1.., trailing_var_arg = true)]
    command: Vec<OsString>,
}

#[derive(Args)]
struct PullArgs {
    /// Image reference, e.g. `alpine`, `alpine:3.21`, `ghcr.io/o/r@sha256:…`.
    image: String,
    /// Fetch and re-extract even if it is already in the store.
    #[arg(long)]
    force: bool,
}

#[derive(Args)]
struct RmiArgs {
    image: String,
}

#[derive(Args)]
struct PsArgs {
    /// Include containers that have exited.
    #[arg(short, long)]
    all: bool,
}

#[derive(Args)]
struct StopArgs {
    id: String,
    /// Seconds to wait for the container to leave before SIGKILL.
    #[arg(short = 't', long, default_value_t = 10)]
    time: u64,
}

#[derive(Args)]
struct RmArgs {
    id: String,
    /// Stop the container first if it is still running.
    #[arg(short, long)]
    force: bool,
}

#[derive(Args)]
struct LogsArgs {
    id: String,
}

fn main() -> ExitCode {
    // RND.md §18.10, and the first thing this binary does. A container that
    // can write to `/proc/self/exe` overwrites the runtime binary itself -
    // CVE-2019-5736. Re-executing from a sealed memfd makes the path refer
    // to a copy nothing can modify.
    if let Err(e) = pentacle::ensure_sealed() {
        eprintln!("rustainer: could not seal /proc/self/exe: {e}");
        return ExitCode::FAILURE;
    }

    match real_main() {
        Ok(code) => code,
        Err(e) => {
            eprintln!("rustainer: {e:#}");
            ExitCode::FAILURE
        }
    }
}

fn real_main() -> Result<ExitCode> {
    let cli = Cli::parse();
    let paths = Paths::new(cli.root.unwrap_or_else(Paths::default_root));
    paths
        .ensure_root()
        .context("preparing the state directory")?;

    match cli.command {
        Command::Run(a) => cmd_run(&paths, *a),
        Command::Ps(a) => cmd_ps(&paths, a),
        Command::Stop(a) => cmd_stop(&paths, a),
        Command::Rm(a) => cmd_rm(&paths, a),
        Command::Logs(a) => cmd_logs(&paths, a),
        Command::Pull(a) => cmd_pull(a),
        Command::Images => cmd_images(),
        Command::Rmi(a) => cmd_rmi(a),
        Command::Supervise { id } => cmd_supervise(&paths, &id),
    }
}

// ---------------------------------------------------------------------------
// run
// ---------------------------------------------------------------------------

fn cmd_run(paths: &Paths, args: RunArgs) -> Result<ExitCode> {
    let (private_net, bridge) = match args.network.as_str() {
        "bridge" => (true, true),
        "none" => (true, false),
        "host" => {
            eprintln!(
                "rustainer: --network host shares the host's network stack. The container \
                 can reach every host-local service, including anything that trusts \
                 127.0.0.1 and the cloud metadata address."
            );
            (false, false)
        }
        other => bail!(
            "--network {other} is not supported. `bridge` gives the container an address \
             and a route off the machine; `none` gives it only loopback; `host` shares \
             the host's stack. Custom networks are not implemented."
        ),
    };

    // Three shapes, chosen by --rootfs:
    //   --rootfs /       the host's filesystem view (stage 1)
    //   --rootfs <dir>   a tree you extracted yourself (stage 2)
    //   no --rootfs      an image reference (stage 6)
    let mut image_env: Vec<String> = Vec::new();
    let mut workdir: Option<String> = None;
    let mut image_user: Option<String> = None;

    let mut layers: Vec<String> = Vec::new();

    let (rootfs, command) = match (&args.rootfs, args.layer.is_empty()) {
        (Some(p), true) if p == Path::new("/") => {
            require_absolute(&args.command[0])?;
            (None, args.command.clone())
        }
        (Some(p), true) => {
            if !p.is_dir() {
                bail!(
                    "--rootfs {p:?} is not a directory. It should be an extracted root \
                     filesystem - or omit --rootfs and name an image instead."
                );
            }
            require_absolute(&args.command[0])?;
            (
                Some(p.canonicalize()?.to_string_lossy().into_owned()),
                args.command.clone(),
            )
        }
        (_, false) => {
            // Explicit layers: the rootfs is the mount point we create for
            // the overlay, not a tree that already exists.
            for d in &args.layer {
                if !d.is_dir() {
                    bail!("--layer {d:?} is not a directory");
                }
                layers.push(d.canonicalize()?.to_string_lossy().into_owned());
            }
            (Some(String::new()), args.command.clone())
        }
        (None, true) => {
            let reference = args.command[0].to_string_lossy().into_owned();
            let store = Store::new(Store::default_root());
            let image = store
                .pull(&reference, args.pull)
                .with_context(|| format!("pulling {reference}"))?;

            // Say what the extractor refused. A device node in a layer is
            // not a corrupt image - it is a layer asking for a direct path
            // to host memory - and silently dropping it would hide that.
            for d in &image.report.devices_skipped {
                eprintln!("rustainer: refused a device node in the image: {d}");
            }
            if image.report.xattrs_dropped > 0 {
                eprintln!(
                    "rustainer: dropped {} security.*/system.* extended attribute(s) from the image",
                    image.report.xattrs_dropped
                );
            }
            if image.report.modes_relaxed > 0 {
                eprintln!(
                    "rustainer: widened {} directory mode(s) so the layer could be extracted",
                    image.report.modes_relaxed
                );
            }

            image_env = image.env.clone();
            workdir = image.working_dir.clone();
            image_user = image.user.clone();
            layers = image
                .layers
                .iter()
                .map(|p| p.to_string_lossy().into_owned())
                .collect();

            let override_cmd: Vec<OsString> = args.command[1..].to_vec();
            let cmd = if override_cmd.is_empty() {
                let c = image.command();
                if c.is_empty() {
                    bail!(
                        "{reference} declares neither an Entrypoint nor a Cmd, so there is \
                         nothing to run. Give a command after the image name."
                    );
                }
                c.into_iter().map(OsString::from).collect()
            } else {
                override_cmd
            };
            (Some(String::new()), cmd)
        }
    };

    // With layers, the rootfs is a mount point inside the container's own
    // state directory, created by the supervisor.
    let mut rootfs = rootfs;
    let mut layer_search: Option<String> = None;
    if !layers.is_empty() {
        // The topmost layer is where a command is most likely to be found,
        // and resolving a bare name needs a tree to look in before the
        // overlay exists.
        layer_search = layers.last().cloned();
    }

    let exe = resolve_exe(&command[0], layer_search.as_deref().or(rootfs.as_deref()))?;

    // Stage 11. Without CAP_SYS_ADMIN here, the only way to get it is to
    // make a user namespace and be root inside that. We do it ourselves
    // rather than telling the user to re-run under `unshare`, and the
    // whole of the change is this flag plus the map the parent writes -
    // which is the stage's actual success criterion.
    let userns = !Privileges::probe().sys_admin;
    if rootfs.is_some() || !layers.is_empty() {
        // No pre-flight check that user namespaces are permitted. Three
        // separate knobs can forbid them and each fails differently, so
        // any check here is a second, worse copy of the one the kernel
        // performs at clone3 - and one that can disagree with it. The
        // clone is where the truth is, so that is where the explanation
        // lives (see `Error::Clone` handling in rustainer-core).
        let privs = if userns {
            Privileges::probe().in_new_user_namespace()
        } else {
            Privileges::probe()
        };
        if !privs.mknod_devices {
            eprintln!(
                "rustainer: degraded: device nodes - mknod of a device requires CAP_MKNOD in \
                 the INITIAL user namespace, and we are in a nested one; the host's nodes \
                 are bind-mounted instead"
            );
        }
    }

    // Move ourselves out of the cgroup that is about to delegate to the
    // container. A CLI left sitting in it keeps it non-empty, and
    // `cgroup.subtree_control` cannot be written for a cgroup that holds
    // processes - so every limit is refused with a message about a busy
    // cgroup that is in fact about us.
    let limits_requested =
        args.memory.is_some() || args.cpus.is_some() || args.pids_limit.is_some();
    if limits_requested && let Err(e) = rustainer_core::cgroup::enter_own_leaf() {
        // Stage 11: rootless this is the normal case, not a malfunction.
        // A limit accepted and never enforced is how a host OOMs three
        // weeks later, so it stays a hard error - but it names what is
        // degraded and how to get the delegation back.
        if userns {
            bail!(
                "the limits you asked for cannot be enforced: this process has no \
                 writable cgroup to delegate from ({e}). A user namespace grants no \
                 authority over the cgroup hierarchy - that is a separate delegation \
                 the host has to make.\n\n\
                 Degraded: no memory, CPU or PID limit. Either drop the limit flags, or \
                 get a delegated cgroup first:\n\n    \
                 systemd-run --user --scope -p Delegate=yes rustainer run ...\n\n\
                 A limit that is accepted and never enforced is how a host runs out of \
                 memory three weeks later, so this refuses rather than degrading quietly."
            );
        }
        bail!(
            "could not move into our own cgroup leaf, so the limits you asked for cannot \
             be enforced: {e}"
        );
    }

    // An address, before the container exists: the netlink messages the
    // container sends to configure itself are built from it in the parent.
    let lease = if bridge {
        Some(
            rustainer_core::net::allocate(&paths_root(paths))
                .context("allocating an address for the container")?,
        )
    } else {
        None
    };

    let id = match args.name {
        Some(n) => {
            if n.is_empty() || n.contains('/') || n.starts_with('.') {
                bail!("{n:?} is not a usable container name");
            }
            n
        }
        None => new_id()?,
    };

    // `create_new` is the uniqueness check: EEXIST means the id is taken,
    // and the check is the mkdir itself rather than a stat-then-mkdir race.
    std::fs::DirBuilder::new()
        .create(paths.dir(&id))
        .with_context(|| format!("container {id} already exists"))?;

    let (seccomp_profile, seccomp_unconfined) = resolve_seccomp(&args.security_opt)?;
    let config = Config {
        id: id.clone(),
        exe: exe.to_string_lossy().into_owned(),
        argv: command
            .iter()
            .map(|a| a.to_string_lossy().into_owned())
            .collect(),
        // The container's environment is the image's, never the host's.
        //
        // Inheriting the caller's environment is the obvious thing and the
        // wrong thing: it leaks whatever the operator happened to have
        // exported into every container, and it overrides the PATH the
        // image set for its own binaries. Docker does not do it either.
        envp: if image_env.is_empty() {
            vec![
                "PATH=/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin".to_owned(),
                "HOME=/root".to_owned(),
            ]
        } else {
            image_env
        },
        hostname: args.hostname,
        // `None` means the host's filesystem view. An empty string means
        // "a mount point the supervisor creates", which is what layers
        // need: there is no tree yet, only an overlay to put there.
        rootfs: match rootfs.take() {
            None => None,
            Some(s) if s.is_empty() => {
                Some(paths.dir(&id).join("rootfs").to_string_lossy().into_owned())
            }
            Some(s) => Some(s),
        },
        workdir,
        layers: layers.clone(),
        read_only: args.read_only,
        // Beside the layers, on whatever filesystem the image store is on.
        // The runtime state directory is usually a tmpfs and sometimes an
        // overlayfs, and overlayfs cannot host an upperdir at all.
        private_net,
        bridge,
        userns,
        ip: lease.map(|l| l.ip.to_string()),
        ports: {
            if !args.publish.is_empty() && !bridge {
                bail!(
                    "-p needs a container with an address, so it only works with \
                     --network bridge. With `host` the container already shares every \
                     port; with `none` there is nothing to reach."
                );
            }
            match lease {
                None => Vec::new(),
                Some(l) => args
                    .publish
                    .iter()
                    .map(|s| parse_publish(s, l.ip))
                    .collect::<Result<Vec<_>>>()?,
            }
        },
        overlay_dir: if layers.is_empty() {
            None
        } else {
            Some(
                Store::default_root()
                    .join("containers")
                    .join(&id)
                    .to_string_lossy()
                    .into_owned(),
            )
        },
        memory: args.memory.as_deref().map(parse_size).transpose()?,
        cpus: args.cpus,
        pids: args.pids_limit,
        capabilities: resolve_capabilities(&args.cap_add, &args.cap_drop)?,
        user: match args.user.as_ref().or(image_user.as_ref()) {
            None => None,
            Some(u) => {
                let spec = UserSpec::parse(u).map_err(|e| anyhow::anyhow!("--user: {e}"))?;
                if let Some(why) = spec.unmapped_reason(userns) {
                    bail!("--user {u}: {why}");
                }
                Some(spec)
            }
        },
        seccomp: seccomp_profile,
        seccomp_unconfined,
    };
    if let Some(c) = config.cpus
        && c <= 0.0
    {
        bail!("--cpus must be greater than 0; {c} would mean no CPU at all");
    }
    config.store(paths)?;
    State::new(id.clone(), config.argv.clone()).store(paths)?;

    // The lock is taken here and dropped when this process exits. It is
    // opened O_CLOEXEC, so the supervisor's execve releases its inherited
    // duplicate - without that, the supervisor would hold this lock for the
    // container's entire life and every later stop/rm/logs would block.
    let lock = rustainer_core::state::Lock::acquire(&paths.lock_file(&id))
        .context("locking the container directory")?;

    let (handshake_rx, handshake_tx) = sys::pipe()?;
    let mut child = spawn_supervisor(paths, &id, handshake_tx, args.detach)?;
    let init_pid = read_handshake(&handshake_rx, &mut child, &id)?;

    // Release before doing anything slow: `stop` and `rm` on this container
    // must not be blocked by a foreground `run` that is merely waiting.
    drop(lock);

    if args.detach {
        // Reap the supervisor's *launcher* relationship: it is our child
        // only until we exit, after which init takes it over. Nothing to
        // wait for.
        println!("{id}");
        return Ok(ExitCode::SUCCESS);
    }

    let status = wait_foreground(paths, &id, &mut child, init_pid)?;
    Ok(ExitCode::from(status.min(255) as u8))
}

/// Fork and exec ourselves as the supervisor.
///
/// `Command` rather than a hand-rolled fork: this process is not the one
/// that calls `clone3`, so it is under no single-thread obligation, and the
/// supervisor gets that guarantee for free by being a fresh `execve`.
fn spawn_supervisor(
    paths: &Paths,
    id: &str,
    handshake_tx: OwnedFd,
    detach: bool,
) -> Result<std::process::Child> {
    use std::os::unix::process::CommandExt;
    use std::process::{Command, Stdio};

    // `/proc/self/exe`, not `current_exe()`. We sealed ourselves into a
    // memfd at startup, so `current_exe()` now resolves to something like
    // "/memfd:rustainer (deleted)" - a path that cannot be spawned. The
    // magic symlink still re-executes the sealed image, which is the whole
    // point of the seal: the binary the supervisor runs is a copy nothing
    // can modify, including a container with access to /proc/self/exe.
    let mut cmd = Command::new("/proc/self/exe");
    cmd.arg("--root")
        .arg(paths_root(paths))
        .arg("supervise")
        .arg(id);

    cmd.stdin(Stdio::null());
    if detach {
        // Detached: the container's output goes to the log files only.
        cmd.stdout(Stdio::null()).stderr(Stdio::null());
    } else {
        // Foreground: the supervisor inherits our streams and tees the
        // container's output onto them. One code path in the supervisor,
        // no branch.
        cmd.stdout(Stdio::inherit()).stderr(Stdio::inherit());
    }

    let raw = handshake_tx.as_raw_fd();
    unsafe {
        cmd.pre_exec(move || {
            // dup2 clears O_CLOEXEC on the destination, which is exactly
            // what we need: fd 3 must survive the execve.
            if libc::dup2(raw, HANDSHAKE_FD) < 0 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }

    let child = cmd.spawn().context("starting the supervisor")?;
    // Our copy must go, or the read below never sees EOF when the
    // supervisor dies without replying.
    drop(handshake_tx);
    Ok(child)
}

fn read_handshake(rx: &OwnedFd, child: &mut std::process::Child, id: &str) -> Result<i32> {
    let mut buf = [0u8; HANDSHAKE_LEN];
    let mut n = 0;
    while n < HANDSHAKE_LEN {
        match sys::read(rx, &mut buf[n..]) {
            Ok(0) => break,
            Ok(got) => n += got,
            Err(sys::Error::INTR) => continue,
            Err(e) => bail!("reading the supervisor handshake: {e}"),
        }
    }
    if n == HANDSHAKE_LEN
        && let Some(Ok(pid)) = supervise::decode_handshake(&buf)
    {
        return Ok(pid);
    }
    // No handshake: the supervisor died before replying. Its exit status and
    // whatever it printed are the real diagnosis.
    let status = child.wait().ok();
    bail!(
        "the supervisor for {id} exited without starting the container{}",
        match status.and_then(|s| s.code()) {
            Some(c) => format!(" (exit {c})"),
            None => String::new(),
        }
    )
}

/// Foreground: forward signals to the container, wait for the supervisor.
fn wait_foreground(
    paths: &Paths,
    id: &str,
    child: &mut std::process::Child,
    init_pid: i32,
) -> Result<i32> {
    // No signal handler. A handler runs at an arbitrary point under the same
    // async-signal-safety rules as the post-clone window; blocking the
    // signals and reading them from a descriptor makes them ordinary events.
    // Blocking must happen before the signalfd exists, or a signal arriving
    // in between takes its default action - which is to kill us.
    let signals = [libc::SIGINT, libc::SIGTERM, libc::SIGQUIT, libc::SIGHUP];
    sys::block_signals(&signals)?;
    let sfd = sys::signalfd(&signals)?;

    let sup_pid =
        rustix::process::Pid::from_raw(child.id() as i32).context("the supervisor has no pid")?;
    let sup_fd = sys::pidfd_open(sup_pid, sys::PidfdFlags::empty())
        .context("opening a pidfd for the supervisor")?;

    loop {
        let mut fds = [
            sys::PollFd::new(&sup_fd, sys::PollFlags::IN),
            sys::PollFd::new(&sfd, sys::PollFlags::IN),
        ];
        match sys::poll(&mut fds, None) {
            Ok(_) => {}
            Err(sys::Error::INTR) => continue,
            Err(e) => bail!("waiting for the container: {e}"),
        }
        if !fds[1].revents().is_empty()
            && let Ok(Some(signo)) = sys::read_signal(&sfd)
        {
            // Forward to the container's init. It may well ignore it: PID 1
            // in a new namespace has no default disposition, so a container
            // that installed no handler is unaffected. That is the kernel's
            // behaviour, not a bug here, and `stop` is what escalates.
            if let Some(p) = rustix::process::Pid::from_raw(init_pid)
                && let Some(sig) = signal_from_raw(signo)
            {
                let _ = sys::kill_process(p, sig);
            }
        }
        if !fds[0].revents().is_empty() {
            break;
        }
    }

    child.wait().context("reaping the supervisor")?;

    let st = State::load(paths, id).context("reading the container's final state")?;
    Ok(match (st.exit_code, st.signal) {
        (Some(c), _) => c,
        (None, Some(s)) => {
            eprintln!("rustainer: container killed by signal {s}");
            128 + s
        }
        (None, None) => {
            eprintln!(
                "rustainer: the container's exit status was never observed; reporting failure \
                 rather than inventing a 0"
            );
            125
        }
    })
}

/// Only the signals we block are forwardable, and an explicit match is
/// better than a numeric cast: rustix's `from_raw_unchecked` would happily
/// accept a number that is not a signal at all.
fn signal_from_raw(signo: i32) -> Option<sys::Signal> {
    Some(match signo {
        libc::SIGINT => sys::Signal::INT,
        libc::SIGTERM => sys::Signal::TERM,
        libc::SIGQUIT => sys::Signal::QUIT,
        libc::SIGHUP => sys::Signal::HUP,
        _ => return None,
    })
}

// ---------------------------------------------------------------------------
// ps / stop / rm / logs / supervise
// ---------------------------------------------------------------------------

fn cmd_ps(paths: &Paths, args: PsArgs) -> Result<ExitCode> {
    let mut ids = paths.list()?;
    ids.sort();
    println!(
        "{:<14} {:<9} {:<8} {:<8} COMMAND",
        "ID", "STATUS", "PID", "EXIT"
    );
    for id in ids {
        let Ok(st) = State::load(paths, &id) else {
            continue;
        };
        let observed = st.observed_status();
        if !args.all && matches!(observed, Status::Exited | Status::Unknown) {
            continue;
        }
        let pid = match (observed, st.init_pid) {
            (Status::Running | Status::Created, Some(p)) => p.to_string(),
            _ => "-".into(),
        };
        // An exit code we never saw is printed as "?", never as 0.
        // An OOM kill is reported as an OOM kill. Collapsing it into
        // "killed by signal 9" sends the user to look for whoever sent the
        // signal, and there was no sender.
        let exit = match (observed, st.exit_code, st.signal, st.oom_killed) {
            (Status::Exited, _, _, true) => "oom".into(),
            (Status::Exited, Some(c), _, _) => c.to_string(),
            (Status::Exited, None, Some(s), _) => format!("sig{s}"),
            (Status::Unknown, _, _, _) => "?".into(),
            _ => "-".into(),
        };
        println!(
            "{:<14} {:<9} {:<8} {:<10} {}",
            id,
            observed.to_string(),
            pid,
            exit,
            st.command.join(" ")
        );
    }
    Ok(ExitCode::SUCCESS)
}

fn cmd_stop(paths: &Paths, args: StopArgs) -> Result<ExitCode> {
    supervise::stop(paths, &args.id, Duration::from_secs(args.time))
        .with_context(|| format!("stopping {}", args.id))?;
    println!("{}", args.id);
    Ok(ExitCode::SUCCESS)
}

fn cmd_rm(paths: &Paths, args: RmArgs) -> Result<ExitCode> {
    supervise::remove(paths, &args.id, args.force)
        .with_context(|| format!("removing {}", args.id))?;
    println!("{}", args.id);
    Ok(ExitCode::SUCCESS)
}

fn cmd_logs(paths: &Paths, args: LogsArgs) -> Result<ExitCode> {
    if !paths.dir(&args.id).exists() {
        bail!("no such container: {}", args.id);
    }
    dump(&paths.stdout_log(&args.id), &mut std::io::stdout())?;
    dump(&paths.stderr_log(&args.id), &mut std::io::stderr())?;
    Ok(ExitCode::SUCCESS)
}

fn dump(path: &Path, out: &mut impl std::io::Write) -> Result<()> {
    match std::fs::File::open(path) {
        Ok(mut f) => {
            let mut buf = Vec::new();
            f.read_to_end(&mut buf)?;
            out.write_all(&buf)?;
            Ok(())
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e.into()),
    }
}

fn cmd_pull(args: PullArgs) -> Result<ExitCode> {
    let store = Store::new(Store::default_root());
    let image = store
        .pull(&args.image, args.force)
        .with_context(|| format!("pulling {}", args.image))?;
    for d in &image.report.devices_skipped {
        eprintln!("rustainer: refused a device node in the image: {d}");
    }
    println!("{}", image.digest);
    Ok(ExitCode::SUCCESS)
}

fn cmd_images() -> Result<ExitCode> {
    let store = Store::new(Store::default_root());
    println!("{:<48} DIGEST", "REFERENCE");
    for (key, digest) in store.list()? {
        println!("{key:<48} {digest}");
    }
    Ok(ExitCode::SUCCESS)
}

fn cmd_rmi(args: RmiArgs) -> Result<ExitCode> {
    let store = Store::new(Store::default_root());
    store
        .remove(&args.image)
        .with_context(|| format!("removing {}", args.image))?;
    println!("{}", args.image);
    Ok(ExitCode::SUCCESS)
}

fn cmd_supervise(paths: &Paths, id: &str) -> Result<ExitCode> {
    let exit = supervise::supervise(paths, id).with_context(|| format!("supervising {id}"))?;
    Ok(ExitCode::from(exit.to_shell_status().min(255) as u8))
}

fn require_absolute(exe: &OsString) -> Result<()> {
    if !Path::new(exe).is_absolute() {
        bail!(
            "{exe:?} must be an absolute path with --rootfs. PATH is not searched: the \
             search would have to run in the child, between clone3 and execve, where it \
             cannot allocate."
        );
    }
    Ok(())
}

/// Find the command inside the container's filesystem.
///
/// Resolved HERE, in the parent, against the rootfs on disk - never in the
/// child, where a PATH search would mean reading directories and allocating
/// in the one window that forbids both.
fn resolve_exe(cmd: &OsString, rootfs: Option<&str>) -> Result<OsString> {
    let p = Path::new(cmd);
    if p.is_absolute() {
        return Ok(cmd.clone());
    }
    let Some(root) = rootfs else {
        return Err(anyhow::anyhow!(
            "{cmd:?} must be an absolute path when there is no image to search"
        ));
    };
    for dir in [
        "/usr/local/sbin",
        "/usr/local/bin",
        "/usr/sbin",
        "/usr/bin",
        "/sbin",
        "/bin",
    ] {
        let candidate = Path::new(root).join(dir.trim_start_matches('/')).join(p);
        // `symlink_metadata`, not `is_file`: in a real image `/bin/echo` is
        // a symlink to `/bin/busybox`, and that absolute target only
        // resolves *inside* the container. Following it from out here looks
        // for the host's busybox and concludes the command does not exist.
        if candidate.symlink_metadata().is_ok() {
            return Ok(OsString::from(format!("{dir}/{}", p.display())));
        }
    }
    bail!(
        "{cmd:?} was not found in the image's PATH directories. Give an absolute path, \
         or check the image actually contains it."
    )
}

/// The bounding set to keep, as names, or `None` for the default.
fn resolve_capabilities(add: &[String], drop: &[String]) -> Result<Option<Vec<String>>> {
    if add.is_empty() && drop.is_empty() {
        return Ok(None);
    }
    let mut set = security::default_capabilities();
    for name in drop {
        if name.eq_ignore_ascii_case("all") {
            set = security::CapabilitySet::empty();
            continue;
        }
        let c = security::capability_by_name(name)
            .with_context(|| format!("--cap-drop: unknown capability {name:?}"))?;
        set &= !c;
    }
    for name in add {
        if name.eq_ignore_ascii_case("all") {
            bail!(
                "--cap-add ALL is not supported. It would hand the container the whole \
                 bounding set including CAP_SYS_ADMIN, which is the capability masked \
                 paths and the seccomp mount rules exist to assume absent - the result \
                 looks confined and is not."
            );
        }
        let c = security::capability_by_name(name)
            .with_context(|| format!("--cap-add: unknown capability {name:?}"))?;
        set |= c;
    }
    Ok(Some(
        set.iter()
            .map(|c| {
                format!("{c:?}")
                    .trim_start_matches("CapabilitySet(")
                    .trim_end_matches(')')
                    .to_owned()
            })
            .collect(),
    ))
}

/// `(profile, unconfined)`: `(None, false)` is the default profile,
/// `(_, true)` installs no filter, `(Some(json), false)` uses that profile.
fn resolve_seccomp(opts: &[String]) -> Result<(Option<serde_json::Value>, bool)> {
    let mut out = None;
    let mut unconfined = false;
    for opt in opts {
        let Some(v) = opt.strip_prefix("seccomp=") else {
            bail!(
                "--security-opt {opt:?} is not supported. This stage understands \
                 seccomp=<file.json> and seccomp=unconfined. AppArmor, SELinux and \
                 no-new-privileges are not implemented."
            );
        };
        if v == "unconfined" {
            unconfined = true;
            continue;
        }
        let text = std::fs::read_to_string(v)
            .with_context(|| format!("reading the seccomp profile {v:?}"))?;
        out = Some(
            serde_json::from_str(&text)
                .with_context(|| format!("parsing the seccomp profile {v:?}"))?,
        );
    }
    Ok((out, unconfined))
}

/// `8080:80`, `127.0.0.1:8080:80`.
///
/// The host address defaults to `0.0.0.0`, which is what `-p 8080:80`
/// means everywhere and is worth being explicit about: it publishes to
/// every interface the machine has, not just loopback.
fn parse_publish(spec: &str, container_ip: std::net::Ipv4Addr) -> Result<Publish> {
    let parts: Vec<&str> = spec.split(':').collect();
    let (host_ip, host_s, cont_s) = match parts.as_slice() {
        [h, c] => (std::net::Ipv4Addr::UNSPECIFIED, *h, *c),
        [ip, h, c] => (
            ip.parse()
                .with_context(|| format!("-p {spec:?}: {ip:?} is not an IPv4 address"))?,
            *h,
            *c,
        ),
        _ => bail!(
            "-p {spec:?} should be host-port:container-port, optionally prefixed with a \
             host address. UDP and port ranges are not implemented."
        ),
    };
    let host_port: u16 = host_s
        .parse()
        .with_context(|| format!("-p {spec:?}: {host_s:?} is not a port"))?;
    let container_port: u16 = cont_s
        .split('/')
        .next()
        .unwrap_or(cont_s)
        .parse()
        .with_context(|| format!("-p {spec:?}: {cont_s:?} is not a port"))?;
    if cont_s.contains('/') && !cont_s.ends_with("/tcp") {
        bail!("-p {spec:?}: only tcp is implemented");
    }
    if host_port == 0 || container_port == 0 {
        bail!("-p {spec:?}: port 0 means \"any port\", which is not what you meant here");
    }
    Ok(Publish {
        host_ip,
        host_port,
        container_ip,
        container_port,
    })
}

/// `512m`, `2g`, `1048576`. Binary multiples, as every container runtime
/// means them.
fn parse_size(s: &str) -> Result<u64> {
    let s = s.trim();
    let (digits, mult) = match s.chars().last() {
        Some('k') | Some('K') => (&s[..s.len() - 1], 1024u64),
        Some('m') | Some('M') => (&s[..s.len() - 1], 1024 * 1024),
        Some('g') | Some('G') => (&s[..s.len() - 1], 1024 * 1024 * 1024),
        Some('b') | Some('B') => (&s[..s.len() - 1], 1),
        _ => (s, 1),
    };
    let n: u64 = digits
        .trim()
        .parse()
        .with_context(|| format!("{s:?} is not a size; try 512m, 2g, or a number of bytes"))?;
    n.checked_mul(mult)
        .with_context(|| format!("{s:?} overflows"))
}

// ---------------------------------------------------------------------------

fn paths_root(paths: &Paths) -> PathBuf {
    paths.containers().parent().unwrap().to_path_buf()
}

/// 12 hex characters from the kernel's RNG.
fn new_id() -> Result<String> {
    let mut b = [0u8; 6];
    std::fs::File::open("/dev/urandom")
        .context("opening /dev/urandom")?
        .read_exact(&mut b)?;
    Ok(b.iter().map(|x| format!("{x:02x}")).collect())
}
