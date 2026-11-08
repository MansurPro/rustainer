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
use rustainer_core::state::{Paths, State, Status};
use rustainer_core::supervise::{self, Config, HANDSHAKE_FD, HANDSHAKE_LEN};
use rustainer_core::{Privileges, mount, sys};

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
    Run(RunArgs),
    /// List containers.
    Ps(PsArgs),
    /// Stop a running container: SIGTERM, a grace period, then SIGKILL.
    Stop(StopArgs),
    /// Remove a container's state directory.
    Rm(RmArgs),
    /// Print a container's captured output.
    Logs(LogsArgs),
    /// Internal: become the supervisor for an already-prepared container.
    #[command(hide = true)]
    Supervise { id: String },
}

#[derive(Args)]
struct RunArgs {
    /// Root filesystem: a directory tree, for example one extracted from an
    /// image tarball with `tar`. `/` keeps the host's filesystem view.
    #[arg(long, default_value = "/")]
    rootfs: PathBuf,

    /// Network mode. Only `host` is supported, and it is required
    /// explicitly so that sharing the host's network stack is a decision
    /// rather than a default.
    #[arg(long)]
    network: String,

    /// Hostname inside the container's UTS namespace.
    #[arg(long, default_value = "container")]
    hostname: String,

    /// Start the container and return immediately, printing its id.
    #[arg(short, long)]
    detach: bool,

    /// Name the container instead of generating an id.
    #[arg(long)]
    name: Option<String>,

    /// The command to run, resolved inside the container's root filesystem.
    #[arg(required = true, num_args = 1.., trailing_var_arg = true)]
    command: Vec<OsString>,
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
        Command::Run(a) => cmd_run(&paths, a),
        Command::Ps(a) => cmd_ps(&paths, a),
        Command::Stop(a) => cmd_stop(&paths, a),
        Command::Rm(a) => cmd_rm(&paths, a),
        Command::Logs(a) => cmd_logs(&paths, a),
        Command::Supervise { id } => cmd_supervise(&paths, &id),
    }
}

// ---------------------------------------------------------------------------
// run
// ---------------------------------------------------------------------------

fn cmd_run(paths: &Paths, args: RunArgs) -> Result<ExitCode> {
    if args.network != "host" {
        bail!(
            "--network {} is not supported yet. This stage shares the host's network \
             namespace, and the flag is mandatory so that is visible. A private network \
             stack is stage 8 and bridge networking is stage 9. Pass --network host .",
            args.network
        );
    }

    let exe = args.command[0].clone();
    if !Path::new(&exe).is_absolute() {
        bail!(
            "{exe:?} must be an absolute path. PATH is not searched: the search would \
             have to run in the child, between clone3 and execve, where it cannot \
             allocate."
        );
    }

    let rootfs = if args.rootfs == Path::new("/") {
        None
    } else {
        if !args.rootfs.is_dir() {
            bail!(
                "--rootfs {:?} is not a directory. It should be an extracted root \
                 filesystem - for example `mkdir rootfs && tar -xf alpine.tar -C rootfs`. \
                 Pulling an image is stage 6.",
                args.rootfs
            );
        }
        let privs = Privileges::probe();
        if !privs.sys_admin {
            bail!(
                "a private root filesystem needs CAP_SYS_ADMIN in the user namespace that \
                 owns this mount namespace, and this process does not have it. Either run \
                 as root, or re-run inside `unshare --user --map-root-user`."
            );
        }
        // Planned here only to report degradations while someone is still
        // watching; the supervisor plans it again for real.
        let plan = mount::plan(&args.rootfs, privs).context("planning the container's mounts")?;
        for d in &plan.degradations {
            eprintln!("rustainer: degraded: {} - {}", d.what, d.because);
        }
        Some(args.rootfs.canonicalize()?.to_string_lossy().into_owned())
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

    let config = Config {
        id: id.clone(),
        exe: exe.to_string_lossy().into_owned(),
        argv: args
            .command
            .iter()
            .map(|a| a.to_string_lossy().into_owned())
            .collect(),
        envp: std::env::vars().map(|(k, v)| format!("{k}={v}")).collect(),
        hostname: args.hostname,
        rootfs,
    };
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
        let exit = match (observed, st.exit_code, st.signal) {
            (Status::Exited, Some(c), _) => c.to_string(),
            (Status::Exited, None, Some(s)) => format!("sig{s}"),
            (Status::Unknown, _, _) => "?".into(),
            _ => "-".into(),
        };
        println!(
            "{:<14} {:<9} {:<8} {:<8} {}",
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

fn cmd_supervise(paths: &Paths, id: &str) -> Result<ExitCode> {
    let exit = supervise::supervise(paths, id).with_context(|| format!("supervising {id}"))?;
    Ok(ExitCode::from(exit.to_shell_status().min(255) as u8))
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
