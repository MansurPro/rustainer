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

mod stack;

use anyhow::{Context, Result, bail};
use clap::{Args, Parser, Subcommand};
use rustainer_core::exec;
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
    /// Run a command inside a running container.
    Exec(ExecArgs),
    /// Download an image and extract it into the local store.
    Pull(PullArgs),
    /// List images in the local store.
    Images,
    /// Remove an image from the local store.
    Rmi(RmiArgs),
    /// List or remove named volumes.
    #[command(subcommand)]
    Volume(VolumeCmd),
    /// Show what the engine has done: starts, exits, restarts, pulls.
    Events(EventsArgs),
    /// Serve the local web UI and HTTP API.
    Serve(ServeArgs),
    /// Start, stop and inspect a stack of containers described in a file.
    #[command(subcommand)]
    Stack(StackCmd),
    /// Internal: become the supervisor for an already-prepared container.
    #[command(hide = true)]
    Supervise { id: String },
}

// `Default` so a stack can build one field by field. clap fills every
// field from the command line in the normal path; the derived default is
// only ever the starting point for the stack path, where the values come
// from a file instead.
#[derive(Args, Default)]
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

    /// Working directory inside the container. Overrides the image's
    /// `WorkingDir`.
    #[arg(short = 'w', long)]
    workdir: Option<String>,

    /// Make a host directory or a named volume visible in the container:
    /// `-v /host/path:/in/container[:ro]` or `-v name:/in/container[:ro]`.
    /// Repeatable.
    #[arg(short = 'v', long = "volume")]
    volumes: Vec<String>,

    /// What to do when the container exits: `no` (the default), `always`,
    /// `unless-stopped`, or `on-failure[:attempts]`.
    #[arg(long, default_value = "no")]
    restart: String,

    /// Give the container a pseudo-terminal. Programs ask `isatty(1)` and
    /// behave differently: a shell prints a prompt, `^C` arrives as a
    /// signal rather than as the byte 0x03, and `ls` colours its output.
    #[arg(short = 't', long)]
    tty: bool,

    /// Keep stdin open and connected. With `-t` this is an interactive
    /// session; `-it` together is the usual spelling.
    #[arg(short = 'i', long)]
    interactive: bool,

    /// Set an environment entry, `KEY=VALUE`. Replaces the image's own
    /// entry for that key rather than shadowing it. Repeatable.
    #[arg(short = 'e', long = "env")]
    env: Vec<String>,

    /// Add an `/etc/hosts` entry: `--add-host name:address`. Repeatable.
    /// This is how a stack's services find each other.
    #[arg(long = "add-host")]
    add_host: Vec<String>,

    /// Use this address instead of allocating one. Only with
    /// `--network bridge`; the caller is responsible for it being free.
    #[arg(long)]
    ip: Option<String>,

    /// Record that this container belongs to a stack. Set by
    /// `stack up`; there is no reason to type it.
    #[arg(long, hide = true)]
    stack_name: Option<String>,

    /// The service name within that stack.
    #[arg(long, hide = true)]
    service_name: Option<String>,

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

#[derive(Subcommand)]
enum StackCmd {
    /// Start every service that is not already running, in dependency order.
    Up(StackArgs),
    /// Stop and remove the stack's containers, in reverse order.
    Down(StackArgs),
    /// Show the stack's containers.
    Ps(StackArgs),
}

#[derive(Args)]
struct StackArgs {
    /// The stack file. `rustainer.toml` in the current directory by default.
    #[arg(short = 'f', long, default_value = "rustainer.toml")]
    file: PathBuf,
}

#[derive(Args)]
struct EventsArgs {
    /// Only events from the last N seconds.
    #[arg(long, default_value_t = 3600)]
    since: u64,

    /// At most this many, newest last.
    #[arg(long, default_value_t = 200)]
    limit: usize,

    /// Keep printing as they happen.
    #[arg(short, long)]
    follow: bool,
}

#[derive(Args)]
struct ServeArgs {
    /// Address to listen on. Loopback by default, and for a reason: there
    /// is no authentication, and anything that can reach this port can
    /// start a container, which is a path to root on the host.
    #[arg(long, default_value = "127.0.0.1:8750")]
    addr: String,

    /// Print the URL and exit without serving. For scripts and tests.
    #[arg(long, hide = true)]
    check: bool,
}

#[derive(Subcommand)]
enum VolumeCmd {
    /// List named volumes.
    Ls,
    /// Remove a named volume and everything in it.
    Rm { name: String },
}

#[derive(Args)]
struct ExecArgs {
    /// Working directory inside the container. Defaults to the container's.
    #[arg(short = 'w', long)]
    workdir: Option<String>,

    /// Extra environment entry, `KEY=VALUE`. Wins over the container's own.
    #[arg(short = 'e', long = "env")]
    env: Vec<String>,

    /// Run as `uid[:gid]`. Defaults to the user the container runs as.
    #[arg(short = 'u', long)]
    user: Option<String>,

    /// Give the command a pseudo-terminal.
    #[arg(short = 't', long)]
    tty: bool,

    /// Keep stdin open. `-it` together is the usual spelling.
    #[arg(short = 'i', long)]
    interactive: bool,

    id: String,

    /// The command, and its arguments. Searched in the container's PATH
    /// directories when it is not an absolute path.
    #[arg(trailing_var_arg = true, required = true)]
    command: Vec<String>,
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
    // Kept, because `serve` has to pass it on to every CLI it executes.
    let cli_root = cli.root.clone();
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
        Command::Exec(a) => cmd_exec(&paths, a),
        Command::Pull(a) => cmd_pull(a),
        Command::Images => cmd_images(),
        Command::Rmi(a) => cmd_rmi(a),
        Command::Volume(a) => cmd_volume(a),
        Command::Events(a) => cmd_events(&paths, a),
        Command::Serve(a) => cmd_serve(cli_root.clone(), a),
        Command::Stack(a) => cmd_stack(&paths, a),
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
            // The store first, the registry only if it has to be. `pull`
            // begins by fetching the manifest, so going through it for an
            // image already on disk cost four TLS round-trips before
            // anything was cloned - 870 of the 890 milliseconds a
            // container start took - and made a fully cached image
            // unrunnable with no network at all. This is Docker's
            // `--pull=missing`, which is Docker's default; `--pull` still
            // means "go and check".
            let image = match store.local(&reference) {
                Some(i) if !args.pull => i,
                _ => store
                    .pull(&reference, args.pull)
                    .with_context(|| format!("pulling {reference}"))?,
            };

            // Say what the extractor refused. A device node in a layer is
            // not a corrupt image - it is a layer asking for a direct path
            // to host memory - and silently dropping it would hide that.
            for d in &image.report.devices_skipped {
                eprintln!("rustainer: refused a device node in the image: {d}");
            }
            warn_about_whiteouts(&image.report);
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
    // EVERY layer, topmost first - the order the overlay itself would
    // resolve in. Searching only the top layer is a guess that is wrong for
    // most real images: `golang:1.24-alpine` is four layers and `/bin/pwd`
    // is in the alpine base at the bottom, so a top-layer-only search
    // reports that the image does not contain a program it plainly does.
    let search: Vec<String> = if layers.is_empty() {
        rootfs.clone().into_iter().collect()
    } else {
        layers.iter().rev().cloned().collect()
    };

    // The image's own PATH, not ours. `golang` puts the Go toolchain in
    // /usr/local/go/bin and says so in its config; an engine that searches
    // a hardcoded list cannot find `go` in the Go image.
    let image_path = image_env
        .iter()
        .find_map(|e| e.strip_prefix("PATH="))
        .map(|p| p.to_owned());
    let exe = resolve_exe(&command[0], &search, image_path.as_deref())?;

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
    let lease = match (bridge, &args.ip) {
        (false, Some(ip)) => bail!(
            "--ip {ip} needs --network bridge: with `none` there is no interface to \
             put an address on, and with `host` the address is the host's."
        ),
        (false, None) => None,
        // Given rather than allocated: a stack has to know every service's
        // address before it starts the first one, so it can write them
        // all into every container's /etc/hosts.
        (true, Some(ip)) => Some(rustainer_core::net::Lease {
            ip: ip
                .parse()
                .with_context(|| format!("--ip {ip:?} is not an IPv4 address"))?,
            prefix: rustainer_core::net::SUBNET_PREFIX,
            gateway: rustainer_core::net::GATEWAY,
        }),
        (true, None) => Some(
            rustainer_core::net::allocate(&paths_root(paths))
                .context("allocating an address for the container")?,
        ),
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
    let volumes = args
        .volumes
        .iter()
        .map(|v| parse_volume(v))
        .collect::<Result<Vec<_>>>()?;

    let (restart_policy, restart_max) = supervise::RestartPolicy::parse(&args.restart)
        .map_err(|e| anyhow::anyhow!("--restart: {e}"))?;

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
        envp: {
            let base = if image_env.is_empty() {
                vec![format!("PATH={DEFAULT_PATH}"), "HOME=/root".to_owned()]
            } else {
                image_env
            };
            // `--env` REPLACES the image's entry for a key rather than
            // being prepended to shadow it. Prepending also "works" -
            // getenv returns the first match - but only by accident of
            // glibc, and `env` inside the container would print the
            // variable twice, which is how an operator concludes the
            // engine is broken. Same function `exec` uses, so the two
            // cannot drift apart.
            rustainer_core::exec::merge_env(&base, &args.env)
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
        // The flag wins over the image's `WorkingDir`.
        workdir: args.workdir.clone().or(workdir),
        layers: layers.clone(),
        read_only: args.read_only,
        // Beside the layers, on whatever filesystem the image store is on.
        // The runtime state directory is usually a tmpfs and sometimes an
        // overlayfs, and overlayfs cannot host an upperdir at all.
        private_net,
        bridge,
        userns,
        tty: args.tty,
        restart: restart_policy,
        restart_max,
        volumes,
        stack: args.stack_name.clone(),
        service: args.service_name.clone(),
        peers: args
            .add_host
            .iter()
            .map(|h| parse_add_host(h))
            .collect::<Result<Vec<_>>>()?,
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
                let spec = UserSpec::parse_with_passwd(
                    u,
                    &read_from_layers(&search, "etc/passwd"),
                    &read_from_layers(&search, "etc/group"),
                )
                .map_err(|e| anyhow::anyhow!("--user: {e}"))?;
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
    let interactive = !args.detach && (args.tty || args.interactive);
    let mut child = spawn_supervisor(paths, &id, handshake_tx, args.detach, interactive)?;
    let init_pid = read_handshake(paths, &handshake_rx, &mut child, &id)?;

    // Release before doing anything slow: `stop` and `rm` on this container
    // must not be blocked by a foreground `run` that is merely waiting.
    drop(lock);

    rustainer_core::events::record(
        paths,
        "create",
        &id,
        serde_json::json!({
            "image": config.rootfs.clone().unwrap_or_default(),
            "command": config.argv.join(" "),
            "detached": args.detach,
        }),
    );

    if args.detach {
        // Reap the supervisor's *launcher* relationship: it is our child
        // only until we exit, after which init takes it over. Nothing to
        // wait for.
        println!("{id}");
        return Ok(ExitCode::SUCCESS);
    }

    // Raw mode here, in the CLI, because the CLI owns the terminal: no
    // line buffering so a keystroke arrives at once, no echo so the
    // container's is the only one, and no signal generation so `^C`
    // reaches the container as a byte and becomes SIGINT *there*.
    //
    // The guard restores it on every path out, including a panic, which
    // is the whole reason it is a guard: a shell left in raw mode is a
    // shell the user has to `reset` blind.
    let _raw = if interactive && args.tty {
        rustainer_core::pty::RawMode::enter(0)
    } else {
        None
    };
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
    interactive: bool,
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

    // The supervisor relays the pty, so for an interactive run it needs
    // our terminal - all three descriptors, not just the two it normally
    // tees onto. Everything else gets /dev/null, because a container is
    // not entitled to read the operator's keyboard unless asked.
    cmd.stdin(if interactive {
        Stdio::inherit()
    } else {
        Stdio::null()
    });
    if detach {
        // Detached: the container's output goes to the log files only -
        // but the SUPERVISOR's own stderr is kept, in a file of its own.
        //
        // It used to go to /dev/null with everything else, and the cost
        // only became obvious with a UI in front of it: a detached
        // container that fails to start showed as `unknown` with no
        // reason anywhere, while the supervisor had printed a precise
        // explanation - "cannot enable the memory controller in ... it
        // still holds ..." - into the void. The engine knew exactly what
        // was wrong and threw it away because nobody was looking.
        let log = paths.dir(id).join("supervisor.log");
        let err =
            std::fs::File::create(&log).with_context(|| format!("creating {}", log.display()))?;
        cmd.stdout(Stdio::null()).stderr(Stdio::from(err));
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

fn read_handshake(
    paths: &Paths,
    rx: &OwnedFd,
    child: &mut std::process::Child,
    id: &str,
) -> Result<i32> {
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
    // whatever it printed are the real diagnosis - so print what it said,
    // rather than only that it said nothing.
    let status = child.wait().ok();
    let said = std::fs::read_to_string(paths.dir(id).join("supervisor.log"))
        .map(|s| s.trim().to_owned())
        .unwrap_or_default();
    let exit = match status.and_then(|s| s.code()) {
        Some(c) => format!(" (exit {c})"),
        None => String::new(),
    };
    // Recorded as well as printed: `ps`, the API and the UI all read the
    // state file, and a failure nobody can see afterwards is a failure
    // that gets reported as a mystery.
    if !said.is_empty()
        && let Ok(mut st) = State::load(paths, id)
    {
        st.error = Some(said.lines().last().unwrap_or(&said).to_owned());
        let _ = st.store(paths);
    }
    if said.is_empty() {
        bail!("the supervisor for {id} exited without starting the container{exit}");
    }
    bail!("{said}")
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

fn cmd_exec(paths: &Paths, args: ExecArgs) -> Result<ExitCode> {
    let mut req = exec::Request {
        argv: args.command,
        env: args.env,
        workdir: args.workdir,
        // Passed through unparsed: a name is only meaningful against this
        // container's /etc/passwd, which exec can read and we cannot.
        user: args.user,
        stdio: None,
        tty: args.tty,
        master: None,
    };

    // The pty is allocated HERE rather than in `exec`, because the
    // terminal being attached to is this process's, and the raw-mode
    // guard below has to outlive the call.
    let _raw = if args.tty {
        let pty = rustainer_core::pty::open().context("allocating a pseudo-terminal")?;
        if let Some((rows, cols)) = rustainer_core::pty::size_of(0) {
            let _ = rustainer_core::pty::set_size(&pty.master, rows, cols);
        }
        req.stdio = Some(rustainer_core::child::Stdio {
            stdin: pty.slave.try_clone()?,
            stdout: pty.slave.try_clone()?,
            stderr: pty.slave.try_clone()?,
        });
        req.master = Some(pty.master);
        // Our own copy of the slave goes now. While any descriptor on it
        // is open the master never reports EOF, and the relay would wait
        // for an end that cannot come.
        drop(pty.slave);
        rustainer_core::pty::RawMode::enter(0)
    } else {
        None
    };
    let exit =
        exec::exec(paths, &args.id, &mut req).with_context(|| format!("exec in {}", args.id))?;
    // The exec'd command's status, not ours: `rustainer exec sh -c 'exit 7'`
    // has to be usable in a shell conditional the same way `docker exec` is.
    Ok(ExitCode::from(exit.to_shell_status().min(255) as u8))
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
    warn_about_whiteouts(&image.report);
    rustainer_core::events::record(
        &Paths::new(Paths::default_root()),
        "pull",
        &args.image,
        serde_json::json!({ "digest": image.digest }),
    );
    println!("{}", image.digest);
    Ok(ExitCode::SUCCESS)
}

fn warn_about_whiteouts(report: &rustainer_image::extract::Report) {
    if report.whiteouts_unrecordable == 0 {
        return;
    }
    // Specific about the consequence, because "whiteout" means nothing to
    // most people and the consequence means a great deal: files the image
    // author deleted are readable in the container, and `RUN ... && rm -rf`
    // is how build secrets, credentials and package caches get removed.
    eprintln!(
        "rustainer: WARNING: {} deletion(s) recorded by this image could not be \
         applied, so files its later layers deleted REMAIN VISIBLE in containers \
         made from it. This usually means an unprivileged container's device \
         filter refused to create an overlay whiteout. Extract the image on the \
         host, or in a user namespace of your own, to get this right.",
        report.whiteouts_unrecordable
    );
    if let Some(why) = &report.whiteout_failure {
        eprintln!("rustainer: the first such failure was: {why}");
    }
}

/// Where named volumes live: beside the image store, because both must
/// outlive a reboot and the runtime state directory is usually a tmpfs.
/// `name:address`, as Docker's `--add-host` takes it.
fn parse_add_host(spec: &str) -> Result<(String, String)> {
    let (name, ip) = spec
        .split_once(':')
        .ok_or_else(|| anyhow::anyhow!("--add-host {spec:?} is not `name:address`"))?;
    if name.is_empty() || name.contains(char::is_whitespace) {
        bail!("--add-host {spec:?}: {name:?} is not a usable host name");
    }
    // Parsed, not trusted: these go into a file the container reads as
    // root, and a line with a space in it is a different /etc/hosts entry
    // than the one that was meant.
    let addr: std::net::Ipv4Addr = ip
        .parse()
        .with_context(|| format!("--add-host {spec:?}: {ip:?} is not an IPv4 address"))?;
    Ok((name.to_owned(), addr.to_string()))
}

fn volumes_root() -> PathBuf {
    Store::default_root().join("volumes")
}

/// `src:dst[:ro]` — Docker's own syntax, with Docker's own rule for
/// telling a host path from a volume name.
///
/// The rule is positional, not semantic: a first field beginning with `/`
/// or `.` is a path, anything else is a name. It looks arbitrary and it is
/// the only rule that can work, because `data` is a perfectly good
/// directory name and a perfectly good volume name, and the engine has to
/// pick one without asking.
fn parse_volume(spec: &str) -> Result<rustainer_core::mount::VolumeMount> {
    let parts: Vec<&str> = spec.split(':').collect();
    let (src, dst, opts) = match parts.as_slice() {
        [s, d] => (*s, *d, ""),
        [s, d, o] => (*s, *d, *o),
        _ => bail!(
            "-v {spec:?} is not `src:dst` or `src:dst:ro`. A single path means \
             an anonymous volume, which is not implemented: name it, or give a \
             host path."
        ),
    };
    if src.is_empty() || dst.is_empty() {
        bail!("-v {spec:?} has an empty source or destination");
    }
    let read_only = match opts {
        "" | "rw" => false,
        "ro" => true,
        other => bail!(
            "-v {spec:?}: {other:?} is not a mount option. Only `ro` and `rw` are \
             supported; label and propagation options are not implemented."
        ),
    };
    if !dst.starts_with('/') {
        bail!("-v {spec:?}: the destination must be an absolute path inside the container");
    }
    if dst.trim_end_matches('/').is_empty() {
        bail!(
            "-v {spec:?}: mounting over `/` would replace the container's whole \
             root filesystem, including the one the image provides"
        );
    }
    if dst.split('/').any(|c| c == "..") {
        bail!("-v {spec:?}: the destination may not contain `..`");
    }

    let source = if src.starts_with('/') || src.starts_with('.') || src.starts_with('~') {
        let p = PathBuf::from(shellexpand_home(src));
        // Docker creates a missing host directory rather than refusing,
        // and scripts depend on it. A missing FILE is still an error: it
        // is far more likely to be a typo than an intention.
        if !p.exists() {
            std::fs::create_dir_all(&p)
                .with_context(|| format!("creating the host directory {}", p.display()))?;
        }
        p.canonicalize()
            .with_context(|| format!("resolving the host path {}", p.display()))?
    } else {
        if !src
            .chars()
            .next()
            .is_some_and(|c| c.is_ascii_alphanumeric())
            || !src
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || "_.-".contains(c))
        {
            bail!(
                "-v {spec:?}: {src:?} is neither an absolute host path nor a usable \
                 volume name. A name must start with a letter or digit and contain \
                 only letters, digits, `_`, `.` and `-`."
            );
        }
        let p = volumes_root().join(src);
        std::fs::create_dir_all(&p).with_context(|| format!("creating the volume {src:?}"))?;
        p
    };

    // Stat'd here because the child cannot: a bind over a file needs a
    // file as its mount point, and mkdir'ing one produces EISDIR from the
    // mount with nothing to say which `-v` caused it.
    let is_file = source.is_file();

    Ok(rustainer_core::mount::VolumeMount {
        source,
        target: dst.trim_end_matches('/').to_owned(),
        read_only,
        is_file,
    })
}

/// `~` and `~/...` only. Not a shell: `~user` is deliberately not resolved,
/// because guessing another user's home directory for a mount is worse
/// than refusing.
fn shellexpand_home(s: &str) -> String {
    match s.strip_prefix('~') {
        Some(rest) if rest.is_empty() || rest.starts_with('/') => match std::env::var_os("HOME") {
            Some(h) => format!("{}{rest}", h.to_string_lossy()),
            None => s.to_owned(),
        },
        _ => s.to_owned(),
    }
}

fn read_stack(path: &Path) -> Result<stack::Stack> {
    let text = std::fs::read_to_string(path).with_context(|| {
        format!(
            "reading the stack file {}. Write one, or point at it with -f.",
            path.display()
        )
    })?;
    stack::Stack::parse(&text).with_context(|| format!("in {}", path.display()))
}

fn cmd_stack(paths: &Paths, cmd: StackCmd) -> Result<ExitCode> {
    match cmd {
        StackCmd::Up(a) => stack_up(paths, &read_stack(&a.file)?),
        StackCmd::Down(a) => stack_down(paths, &read_stack(&a.file)?),
        StackCmd::Ps(a) => stack_ps(paths, &read_stack(&a.file)?),
    }
}

fn stack_up(paths: &Paths, st: &stack::Stack) -> Result<ExitCode> {
    let order = st.start_order()?;

    // Every address is allocated BEFORE anything starts, because every
    // container's /etc/hosts has to name every other one - including the
    // services that do not exist yet. A stack whose first service cannot
    // resolve its last is a stack that only works if you start it twice.
    let mut peers: Vec<(String, String)> = Vec::new();
    let mut address: std::collections::BTreeMap<String, String> = std::collections::BTreeMap::new();
    for name in &order {
        let svc = &st.services[name];
        if svc.network.as_deref().unwrap_or("bridge") != "bridge" {
            continue;
        }
        let lease = rustainer_core::net::allocate(&paths_root(paths))
            .with_context(|| format!("allocating an address for {name}"))?;
        let ip = lease.ip.to_string();
        // Both names resolve: the service's own short name, which is what
        // a config file inside the container will say, and the container
        // name, which is what `ps` shows.
        peers.push((name.clone(), ip.clone()));
        peers.push((st.container_name(name), ip.clone()));
        address.insert(name.clone(), ip);
    }

    let mut started = 0usize;
    for name in &order {
        let id = st.container_name(name);
        if let Ok(state) = State::load(paths, &id)
            && matches!(state.observed_status(), Status::Running | Status::Created)
        {
            println!("{id}  already running");
            continue;
        }
        // A previous run's remains. Removed rather than refused: `up` is
        // the command people run after `down` failed half way.
        if paths.dir(&id).exists() {
            let _ = supervise::remove(paths, &id, true);
        }

        let svc = &st.services[name];
        let mut command: Vec<OsString> = vec![svc.image.clone().into()];
        command.extend(svc.command.iter().map(OsString::from));
        let args = RunArgs {
            rootfs: None,
            pull: false,
            detach: true,
            name: Some(id.clone()),
            network: svc.network.clone().unwrap_or_else(|| "bridge".into()),
            hostname: name.clone(),
            workdir: svc.workdir.clone(),
            volumes: svc.volumes.clone(),
            add_host: peers
                .iter()
                .filter(|(n, _)| n != name && *n != id)
                .map(|(n, ip)| format!("{n}:{ip}"))
                .collect(),
            ip: address.get(name).cloned(),
            stack_name: Some(st.name.clone()),
            service_name: Some(name.clone()),
            memory: svc.memory.clone(),
            cpus: svc.cpus,
            pids_limit: svc.pids_limit,
            user: svc.user.clone(),
            env: svc.env.clone(),
            publish: svc.ports.clone(),
            command,
            ..Default::default()
        };
        // The exit code is the CLI's, and for a detached run it is always
        // success - the interesting failure comes back as an Err and is
        // propagated. Discarded explicitly so it is clear that is why.
        let _ = cmd_run(paths, args).with_context(|| format!("starting {id}"))?;
        started += 1;
    }
    println!(
        "{} service(s) started, {} already running",
        started,
        order.len() - started
    );
    Ok(ExitCode::SUCCESS)
}

fn stack_down(paths: &Paths, st: &stack::Stack) -> Result<ExitCode> {
    // Reverse of the start order: a service is stopped before the things
    // it depends on, which is the only order in which shutting down looks
    // like anything other than a cascade of connection errors.
    let mut order = st.start_order()?;
    order.reverse();
    for name in order {
        let id = st.container_name(&name);
        if !paths.dir(&id).exists() {
            continue;
        }
        let _ = supervise::stop(paths, &id, Duration::from_secs(5));
        match supervise::remove(paths, &id, true) {
            Ok(()) => println!("{id}  removed"),
            Err(e) => eprintln!("rustainer: {id}: {e}"),
        }
    }
    Ok(ExitCode::SUCCESS)
}

fn stack_ps(paths: &Paths, st: &stack::Stack) -> Result<ExitCode> {
    println!(
        "{:<24} {:<9} {:<8} {:<15} COMMAND",
        "SERVICE", "STATUS", "PID", "ADDRESS"
    );
    for name in st.start_order()? {
        let id = st.container_name(&name);
        let (status, pid, addr, cmd) = match State::load(paths, &id) {
            Err(_) => (
                "-".to_owned(),
                "-".to_owned(),
                "-".to_owned(),
                String::new(),
            ),
            Ok(state) => {
                let observed = state.observed_status();
                let cfg = Config::load(paths, &id).ok();
                (
                    observed.to_string(),
                    match (observed, state.init_pid) {
                        (Status::Running | Status::Created, Some(p)) => p.to_string(),
                        _ => "-".into(),
                    },
                    cfg.as_ref()
                        .and_then(|c| c.ip.clone())
                        .unwrap_or_else(|| "-".into()),
                    state.command.join(" "),
                )
            }
        };
        println!("{name:<24} {status:<9} {pid:<8} {addr:<15} {cmd}");
    }
    Ok(ExitCode::SUCCESS)
}

fn cmd_events(paths: &Paths, args: EventsArgs) -> Result<ExitCode> {
    use rustainer_core::events;
    let now = rustainer_core::state::now();
    let mut since = now.saturating_sub(args.since);
    let mut seen = 0usize;
    loop {
        let batch = events::read(paths, since, args.limit);
        for e in &batch {
            let at = e["at"].as_u64().unwrap_or(0);
            // Printed as a local clock time rather than a unix second:
            // the reader is looking for "what happened just now", and
            // 1794678683 answers a different question.
            let hhmmss = {
                let secs = at % 86400;
                format!(
                    "{:02}:{:02}:{:02}",
                    secs / 3600,
                    (secs % 3600) / 60,
                    secs % 60
                )
            };
            println!(
                "{hhmmss}  {:<10} {:<14} {}",
                e["kind"].as_str().unwrap_or("?"),
                e["id"].as_str().unwrap_or("-"),
                e["detail"]
            );
            since = at.max(since);
            seen += 1;
        }
        if !args.follow {
            break;
        }
        // Polling, not inotify: the file is appended to by processes that
        // come and go, and a 400ms poll on a small file costs nothing
        // next to the complexity of watching it properly.
        std::thread::sleep(Duration::from_millis(400));
        // `since` is a whole second, so events written later in the same
        // second would repeat. Step past it once we have seen anything.
        if seen > 0 {
            since += 1;
            seen = 0;
        }
    }
    Ok(ExitCode::SUCCESS)
}

fn cmd_serve(root: Option<PathBuf>, args: ServeArgs) -> Result<ExitCode> {
    use std::net::{TcpListener, ToSocketAddrs};

    let paths = Paths::new(root.clone().unwrap_or_else(Paths::default_root));
    paths
        .ensure_root()
        .context("preparing the state directory")?;

    let addr = args
        .addr
        .to_socket_addrs()
        .with_context(|| format!("resolving {:?}", args.addr))?
        .next()
        .ok_or_else(|| anyhow::anyhow!("{:?} resolved to no address", args.addr))?;

    // Said plainly, once, and not buried in a man page. An API with no
    // authentication on a routable address is a root shell with extra
    // steps, and the person who binds 0.0.0.0 to "check it from my
    // laptop" is exactly the person who needs to be told.
    if !addr.ip().is_loopback() {
        eprintln!(
            "rustainer: WARNING: {addr} is not loopback and this API has NO \
             AUTHENTICATION. Anything that can reach this port can start a \
             container, and a container is a path to root on this host. Put it \
             behind something, or bind 127.0.0.1 and use an SSH tunnel."
        );
    }

    let listener = TcpListener::bind(addr).with_context(|| format!("binding {addr}"))?;
    let local = listener.local_addr().unwrap_or(addr);
    println!("rustainer: serving http://{local}/");
    if args.check {
        return Ok(ExitCode::SUCCESS);
    }
    rustainer_api::Server::new(paths, root.map(|r| r.to_string_lossy().into_owned()))
        .serve(listener)
        .context("serving")?;
    Ok(ExitCode::SUCCESS)
}

fn cmd_volume(cmd: VolumeCmd) -> Result<ExitCode> {
    let root = volumes_root();
    match cmd {
        VolumeCmd::Ls => {
            println!("{:<32} {:>10}  PATH", "VOLUME", "ENTRIES");
            let Ok(rd) = std::fs::read_dir(&root) else {
                return Ok(ExitCode::SUCCESS);
            };
            let mut rows: Vec<(String, usize, String)> = Vec::new();
            for e in rd.flatten() {
                if !e.path().is_dir() {
                    continue;
                }
                let n = std::fs::read_dir(e.path()).map(|d| d.count()).unwrap_or(0);
                rows.push((
                    e.file_name().to_string_lossy().into_owned(),
                    n,
                    e.path().display().to_string(),
                ));
            }
            rows.sort();
            for (name, n, path) in rows {
                println!("{name:<32} {n:>10}  {path}");
            }
        }
        VolumeCmd::Rm { name } => {
            // Resolved through the volumes root rather than joined blindly:
            // `volume rm ../../etc` must not delete anything.
            if name.contains('/') || name.contains("..") || name.is_empty() {
                bail!("{name:?} is not a volume name");
            }
            let p = root.join(&name);
            if !p.is_dir() {
                bail!("no such volume: {name}");
            }
            std::fs::remove_dir_all(&p).with_context(|| format!("removing the volume {name}"))?;
            println!("{name}");
        }
    }
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

/// The PATH searched when the image does not declare one.
const DEFAULT_PATH: &str = "/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin";

/// Find the command inside the container's filesystem.
///
/// Resolved HERE, in the parent, against the layers on disk - never in the
/// child, where a PATH search would mean reading directories and allocating
/// in the one window that forbids both.
///
/// `roots` is every layer, topmost first, which is the order the overlay
/// will resolve in once it exists. One caveat, stated rather than hidden: a
/// binary deleted by an upper layer leaves an overlay whiteout that this
/// search does not interpret, so it can still be found here. An image whose
/// own command is whited out is pathological, and the alternative - building
/// the overlay before we know what to run - is a worse trade.
fn resolve_exe(cmd: &OsString, roots: &[String], path_env: Option<&str>) -> Result<OsString> {
    let p = Path::new(cmd);
    if p.is_absolute() {
        return Ok(cmd.clone());
    }
    if roots.is_empty() {
        return Err(anyhow::anyhow!(
            "{cmd:?} must be an absolute path when there is no image to search"
        ));
    }
    let path = path_env.unwrap_or(DEFAULT_PATH);
    // PATH order outermost: the first directory on PATH that has the
    // command wins, and within that directory the topmost layer wins. That
    // is `execvp` composed with overlayfs, in that order, and swapping them
    // would pick a shadowed binary from a lower layer.
    for dir in path.split(':').filter(|d| !d.is_empty()) {
        for root in roots {
            let candidate = Path::new(root).join(dir.trim_start_matches('/')).join(p);
            // `symlink_metadata`, not `is_file`: in a real image `/bin/echo`
            // is a symlink to `/bin/busybox`, and that absolute target only
            // resolves *inside* the container. Following it from out here
            // looks for the host's busybox and concludes the command does
            // not exist.
            if candidate.symlink_metadata().is_ok() {
                return Ok(OsString::from(format!(
                    "{}/{}",
                    dir.trim_end_matches('/'),
                    p.display()
                )));
            }
        }
    }
    bail!(
        "{cmd:?} was not found in {path}, which is the PATH this image declares. Give an \
         absolute path, or check the image actually contains it."
    )
}

/// Read a file out of the container's filesystem, topmost layer first.
///
/// The same resolution order as [`resolve_exe`], for the same reason, and
/// used for the two files that decide who the container runs as.
fn read_from_layers(roots: &[String], rel: &str) -> String {
    roots
        .iter()
        .find_map(|r| std::fs::read_to_string(Path::new(r).join(rel)).ok())
        .unwrap_or_default()
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
