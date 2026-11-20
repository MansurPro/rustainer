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

mod build;
mod dockerfile;
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
    /// Start an exited container again, exactly as it was configured.
    ///
    /// Same id, address, volumes and limits, and the same writable
    /// layer: files the container wrote before it exited are still
    /// there. It is the container resumed, not a new one like it.
    Start { id: String },
    /// Stop a container if it is running, then start it.
    Restart(StopArgs),
    /// Remove a container's state directory.
    Rm(RmArgs),
    /// Print a container's captured output.
    Logs(LogsArgs),
    /// Run a command inside a running container.
    Exec(ExecArgs),
    /// Build an image from a Dockerfile.
    Build(BuildArgs),
    /// Download an image and extract it into the local store.
    Pull(PullArgs),
    /// Upload a local image to a registry.
    Push(PushArgs),
    /// List images in the local store.
    Images,
    /// Look up an image's installed packages in the OSV vulnerability
    /// database.
    ///
    /// Sends the **names and versions** of the packages to api.osv.dev,
    /// and nothing else. Reads the distribution's package database, so
    /// it sees `apk` and `dpkg` packages and not application
    /// dependencies or anything RPM-based.
    Scan {
        /// An image already in the local store.
        image: String,
        /// Print the result as JSON.
        #[arg(long)]
        json: bool,
    },
    /// A single-node Kubernetes cluster, in one container.
    ///
    /// k3s, running in a `--privileged` container of this engine: the
    /// way Docker Desktop's own Kubernetes is a node in a container.
    /// The cluster's data is a named volume, so `down` and `up` again
    /// is the same cluster. Needs root and a Linux host.
    #[command(subcommand)]
    Kube(KubeCmd),
    /// Language models: pull one from a registry, serve it, ask it.
    ///
    /// Docker's `ai/` namespace on Docker Hub holds models as OCI
    /// artifacts whose layer is a GGUF file. `model pull` downloads
    /// that file; `model run` starts a llama.cpp server container with
    /// it mounted read-only and an OpenAI-compatible API on a local
    /// port.
    #[command(subcommand)]
    Model(ModelCmd),
    /// Add something to the page: a stack plus the address of its UI.
    ///
    /// An extension here is a directory with two files - a stack file,
    /// which says what to run, and `extension.toml`, which says what it
    /// is for and where its page is. Enabling one is `stack up`;
    /// the page then shows its UI in a tab. That is the whole format.
    /// It is NOT Docker's extension format and cannot load Docker's
    /// extensions.
    #[command(subcommand)]
    Extension(ExtensionCmd),
    /// Find MCP servers and say how to plug one into a client.
    ///
    /// An MCP server packaged as an image is a program that speaks a
    /// protocol on stdin and stdout, so running one is `run -i --rm
    /// <image>` and nothing more; what is worth having is the catalogue
    /// and the exact snippet a client's configuration wants.
    #[command(subcommand)]
    Mcp(McpCmd),
    /// List the public repositories of a Docker Hub namespace: a user,
    /// an organisation, or `library` for the official images.
    ///
    /// Docker Hub only - it is Hub's own API, not part of the registry
    /// protocol. Anonymous by default: public repositories, the hundred
    /// most pulled. With RUSTAINER_REGISTRY_AUTH=user:token it signs in:
    /// private repositories are listed too, all of them, and the
    /// namespace defaults to the user.
    Repos { namespace: Option<String> },
    /// List the tags a repository has in its registry.
    Tags {
        /// A repository, e.g. `alpine` or `ghcr.io/owner/name`.
        repository: String,
    },
    /// Remove an image from the local store.
    Rmi(RmiArgs),
    /// List or remove named volumes.
    #[command(subcommand)]
    Volume(VolumeCmd),
    /// Create, list or remove user-defined networks.
    #[command(subcommand)]
    Network(NetworkCmd),
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

    /// Internal: forward a host port to a container, in a process of its
    /// own.
    ///
    /// A process rather than a thread in the supervisor, because the
    /// supervisor has to be able to call `clone3` and `clone3` refuses a
    /// process that has ever had a second thread.
    #[command(hide = true)]
    Forward {
        /// An already-bound listening socket, inherited across the exec.
        #[arg(long)]
        fd: i32,
        #[arg(long)]
        to: String,
    },

    /// Internal: write stdin to a file in a running container, or in a
    /// volume with `--volume`. What the page's editor and upload call.
    #[command(hide = true)]
    Put {
        #[arg(long)]
        volume: bool,
        /// A container id, or a volume name.
        target: String,
        /// Where in it, as an absolute path.
        path: String,
    },

    /// Internal: keep running a container's healthcheck and record what
    /// it says.
    ///
    /// A process of its own, started detached by `stack up`, and NOT
    /// part of the supervisor. The supervisor has to be able to call
    /// `clone3` to restart its container, `clone3` refuses a process
    /// that has ever had a second thread, and a probe loop inside it
    /// would be one more reason to be careful forever. This writes a
    /// `health` file beside the container and exits when the container
    /// does; nothing depends on it being alive.
    #[command(hide = true)]
    HealthWatch {
        id: String,
        #[arg(long)]
        interval: u64,
        #[arg(long)]
        retries: u64,
        /// The probe, after `--`.
        ///
        /// Trailing and hyphen-tolerant, because the overwhelmingly
        /// common healthcheck is `/bin/sh -c '...'` and a repeated
        /// `--test` option would read that `-c` as one of its own
        /// flags. Found by running it rather than by reading it.
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        test: Vec<String>,
    },
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

    /// Run another platform's build of the image, e.g. `linux/amd64` on
    /// an arm64 machine. Needs an emulator registered with the kernel.
    #[arg(long)]
    platform: Option<String>,

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

    /// Remove the container when it exits. Foreground runs only: a
    /// detached container has nobody waiting to do the removing.
    #[arg(long)]
    rm: bool,

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

    /// Replace the image's ENTRYPOINT. An empty string removes it, so
    /// that what follows the image name is the whole command.
    #[arg(long)]
    entrypoint: Option<String>,

    /// Run without confinement: every capability, no syscall filter,
    /// nothing masked, `/sys` and the container's cgroup writable, and
    /// the host's kmsg, fuse, loop and tun devices.
    ///
    /// For a container that has to be a container engine or a cluster
    /// node itself. A process in one is root on this machine in every
    /// way that matters; do not use it for anything you would not run
    /// as root directly.
    #[arg(long)]
    privileged: bool,

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
    /// Another platform's build, e.g. `linux/amd64`.
    #[arg(long)]
    platform: Option<String>,
}

#[derive(Args)]
struct RmiArgs {
    image: String,
}

#[derive(Subcommand)]
enum KubeCmd {
    /// Start the cluster, or start it again.
    Up {
        /// The k3s image. Pinned by default so that `up` means the
        /// same cluster next month.
        #[arg(long, default_value = "rancher/k3s:v1.31.4-k3s1")]
        image: String,
    },
    /// Stop the cluster's container. Its data volume is kept.
    Down,
    /// Run kubectl against the cluster: `rustainer kube kubectl get pods -A`.
    Kubectl {
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },
    /// Print a kubeconfig for the cluster, for kubectl on this machine.
    Config,
}

#[derive(Subcommand)]
enum ModelCmd {
    /// Download a model: `rustainer model pull ai/smollm2:360M-Q4_K_M`.
    Pull { model: String },
    /// The models on disk, and whether each is being served.
    Ls,
    /// Delete a model's file (and stop its server).
    Rm { model: String },
    /// Serve a model. Pulls it first if it is not here.
    Run {
        model: String,
        /// The local port for the OpenAI-compatible API. Docker's own
        /// model runner uses this one, so clients set up for it work.
        #[arg(long, default_value_t = 12434)]
        port: u16,
        /// The server. Any image whose entrypoint is llama.cpp's
        /// `llama-server` will do, e.g. a `server-cuda` build.
        #[arg(long, default_value = "ghcr.io/ggml-org/llama.cpp:server")]
        image: String,
    },
    /// Stop serving a model. Its file is kept.
    Stop { model: String },
    /// Ask a running model one question and print the answer.
    Ask { model: String, prompt: String },
}

fn cmd_model(paths: &Paths, cmd: ModelCmd) -> Result<ExitCode> {
    use rustainer_image::model;
    let root = Store::default_root();
    let parse =
        |m: &str| rustainer_image::Reference::parse(m).map_err(|e| anyhow::anyhow!("{m}: {e}"));
    // One container per model, named after it.
    let container = |r: &rustainer_image::Reference| {
        let slug: String = format!("{}-{}", r.name, r.tag)
            .chars()
            .map(|c| {
                if c.is_ascii_alphanumeric() {
                    c.to_ascii_lowercase()
                } else {
                    '-'
                }
            })
            .collect();
        format!("model-{slug}")
    };
    let serving =
        |id: &str| State::load(paths, id).is_ok_and(|s| s.observed_status() == Status::Running);
    // Reached on the bridge, not through the published port: the
    // address is in the container's own record and the port is not.
    let base = |id: &str| -> Result<String> {
        let ip = Config::load(paths, id)
            .ok()
            .and_then(|c| c.ip)
            .with_context(|| format!("{id} is not running; `rustainer model run` starts it"))?;
        Ok(format!("http://{ip}:8080"))
    };
    match cmd {
        ModelCmd::Pull { model: m } => {
            let r = parse(&m)?;
            let (path, fresh) = model::pull(&root, &r).map_err(|e| anyhow::anyhow!(e))?;
            if fresh {
                rustainer_core::events::record(paths, "model-pull", &m, serde_json::json!({}));
            }
            println!("{}", path.display());
        }
        ModelCmd::Ls => {
            for (name, bytes) in model::list(&root) {
                let state = if serving(&container(&parse(&name)?)) {
                    "serving"
                } else {
                    ""
                };
                println!("{name}\t{bytes}\t{state}");
            }
        }
        ModelCmd::Stop { model: m } => {
            let id = container(&parse(&m)?);
            if paths.dir(&id).exists() {
                supervise::remove(paths, &id, true)?;
            }
            println!("{id}");
        }
        ModelCmd::Rm { model: m } => {
            let r = parse(&m)?;
            let id = container(&r);
            if paths.dir(&id).exists() {
                supervise::remove(paths, &id, true)?;
            }
            let path = model::path(&root, &r);
            std::fs::remove_file(&path).with_context(|| format!("removing {}", path.display()))?;
            println!("{m}");
        }
        ModelCmd::Run {
            model: m,
            port,
            image,
        } => {
            let r = parse(&m)?;
            let id = container(&r);
            if serving(&id) {
                println!("{id} is already serving");
                return Ok(ExitCode::SUCCESS);
            }
            let (path, _) = model::pull(&root, &r).map_err(|e| anyhow::anyhow!(e))?;
            if paths.dir(&id).exists() {
                supervise::remove(paths, &id, true)?;
            }
            let file = path.file_name().unwrap().to_string_lossy();
            let mut command: Vec<OsString> = vec![image.into()];
            command.extend(
                [
                    "-m",
                    &format!("/models/{file}"),
                    "--host",
                    "0.0.0.0",
                    "--port",
                    "8080",
                ]
                .map(OsString::from),
            );
            let _ = cmd_run(
                paths,
                RunArgs {
                    detach: true,
                    name: Some(id.clone()),
                    network: "bridge".into(),
                    hostname: id.clone(),
                    volumes: vec![format!("{}:/models:ro", path.parent().unwrap().display())],
                    publish: vec![format!("127.0.0.1:{port}:8080")],
                    restart: "no".into(),
                    command,
                    ..Default::default()
                },
            )
            .context("starting the model server")?;
            let base = base(&id)?;
            // Loading is reading the whole file; allow a large one time.
            for _ in 0..300 {
                if model::ready(&base) {
                    println!("serving {m} at http://127.0.0.1:{port}/v1");
                    return Ok(ExitCode::SUCCESS);
                }
                std::thread::sleep(Duration::from_secs(1));
                // After the sleep: a container is `created` for a moment
                // before it is `running`, and that is not an exit.
                if !serving(&id) {
                    bail!("the model server exited while loading. `rustainer logs {id}` has why.");
                }
            }
            bail!("the model was not loaded in five minutes; `rustainer logs {id}`");
        }
        ModelCmd::Ask { model: m, prompt } => {
            let id = container(&parse(&m)?);
            let answer = model::ask(&base(&id)?, &prompt).map_err(|e| anyhow::anyhow!(e))?;
            println!("{answer}");
        }
    }
    Ok(ExitCode::SUCCESS)
}

/// The cluster's container, and its volume.
const KUBE: &str = "rustainer-kube";

fn cmd_kube(paths: &Paths, cmd: KubeCmd) -> Result<ExitCode> {
    // kubectl runs inside the node, where k3s ships it already pointed
    // at the cluster - as a child process, for the reason every exec
    // from a long-lived caller is one: `setns` moves the next child.
    let kubectl = |args: &[String]| -> Result<ExitCode> {
        let status = std::process::Command::new("/proc/self/exe")
            .arg("--root")
            .arg(paths_root(paths))
            .args(["exec", KUBE, "kubectl"])
            .args(args)
            .status()
            .context("running kubectl in the cluster's container")?;
        Ok(ExitCode::from(
            status.code().unwrap_or(1).clamp(0, 255) as u8
        ))
    };
    let running = State::load(paths, KUBE)
        .is_ok_and(|s| matches!(s.observed_status(), Status::Running | Status::Created));
    match cmd {
        KubeCmd::Up { image } => {
            if running {
                println!("{KUBE} is already running");
                return Ok(ExitCode::SUCCESS);
            }
            // A stopped one is in the way of the name; its state is in
            // the volume, not in the container.
            if paths.dir(KUBE).exists() {
                supervise::remove(paths, KUBE, true)?;
            }
            let mut command: Vec<OsString> = vec![image.into()];
            // Traefik is left out: it claims ports 80 and 443 on the
            // node, and a cluster that does that unasked is a surprise.
            command.extend(["server", "--disable", "traefik"].map(OsString::from));
            let _ = cmd_run(
                paths,
                RunArgs {
                    detach: true,
                    privileged: true,
                    name: Some(KUBE.into()),
                    network: "bridge".into(),
                    hostname: "rustainer-kube".into(),
                    volumes: vec![format!("{KUBE}:/var/lib/rancher")],
                    publish: vec!["127.0.0.1:6443:6443".into()],
                    restart: "no".into(),
                    command,
                    ..Default::default()
                },
            )
            .context("starting the cluster's container")?;
            println!("waiting for the node to be Ready");
            for _ in 0..90 {
                std::thread::sleep(Duration::from_secs(2));
                let out = std::process::Command::new("/proc/self/exe")
                    .arg("--root")
                    .arg(paths_root(paths))
                    .args(["exec", KUBE, "kubectl", "get", "nodes", "--no-headers"])
                    .output();
                if let Ok(o) = out
                    && String::from_utf8_lossy(&o.stdout).split_whitespace().nth(1) == Some("Ready")
                {
                    println!("the cluster is ready: rustainer kube kubectl get pods -A");
                    return Ok(ExitCode::SUCCESS);
                }
                if !State::load(paths, KUBE).is_ok_and(|s| s.observed_status() == Status::Running) {
                    bail!(
                        "the cluster's container exited while starting. \
                         `rustainer logs {KUBE}` has why."
                    );
                }
            }
            bail!("the node did not become Ready in three minutes; `rustainer logs {KUBE}`");
        }
        KubeCmd::Down => {
            if paths.dir(KUBE).exists() {
                supervise::remove(paths, KUBE, true)?;
            }
            println!("{KUBE} stopped; its data is in the {KUBE} volume");
        }
        KubeCmd::Kubectl { args } => return kubectl(&args),
        KubeCmd::Config => {
            if !running {
                bail!("the cluster is not running; `rustainer kube up` first");
            }
            let status = std::process::Command::new("/proc/self/exe")
                .arg("--root")
                .arg(paths_root(paths))
                .args(["exec", KUBE, "cat", "/etc/rancher/k3s/k3s.yaml"])
                .status()?;
            return Ok(ExitCode::from(
                status.code().unwrap_or(1).clamp(0, 255) as u8
            ));
        }
    }
    Ok(ExitCode::SUCCESS)
}

#[derive(Subcommand)]
enum ExtensionCmd {
    /// Copy an extension directory into the store.
    Install { dir: PathBuf },
    /// List installed extensions.
    Ls {
        #[arg(long)]
        json: bool,
    },
    /// Start an extension's containers.
    Enable { name: String },
    /// Stop and remove an extension's containers.
    Disable { name: String },
    /// Disable an extension and delete it from the store.
    Rm { name: String },
}

/// What `extension.toml` may say. Deliberately two fields.
#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct ExtensionManifest {
    #[serde(default)]
    description: String,
    /// Where the extension's page is once it is enabled. Loopback only:
    /// the page embeds it, and an extension that could point that frame
    /// at an arbitrary site would be a way to put somebody else's page
    /// inside this one.
    ui: String,
}

fn extensions_root() -> PathBuf {
    Store::default_root().join("extensions")
}

/// An installed extension: its directory, its stack and its manifest.
fn load_extension(dir: &Path) -> Result<(stack::Stack, ExtensionManifest)> {
    let st = read_stack(&[dir.join("rustainer.toml")], &[])
        .with_context(|| format!("{} has no usable rustainer.toml", dir.display()))?;
    let text = std::fs::read_to_string(dir.join("extension.toml"))
        .with_context(|| format!("{} has no extension.toml", dir.display()))?;
    let m: ExtensionManifest = toml::from_str(&text).context("extension.toml")?;
    let local = ["http://127.0.0.1:", "http://localhost:", "http://[::1]:"];
    if !local.iter().any(|p| m.ui.starts_with(p)) {
        bail!(
            "extension.toml: ui is {:?}, and an extension's page must be on this \
             machine (http://127.0.0.1:<port>/).",
            m.ui
        );
    }
    Ok((st, m))
}

fn cmd_extension(paths: &Paths, cmd: ExtensionCmd) -> Result<ExitCode> {
    let root = extensions_root();
    let installed = |name: &str| -> Result<PathBuf> {
        if name.is_empty() || name.contains('/') || name.starts_with('.') {
            bail!("{name:?} is not an extension name");
        }
        let d = root.join(name);
        if !d.is_dir() {
            bail!("no extension called {name} is installed");
        }
        Ok(d)
    };
    match cmd {
        ExtensionCmd::Install { dir } => {
            let (st, _) = load_extension(&dir)?;
            let dest = root.join(&st.name);
            if dest.exists() {
                bail!(
                    "an extension called {} is already installed; `extension rm {}` first",
                    st.name,
                    st.name
                );
            }
            std::fs::create_dir_all(&dest)?;
            // ponytail: `cp -a`, as for volumes - it is the copy that
            // already gets links and modes right.
            let ok = std::process::Command::new("cp")
                .arg("-a")
                .arg(dir.join("."))
                .arg(&dest)
                .status()
                .context("running cp")?
                .success();
            if !ok {
                let _ = std::fs::remove_dir_all(&dest);
                bail!("copying the extension failed");
            }
            println!("{}", st.name);
        }
        ExtensionCmd::Ls { json } => {
            let mut rows = Vec::new();
            for e in std::fs::read_dir(&root).into_iter().flatten().flatten() {
                let Ok((st, m)) = load_extension(&e.path()) else {
                    continue;
                };
                // Enabled means running, not "was asked to run": read
                // off the containers, like everything else here.
                let running = st
                    .services
                    .keys()
                    .flat_map(|svc| st.container_names(svc))
                    .filter(|id| {
                        State::load(paths, id).is_ok_and(|s| s.observed_status() == Status::Running)
                    })
                    .count();
                rows.push(serde_json::json!({
                    "name": st.name, "description": m.description, "ui": m.ui,
                    "running": running, "services": st.services.len(),
                }));
            }
            rows.sort_by_key(|r| r["name"].as_str().unwrap_or("").to_owned());
            if json {
                println!("{}", serde_json::Value::Array(rows));
            } else {
                for r in rows {
                    println!(
                        "{}\t{}/{} running\t{}",
                        r["name"].as_str().unwrap_or(""),
                        r["running"],
                        r["services"],
                        r["ui"].as_str().unwrap_or("")
                    );
                }
            }
        }
        ExtensionCmd::Enable { name } => {
            let file = installed(&name)?.join("rustainer.toml");
            let st = read_stack(std::slice::from_ref(&file), &[])?;
            return stack_up(paths, &st, &[file]);
        }
        ExtensionCmd::Disable { name } => {
            let file = installed(&name)?.join("rustainer.toml");
            let st = read_stack(&[file], &[])?;
            return stack_down(paths, &st, 5);
        }
        ExtensionCmd::Rm { name } => {
            let dir = installed(&name)?;
            let st = read_stack(&[dir.join("rustainer.toml")], &[])?;
            let _ = stack_down(paths, &st, 5);
            std::fs::remove_dir_all(&dir)?;
            println!("{name}");
        }
    }
    Ok(ExitCode::SUCCESS)
}

#[derive(Subcommand)]
enum McpCmd {
    /// List the servers in Docker Hub's `mcp` catalogue.
    Ls,
    /// Print the configuration that makes a client start this server
    /// in a container: the `mcpServers` entry most clients read.
    Config {
        /// A catalogue name such as `time`, or a full image reference.
        server: String,
        /// Pass an environment variable through, `KEY` or `KEY=value`.
        /// Repeatable. A bare `KEY` is written as a placeholder to fill
        /// in: a secret belongs in the client's file, not in this
        /// command's history.
        #[arg(short = 'e', long = "env")]
        env: Vec<String>,
        /// The network the server gets. `none` for one that needs no
        /// network is the safer default to choose deliberately.
        #[arg(long, default_value = "bridge")]
        network: String,
    },
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
    Down(StackDownArgs),
    /// Show the stack's containers.
    Ps(StackArgs),
    /// Rebuild or restart services when their files change.
    ///
    /// Compose's `develop.watch`: `rebuild`, `restart`, and `sync`,
    /// which copies changed files into the running container. Polls mtimes rather
    /// than subscribing to inotify - a watch that misses an event is
    /// worse than one that notices a second late, and inotify's queue
    /// overflows silently under a build that touches a thousand files.
    Watch(StackArgs),
    /// Replace a service's containers with what the file now says, and
    /// put the old ones back if the new ones are not healthy.
    ///
    /// Compose's `deploy.rollback_config`, in the one form a single
    /// host can honour: the service's own healthcheck decides. The old
    /// container is stopped and set aside - not removed - the new one
    /// is started in its place with the same name and address, and
    /// only once it passes its healthcheck is the old one deleted. If
    /// it never passes, the new one is removed and the old one is
    /// started again exactly as it was, files and all.
    ///
    /// There is a gap in service while this happens. Two containers
    /// cannot hold one published port, so "start the new one first" is
    /// not available without a proxy in front, and there is none here.
    Update(StackRunArgs),
    /// Run a one-off container with a service's settings.
    ///
    /// The stack's `compose run`: same image, environment, volumes and
    /// network as the service, a command of your choosing, in the
    /// foreground, removed when it exits. It does NOT start the
    /// service's dependencies - they are either up or they are not, and
    /// starting half a stack as a side effect of a one-off command is
    /// how you end up with two copies of a database.
    Run(StackRunArgs),
}

#[derive(Args)]
struct StackArgs {
    /// The stack file. `rustainer.toml` in the current directory by
    /// default. Repeatable: later files override earlier ones key by
    /// key, which is what `-f base.toml -f prod.toml` means everywhere
    /// else. A list REPLACES rather than appends - appending would make
    /// an override that sets `command` run something nobody wrote.
    #[arg(short = 'f', long, default_value = "rustainer.toml")]
    file: Vec<PathBuf>,

    /// Include the services in this profile. Repeatable.
    ///
    /// A service with no `profiles` is always included; one with them is
    /// included only when you name a profile it belongs to.
    #[arg(long = "profile")]
    profile: Vec<String>,
}

#[derive(Args)]
struct StackRunArgs {
    /// The stack file(s). Repeatable, later over earlier.
    #[arg(short = 'f', long, default_value = "rustainer.toml")]
    file: Vec<PathBuf>,

    /// Include the services in this profile. Repeatable.
    #[arg(long = "profile")]
    profile: Vec<String>,

    /// The service whose settings to borrow.
    service: String,

    /// The command to run instead of the service's own. Optional.
    #[arg(trailing_var_arg = true)]
    command: Vec<OsString>,
}

#[derive(Args)]
struct StackDownArgs {
    /// The stack file(s). Repeatable, later over earlier, as with `up`.
    #[arg(short = 'f', long, default_value = "rustainer.toml")]
    file: Vec<PathBuf>,

    /// Include the services in this profile. Repeatable.
    #[arg(long = "profile")]
    profile: Vec<String>,

    /// Seconds to wait for each service to exit before killing it.
    ///
    /// This is most of what `down` costs, and the reason is worth
    /// knowing: PID 1 in a new PID namespace has no default disposition
    /// for SIGTERM, so a process that installs no handler - `sleep`, and
    /// plenty of real entrypoints - ignores it and waits out the whole
    /// grace period. Three such services at the default is fifteen
    /// seconds of nothing happening.
    #[arg(short = 't', long, default_value_t = 5)]
    timeout: u64,
}

#[derive(Args)]
struct PushArgs {
    /// The image in the local store.
    image: String,

    /// Where to push it. Defaults to the same reference.
    #[arg(long)]
    to: Option<String>,

    /// `user:password` for the registry. Prefer the environment variable
    /// RUSTAINER_REGISTRY_AUTH, which does not appear in `ps` output or
    /// in your shell history.
    #[arg(long)]
    creds: Option<String>,
}

#[derive(Args)]
struct BuildArgs {
    /// Name for the built image, e.g. `-t myapp:1`.
    #[arg(short = 't', long = "tag")]
    tag: String,

    /// The Dockerfile. `<context>/Dockerfile` by default.
    #[arg(short = 'f', long)]
    file: Option<PathBuf>,

    /// Set a build argument, `KEY=value`. Repeatable.
    #[arg(long = "build-arg")]
    build_args: Vec<String>,

    /// The network a RUN step gets: `bridge`, `none`, `host`, or one
    /// you created. The same values `run --network` takes, because a
    /// step is an ordinary container.
    ///
    /// `none` is the one worth knowing about. A bridge needs
    /// CAP_NET_ADMIN in the user namespace that owns the HOST's network
    /// namespace, which a nested one can never have, so without this a
    /// build is impossible rootless even when no step wants the network.
    /// It is also how a build is made hermetic: a step that cannot
    /// reach a registry cannot quietly depend on one.
    #[arg(long, default_value = "bridge")]
    network: String,

    /// Run every step again, even ones an earlier build already ran
    /// with the same inputs.
    #[arg(long)]
    no_cache: bool,

    /// The build context: the directory COPY may read from.
    #[arg(default_value = ".")]
    context: PathBuf,
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
enum NetworkCmd {
    /// Create a network with a bridge and subnet of its own.
    Create {
        name: String,
        /// `10.89.N.0/24` by default, picked from what is free.
        #[arg(long)]
        subnet: Option<String>,
    },
    /// List networks.
    Ls,
    /// Remove a network. Refused while containers are on it.
    Rm { name: String },
}

#[derive(Subcommand)]
enum VolumeCmd {
    /// List named volumes.
    Ls,
    /// Remove a named volume and everything in it.
    Rm { name: String },
    /// Create an empty named volume.
    Create { name: String },
    /// Delete everything in a volume and keep the volume.
    Empty { name: String },
    /// Copy a volume, contents and ownership, under a new name.
    Clone { from: String, to: String },
    /// Write a volume's contents as a tar archive, to a file or stdout.
    Export {
        name: String,
        #[arg(short, long)]
        output: Option<PathBuf>,
    },
    /// Replace nothing, add everything: unpack a tar archive into a
    /// volume, from a file or stdin. The volume is created if missing.
    Import {
        name: String,
        #[arg(short, long)]
        input: Option<PathBuf>,
    },
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
        Command::Start { id } => cmd_start(&paths, &id),
        Command::Restart(a) => {
            supervise::stop(&paths, &a.id, Duration::from_secs(a.time))
                .with_context(|| format!("stopping {}", a.id))?;
            cmd_start(&paths, &a.id)
        }
        Command::Rm(a) => cmd_rm(&paths, a),
        Command::Logs(a) => cmd_logs(&paths, a),
        Command::Exec(a) => cmd_exec(&paths, a),
        Command::Build(a) => cmd_build(&paths, a),
        Command::Pull(a) => cmd_pull(a),
        Command::Push(a) => cmd_push(&paths, a),
        Command::Images => cmd_images(),
        Command::Rmi(a) => cmd_rmi(a),
        Command::Scan { image, json } => cmd_scan(&image, json),
        Command::Kube(c) => cmd_kube(&paths, c),
        Command::Model(c) => cmd_model(&paths, c),
        Command::Extension(c) => cmd_extension(&paths, c),
        Command::Mcp(McpCmd::Ls) => {
            for r in rustainer_image::registry::hub_repositories("mcp", None)
                .map_err(|e| anyhow::anyhow!(e))?
            {
                println!("{}\t{}", r.name, r.pulls);
            }
            Ok(ExitCode::SUCCESS)
        }
        Command::Mcp(McpCmd::Config {
            server,
            env,
            network,
        }) => {
            let image = if server.contains('/') {
                server.clone()
            } else {
                format!("mcp/{server}")
            };
            let name = server
                .rsplit('/')
                .next()
                .unwrap_or(&server)
                .split(':')
                .next()
                .unwrap_or(&server)
                .to_owned();
            let mut args: Vec<String> = ["run", "-i", "--rm", "--network", &network]
                .iter()
                .map(|s| (*s).to_owned())
                .collect();
            let mut placeholders = serde_json::Map::new();
            for e in &env {
                let key = e.split('=').next().unwrap_or(e);
                // The value travels in the client's `env` block and is
                // named, not inlined, on the command line.
                args.push("-e".into());
                args.push(key.to_owned());
                placeholders.insert(
                    key.to_owned(),
                    serde_json::json!(e.split_once('=').map_or("<fill in>", |(_, v)| v)),
                );
            }
            args.push(image);
            let doc = serde_json::json!({ "mcpServers": { name: {
                "command": "rustainer", "args": args, "env": placeholders,
            }}});
            println!("{}", serde_json::to_string_pretty(&doc)?);
            Ok(ExitCode::SUCCESS)
        }
        Command::Repos { namespace } => {
            let creds = std::env::var("RUSTAINER_REGISTRY_AUTH")
                .ok()
                .filter(|s| !s.is_empty());
            // Signed in and asked for nothing in particular: your own.
            let namespace = namespace
                .or_else(|| Some(creds.as_deref()?.split_once(':')?.0.to_owned()))
                .context(
                    "a namespace is required (or sign in: RUSTAINER_REGISTRY_AUTH=user:token)",
                )?;
            let repos = rustainer_image::registry::hub_repositories(&namespace, creds.as_deref())
                .map_err(|e| anyhow::anyhow!(e))?;
            for r in repos {
                let private = if r.private { "\tprivate" } else { "" };
                println!(
                    "{namespace}/{}\t{}\t{}{private}",
                    r.name, r.pulls, r.updated
                );
            }
            Ok(ExitCode::SUCCESS)
        }
        Command::Tags { repository } => {
            let r = rustainer_image::Reference::parse(&repository)
                .map_err(|e| anyhow::anyhow!("{repository}: {e}"))?;
            let tags = rustainer_image::registry::Client::for_reading(&r)
                .tags(&r)
                .with_context(|| format!("listing the tags of {repository}"))?;
            for t in tags {
                println!("{t}");
            }
            Ok(ExitCode::SUCCESS)
        }
        Command::Volume(a) => cmd_volume(&paths, a),
        Command::Network(a) => cmd_network(&paths, a),
        Command::Events(a) => cmd_events(&paths, a),
        Command::Serve(a) => cmd_serve(cli_root.clone(), a),
        Command::Stack(a) => cmd_stack(&paths, a),
        Command::Supervise { id } => cmd_supervise(&paths, &id),
        Command::Forward { fd, to } => cmd_forward(fd, &to),
        Command::Put {
            volume,
            target,
            path,
        } => {
            let root = if volume {
                volume_dir(&target, true)?
            } else {
                let pid = State::load(&paths, &target)
                    .ok()
                    .filter(|s| s.observed_status() == Status::Running)
                    .and_then(|s| s.init_pid)
                    .with_context(|| format!("{target} is not a running container"))?;
                PathBuf::from(format!("/proc/{pid}/root"))
            };
            write_confined(&root, Path::new(&path), &mut std::io::stdin().lock(), None)?;
            println!("{path}");
            Ok(ExitCode::SUCCESS)
        }
        Command::HealthWatch {
            id,
            interval,
            retries,
            test,
        } => cmd_health_watch(&paths, &id, interval, retries, &test),
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
        other => {
            if networks_dir().join(format!("{other}.json")).exists() {
                (true, true)
            } else {
                bail!(
                    "--network {other} is not a network. `bridge` gives the container an \
                     address on the engine's own bridge; `none` gives it only loopback; \
                     `host` shares the host's stack. Make a network of your own with \
                     `rustainer network create {other}`."
                )
            }
        }
    };
    let network_def = match args.network.as_str() {
        "bridge" | "none" | "host" => None,
        other => Some(load_network(other)?),
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
            let store = Store::new(Store::default_root()).platform(args.platform.as_deref());
            // The store first, the registry only if it has to be. `pull`
            // begins by fetching the manifest, so going through it for an
            // image already on disk cost four TLS round-trips before
            // anything was cloned - 870 of the 890 milliseconds a
            // container start took - and made a fully cached image
            // unrunnable with no network at all. This is Docker's
            // `--pull=missing`, which is Docker's default; `--pull` still
            // means "go and check".
            let image = match store.local(&reference) {
                // A named platform always asks: the tag on disk may be
                // this machine's build, and the config does not say.
                Some(i) if !args.pull && args.platform.is_none() => i,
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

            // What comes after the image name replaces the image's CMD,
            // not its ENTRYPOINT: `run rancher/k3s server` is `k3s
            // server`, because the image's entrypoint is `k3s` and its
            // arguments are what the user is supplying. This used to
            // replace both, which is right only for images with no
            // entrypoint - most small ones, and so most tests.
            let entrypoint: Vec<String> = match &args.entrypoint {
                Some(e) if e.is_empty() => Vec::new(),
                Some(e) => vec![e.clone()],
                None => image.entrypoint.clone(),
            };
            let tail: Vec<OsString> = if args.command.len() > 1 {
                args.command[1..].to_vec()
            } else if args.entrypoint.is_some() {
                // Replacing the entrypoint discards the image's CMD too,
                // as Docker's does: it was written as arguments to the
                // entrypoint that is no longer there.
                Vec::new()
            } else {
                image.cmd.iter().map(OsString::from).collect()
            };
            let cmd: Vec<OsString> = entrypoint
                .into_iter()
                .map(OsString::from)
                .chain(tail)
                .collect();
            if cmd.is_empty() {
                bail!(
                    "{reference} declares neither an Entrypoint nor a Cmd, so there is \
                     nothing to run. Give a command after the image name."
                );
            }
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
    let net_def = network_def.clone().unwrap_or_default();
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
            prefix: net_def.prefix,
            gateway: net_def.gateway,
        }),
        (true, None) => Some(
            rustainer_core::net::allocate_in(&paths_root(paths), &net_def)
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
            // A bare `-e KEY` means "the value I have": taken from our
            // own environment, and dropped if we do not have one. It is
            // how a secret reaches a container without being written
            // on a command line that `ps` shows to everybody.
            let wanted: Vec<String> = args
                .env
                .iter()
                .filter_map(|e| match e.contains('=') {
                    true => Some(e.clone()),
                    false => std::env::var(e).ok().map(|v| format!("{e}={v}")),
                })
                .collect();
            rustainer_core::exec::merge_env(&base, &wanted)
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
        network: network_def.clone(),
        userns,
        tty: args.tty,
        stdin_open: args.interactive && !args.tty && !args.detach,
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
        privileged: args.privileged,
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
            // The reference when there was one: the rootfs of an image
            // is a path under the state directory, which names nothing.
            "image": if args.rootfs.is_none() && args.layer.is_empty() {
                args.command.first().map(|c| c.to_string_lossy().into_owned())
            } else {
                config.rootfs.clone()
            }
            .unwrap_or_default(),
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
    if args.rm {
        let _ = supervise::remove(paths, &id, true);
    }
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

fn cmd_start(paths: &Paths, id: &str) -> Result<ExitCode> {
    use rustainer_core::state::Status;
    let st = State::load(paths, id).with_context(|| format!("no such container: {id}"))?;
    if matches!(st.observed_status(), Status::Running | Status::Created) {
        bail!("{id} is already running");
    }
    // Everything a start needs is in the stored config, and the
    // supervisor already knows how to bring a container up from it more
    // than once - that is what a restart policy is. So this is `run`
    // from the point where the config has been written: a fresh state
    // record, a new supervisor, the same handshake.
    let config = supervise::Config::load(paths, id)
        .with_context(|| format!("{id} has no stored configuration to start from"))?;
    let lock = rustainer_core::state::Lock::acquire(&paths.lock_file(id))
        .context("locking the container directory")?;
    State::new(id.to_owned(), config.argv.clone()).store(paths)?;
    // The health watcher left with the old process and nothing is
    // probing this one, so its last verdict is about something that no
    // longer exists. No answer is better than a stale "healthy".
    let _ = std::fs::remove_file(paths.dir(id).join("health"));
    let (handshake_rx, handshake_tx) = sys::pipe()?;
    let mut child = spawn_supervisor(paths, id, handshake_tx, true, false)?;
    read_handshake(paths, &handshake_rx, &mut child, id)?;
    drop(lock);
    println!("{id}");
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
    let store = Store::new(Store::default_root()).platform(args.platform.as_deref());
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

/// Where user-defined networks are recorded.
///
/// Beside the volumes, because a network outlives a reboot in the same
/// way: the bridge does not, but the NAME, the subnet and the containers
/// that reference it all do, and recreating the bridge from the record is
/// what the first container on it does anyway.
fn networks_dir() -> PathBuf {
    Store::default_root().join("networks")
}

fn load_network(name: &str) -> Result<rustainer_core::net::Network> {
    let p = networks_dir().join(format!("{name}.json"));
    let text = std::fs::read_to_string(&p)
        .with_context(|| format!("no network {name:?}. `rustainer network ls` lists them."))?;
    serde_json::from_str(&text).with_context(|| format!("reading the network {name:?}"))
}

fn all_networks() -> Vec<rustainer_core::net::Network> {
    let mut out = Vec::new();
    if let Ok(rd) = std::fs::read_dir(networks_dir()) {
        for e in rd.flatten() {
            if e.path().extension().and_then(|x| x.to_str()) != Some("json") {
                continue;
            }
            if let Ok(t) = std::fs::read_to_string(e.path())
                && let Ok(n) = serde_json::from_str(&t)
            {
                out.push(n);
            }
        }
    }
    out.sort_by(|a: &rustainer_core::net::Network, b| a.name.cmp(&b.name));
    out
}

fn cmd_network(paths: &Paths, cmd: NetworkCmd) -> Result<ExitCode> {
    use rustainer_core::net::Network;
    match cmd {
        NetworkCmd::Create { name, subnet } => {
            if name.is_empty()
                || !name
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
            {
                bail!("{name:?} is not a usable network name");
            }
            if matches!(name.as_str(), "bridge" | "none" | "host") {
                bail!(
                    "{name:?} is one of the three built-in network modes, so a network \
                     of that name could never be selected"
                );
            }
            std::fs::create_dir_all(networks_dir())?;
            let path = networks_dir().join(format!("{name}.json"));
            if path.exists() {
                bail!("a network called {name:?} already exists");
            }
            let existing = all_networks();
            let (gateway, prefix) = match &subnet {
                Some(cidr) => parse_subnet(cidr)?,
                None => {
                    // 10.89.N.0/24, taking the first N nobody is using.
                    // Deliberately not in 172.16/12: Docker allocates
                    // there, and two engines on one host colliding on a
                    // subnet is a routing bug nobody enjoys.
                    let used: Vec<u8> = existing
                        .iter()
                        .filter(|n| n.gateway.octets()[..2] == [10, 89])
                        .map(|n| n.gateway.octets()[2])
                        .collect();
                    let third = (0u8..=254)
                        .find(|n| !used.contains(n))
                        .ok_or_else(|| anyhow::anyhow!("no free subnet in 10.89.0.0/16"))?;
                    (std::net::Ipv4Addr::new(10, 89, third, 1), 24)
                }
            };
            for n in &existing {
                if n.gateway == gateway {
                    bail!("the network {:?} already uses {gateway}", n.name);
                }
            }
            let net = Network {
                name: name.clone(),
                bridge: Network::bridge_for(&name),
                gateway,
                prefix,
            };
            std::fs::write(&path, serde_json::to_vec_pretty(&net)?)?;
            rustainer_core::events::record(
                paths,
                "network-create",
                &name,
                serde_json::json!({ "subnet": format!("{gateway}/{prefix}"), "bridge": net.bridge }),
            );
            println!("{name}");
        }
        NetworkCmd::Ls => {
            println!(
                "{:<20} {:<20} {:<18} CONTAINERS",
                "NAME", "SUBNET", "BRIDGE"
            );
            let mut rows = vec![Network::default()];
            rows.extend(all_networks());
            for n in rows {
                let count = containers_on(paths, &n.name).len();
                println!(
                    "{:<20} {:<20} {:<18} {}",
                    n.name,
                    format!("{}/{}", n.gateway, n.prefix),
                    n.bridge,
                    count
                );
            }
        }
        NetworkCmd::Rm { name } => {
            if name.contains('/') || name.contains("..") {
                bail!("{name:?} is not a network name");
            }
            let on_it = containers_on(paths, &name);
            if !on_it.is_empty() {
                bail!(
                    "{} container(s) are still on {name}: {}. Remove them first - \
                     taking the network away under a running container leaves it with \
                     an address nothing routes.",
                    on_it.len(),
                    on_it.join(", ")
                );
            }
            let path = networks_dir().join(format!("{name}.json"));
            if !path.exists() {
                bail!("no network {name:?}");
            }
            std::fs::remove_file(&path)?;
            rustainer_core::events::record(paths, "network-remove", &name, serde_json::json!({}));
            println!("{name}");
        }
    }
    Ok(ExitCode::SUCCESS)
}

/// `10.89.3.0/24` or `10.89.3.1/24` - the gateway is always `.1`.
fn parse_subnet(cidr: &str) -> Result<(std::net::Ipv4Addr, u8)> {
    let (addr, prefix) = cidr
        .split_once('/')
        .ok_or_else(|| anyhow::anyhow!("{cidr:?} is not a CIDR subnet like 10.89.3.0/24"))?;
    let prefix: u8 = prefix
        .parse()
        .map_err(|_| anyhow::anyhow!("{prefix:?} is not a prefix length"))?;
    if !(8..=24).contains(&prefix) {
        bail!("a prefix of /{prefix} is not supported; use between /8 and /24");
    }
    let a: std::net::Ipv4Addr = addr
        .parse()
        .map_err(|_| anyhow::anyhow!("{addr:?} is not an IPv4 address"))?;
    let o = a.octets();
    Ok((std::net::Ipv4Addr::new(o[0], o[1], o[2], 1), prefix))
}

/// Which containers are on a network right now.
///
/// Read from the containers themselves rather than from a membership
/// list: a list would be a second thing to keep correct, and a container
/// that died without updating it would leave the network permanently
/// un-removable.
fn containers_on(paths: &Paths, network: &str) -> Vec<String> {
    let mut out = Vec::new();
    for id in paths.list().unwrap_or_default() {
        let Ok(cfg) = Config::load(paths, &id) else {
            continue;
        };
        let name = cfg
            .network
            .as_ref()
            .map(|n| n.name.clone())
            .unwrap_or_else(|| "bridge".into());
        if name != network || !cfg.bridge {
            continue;
        }
        if State::load(paths, &id)
            .map(|s| matches!(s.observed_status(), Status::Running | Status::Created))
            .unwrap_or(false)
        {
            out.push(id);
        }
    }
    out
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

/// Checked from the stack parser so a bad policy is refused before any
/// of the stack is running, rather than when that one service starts.
pub(crate) fn supervise_restart_check(spec: &str) -> Result<()> {
    supervise::RestartPolicy::parse(spec).map_err(|e| anyhow::anyhow!("{e}"))?;
    Ok(())
}

fn read_stack(files: &[PathBuf], profiles: &[String]) -> Result<stack::Stack> {
    let fallback = PathBuf::from("rustainer.toml");
    let first = files.first().unwrap_or(&fallback);
    // `.env` beside the FIRST STACK FILE, not beside the working
    // directory: `-f ../other/rustainer.toml` must mean the same thing
    // from anywhere, and a stack that picks up a different .env
    // depending on where you ran it is the bug this ordering avoids.
    let mut vars: std::collections::BTreeMap<String, String> = Default::default();
    let dotenv = first.parent().unwrap_or(Path::new(".")).join(".env");
    if let Ok(t) = std::fs::read_to_string(&dotenv) {
        vars =
            stack::Stack::parse_dotenv(&t).with_context(|| format!("in {}", dotenv.display()))?;
    }
    // The environment wins over .env, as it does in Compose: the file is
    // the default, the environment is the override.
    for (k, v) in std::env::vars() {
        vars.insert(k, v);
    }
    // Each file is interpolated on its own and the results are merged.
    // The other order would let a value substituted into one file be
    // read as TOML structure when another is merged over it.
    let mut texts: Vec<String> = Vec::with_capacity(files.len());
    for f in files {
        let t = std::fs::read_to_string(f).with_context(|| {
            format!(
                "reading the stack file {}. Write one, or point at it with -f.",
                f.display()
            )
        })?;
        texts.push(
            stack::Stack::interpolate(&t, &vars).with_context(|| format!("in {}", f.display()))?,
        );
    }
    let where_ = files
        .iter()
        .map(|p| p.display().to_string())
        .collect::<Vec<_>>()
        .join(" + ");
    let mut st = stack::Stack::parse_many(&texts).with_context(|| format!("in {where_}"))?;
    st.select_profiles(profiles)
        .with_context(|| format!("in {where_}"))?;
    Ok(st)
}

fn cmd_stack(paths: &Paths, cmd: StackCmd) -> Result<ExitCode> {
    match cmd {
        StackCmd::Up(a) => stack_up(paths, &read_stack(&a.file, &a.profile)?, &a.file),
        StackCmd::Down(a) => stack_down(paths, &read_stack(&a.file, &a.profile)?, a.timeout),
        StackCmd::Ps(a) => stack_ps(paths, &read_stack(&a.file, &a.profile)?),
        StackCmd::Watch(a) => stack_watch(paths, &read_stack(&a.file, &a.profile)?, &a.file),
        StackCmd::Run(a) => {
            let st = read_stack(&a.file, &a.profile)?;
            stack_run(paths, &st, &a)
        }
        StackCmd::Update(a) => {
            let st = read_stack(&a.file, &a.profile)?;
            stack_update(paths, &st, &a.file, &a.service)
        }
    }
}

/// A cheap fingerprint of a directory tree: how many files, and the
/// newest modification time among them.
///
/// Not a hash. Hashing a build context on every poll is how a watch
/// comes to cost more than the build it triggers, and the question
/// being answered is "did anything change", which mtime answers for
/// every editor anybody uses.
fn tree_stamp(root: &Path) -> (u64, u64) {
    fn walk(p: &Path, n: &mut u64, newest: &mut u64) {
        let Ok(rd) = std::fs::read_dir(p) else { return };
        for e in rd.flatten() {
            let Ok(md) = e.metadata() else { continue };
            if md.is_dir() {
                walk(&e.path(), n, newest);
            } else {
                *n += 1;
                if let Ok(t) = md.modified()
                    && let Ok(d) = t.duration_since(std::time::UNIX_EPOCH)
                {
                    *newest = (*newest).max(d.as_secs());
                }
            }
        }
    }
    let mut n = 0;
    let mut newest = 0;
    if root.is_file() {
        if let Ok(md) = std::fs::metadata(root)
            && let Ok(t) = md.modified()
            && let Ok(d) = t.duration_since(std::time::UNIX_EPOCH)
        {
            return (1, d.as_secs());
        }
        return (1, 0);
    }
    walk(root, &mut n, &mut newest);
    (n, newest)
}

/// Watch the declared paths and rebuild or restart when they change.
///
/// Runs until interrupted, like `compose watch`.
/// What a `sync` watch remembers about each file: enough to tell that
/// it changed. Per file rather than "newer than the last pass", because
/// a file moved into the tree keeps its old mtime and is still new.
type Seen = std::collections::HashMap<PathBuf, (Option<std::time::SystemTime>, u64)>;

/// The regular files under `root`, relative to it. Symlinks are not
/// followed and not copied: where one points means something different
/// inside the container, and guessing is how a sync copies the wrong
/// file.
fn sync_scan(root: &Path) -> Seen {
    fn walk(root: &Path, dir: &Path, out: &mut Seen) {
        let Ok(rd) = std::fs::read_dir(dir) else {
            return;
        };
        for e in rd.flatten() {
            let Ok(md) = e.metadata() else { continue };
            if md.is_dir() {
                walk(root, &e.path(), out);
            } else if md.is_file()
                && let Ok(rel) = e.path().strip_prefix(root)
            {
                out.insert(rel.to_path_buf(), (md.modified().ok(), md.len()));
            }
        }
    }
    let mut out = Seen::new();
    if root.is_file() {
        if let (Ok(md), Some(name)) = (std::fs::metadata(root), root.file_name()) {
            out.insert(PathBuf::from(name), (md.modified().ok(), md.len()));
        }
    } else {
        walk(root, root, &mut out);
    }
    out
}

/// Copy `rels` from `src` into a running container at `target`.
///
/// Written through `/proc/<pid>/root`, with every directory resolved by
/// `openat2(RESOLVE_IN_ROOT)`: the destination tree belongs to the
/// container, and a symlink it planted at `/app/x -> /etc` must land
/// the copy in ITS `/etc`, never the host's. Each file is written
/// beside its destination and renamed over it, so a reader in the
/// container sees the old file or the new one and never half of
/// either, and a symlink sitting at the final name is replaced rather
/// than followed.
///
/// Returns how many arrived. One file that cannot be written - the
/// planted-symlink case above, or a read-only mount - is reported and
/// does not hold up the rest of the batch.
fn sync_into(pid: i32, src: &Path, target: &str, rels: &[PathBuf]) -> Result<usize> {
    use std::os::unix::fs::PermissionsExt;
    let root = PathBuf::from(format!("/proc/{pid}/root"));
    let single = src.is_file();
    let mut done = 0;
    for rel in rels {
        let from = if single {
            src.to_path_buf()
        } else {
            src.join(rel)
        };
        let dest = Path::new(target.trim_start_matches('/')).join(rel);
        let one = || -> Result<()> {
            let mode = std::fs::metadata(&from)?.permissions().mode();
            write_confined(&root, &dest, &mut std::fs::File::open(&from)?, Some(mode))
        };
        match one() {
            Ok(()) => done += 1,
            Err(e) => eprintln!("rustainer: sync: {}: {e:#}", rel.display()),
        }
    }
    Ok(done)
}

/// Write `reader` to `dest`, a path relative to `root`, without leaving
/// `root` - the one way this engine writes into a tree it does not own.
///
/// Missing parent directories are made. With no `mode`, a file that is
/// being replaced keeps its mode and its owner: an edit that quietly
/// turned nginx's config from `nginx:nginx 0640` into `root:root 0644`
/// would be a different change from the one that was asked for.
fn write_confined(
    root: &Path,
    dest: &Path,
    reader: &mut dyn Read,
    mode: Option<u32>,
) -> Result<()> {
    use rustix::fs::{AtFlags, Mode, OFlags, ResolveFlags};
    use std::os::unix::fs::{MetadataExt, PermissionsExt};

    let root = rustix::fs::open(
        root,
        OFlags::DIRECTORY | OFlags::RDONLY | OFlags::CLOEXEC,
        Mode::empty(),
    )
    .with_context(|| format!("opening {}", root.display()))?;
    let inside = |rel: &Path| {
        rustix::fs::openat2(
            &root,
            if rel.as_os_str().is_empty() {
                Path::new(".")
            } else {
                rel
            },
            OFlags::DIRECTORY | OFlags::RDONLY | OFlags::CLOEXEC,
            Mode::empty(),
            ResolveFlags::IN_ROOT | ResolveFlags::NO_MAGICLINKS,
        )
    };
    let dest = dest.strip_prefix("/").unwrap_or(dest);
    let (Some(parent), Some(name)) = (dest.parent(), dest.file_name()) else {
        bail!("{} is not a file path", dest.display());
    };
    // mkdir -p, one component at a time, each one looked up from the
    // root again so the confinement holds at every step.
    let mut walked = PathBuf::new();
    for comp in parent.components() {
        let here = inside(&walked).with_context(|| format!("/{}", walked.display()))?;
        match rustix::fs::mkdirat(&here, comp.as_os_str(), Mode::from_bits_truncate(0o755)) {
            Ok(()) | Err(rustix::io::Errno::EXIST) => {}
            Err(e) => bail!("creating /{}: {e}", walked.join(comp).display()),
        }
        walked.push(comp);
    }
    let dir = inside(parent).with_context(|| format!("/{}", parent.display()))?;
    // What is there now, not following a link: a link at the final name
    // is replaced, never written through.
    let before = rustix::fs::statat(&dir, name, AtFlags::SYMLINK_NOFOLLOW)
        .ok()
        .filter(|st| st.st_mode & libc::S_IFMT == libc::S_IFREG);
    let mode = mode
        .or(before.as_ref().map(|st| st.st_mode))
        .unwrap_or(0o644)
        & 0o7777;
    // Written beside the destination and renamed over it, so a reader
    // sees the old file or the new one and never half of either.
    let tmp = format!(".{}.rustainer-tmp", name.to_string_lossy());
    let _ = rustix::fs::unlinkat(&dir, tmp.as_str(), AtFlags::empty());
    let fd = rustix::fs::openat(
        &dir,
        tmp.as_str(),
        OFlags::WRONLY | OFlags::CREATE | OFlags::EXCL | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::from_bits_truncate(mode),
    )
    .with_context(|| format!("writing /{}", dest.display()))?;
    let mut out = std::fs::File::from(fd);
    std::io::copy(reader, &mut out).with_context(|| format!("writing /{}", dest.display()))?;
    // The mode again, explicitly: the create was subject to our umask.
    let _ = out.set_permissions(std::fs::Permissions::from_mode(mode));
    if let Some(st) = before
        && let Ok(now) = out.metadata()
        && (now.uid(), now.gid()) != (st.st_uid, st.st_gid)
    {
        let _ = std::os::unix::fs::fchown(&out, Some(st.st_uid), Some(st.st_gid));
    }
    rustix::fs::renameat(&dir, tmp.as_str(), &dir, name)
        .with_context(|| format!("replacing /{}", dest.display()))?;
    Ok(())
}

fn stack_watch(paths: &Paths, st: &stack::Stack, files: &[PathBuf]) -> Result<ExitCode> {
    let base = files
        .first()
        .and_then(|f| f.parent())
        .unwrap_or(Path::new("."))
        .to_path_buf();
    let mut watched: Vec<(String, stack::Watch, PathBuf, (u64, u64))> = Vec::new();
    let mut seen: std::collections::HashMap<PathBuf, Seen> = std::collections::HashMap::new();
    for (name, svc) in &st.services {
        for w in &svc.watch {
            let p = base.join(&w.path);
            if !p.exists() {
                bail!(
                    "service {name:?} watches {}, which does not exist",
                    p.display()
                );
            }
            let stamp = tree_stamp(&p);
            println!("watching {} for {name} ({})", p.display(), w.action);
            if w.action == "sync" {
                seen.insert(p.clone(), sync_scan(&p));
            }
            watched.push((name.clone(), w.clone(), p, stamp));
        }
    }
    if watched.is_empty() {
        bail!(
            "no service in this stack declares a `watch`, so there is nothing to \
             watch. Add `[[services.<name>.watch]]` with a `path`."
        );
    }
    println!("watching. ^C to stop.");
    loop {
        std::thread::sleep(Duration::from_secs(1));
        // Collected first, acted on after: two services watching the
        // same directory must not rebuild one at a time while the other
        // sees a half-written tree.
        let mut hit: Vec<(String, String)> = Vec::new();
        for (name, w, path, stamp) in &mut watched {
            if w.action == "sync" {
                // No container is replaced, so there is nothing to
                // collect for later: copy what differs, now.
                let now = sync_scan(path);
                let before = seen.insert(path.clone(), now.clone()).unwrap_or_default();
                let changed: Vec<PathBuf> = now
                    .iter()
                    .filter(|(rel, meta)| before.get(*rel) != Some(meta))
                    .map(|(rel, _)| rel.clone())
                    .collect();
                if changed.is_empty() {
                    continue;
                }
                let target = w.target.as_deref().unwrap_or("/");
                for id in st.container_names(name) {
                    let pid = State::load(paths, &id)
                        .ok()
                        .filter(|s| s.observed_status() == Status::Running)
                        .and_then(|s| s.init_pid);
                    let Some(pid) = pid else { continue };
                    match sync_into(pid, path, target, &changed) {
                        Ok(n) => println!("sync: {n} file(s) into {id}:{target}"),
                        Err(e) => eprintln!("rustainer: sync into {id}: {e:#}"),
                    }
                }
                continue;
            }
            let now = tree_stamp(path);
            if now != *stamp {
                *stamp = now;
                hit.push((name.clone(), w.action.clone()));
            }
        }
        hit.dedup();
        for (name, action) in hit {
            println!("{}: {name} changed", action);
            for id in st.container_names(&name) {
                if paths.dir(&id).exists() {
                    let _ = supervise::stop(paths, &id, Duration::from_secs(5));
                    let _ = supervise::remove(paths, &id, true);
                }
            }
            // `up` rebuilds when the service has a `build`, and starts
            // whatever is not running - which is exactly both actions,
            // so neither one needs its own code path. `restart` differs
            // only in that nothing was built, because nothing in the
            // build context changed.
            if let Err(e) = stack_up(paths, st, files) {
                eprintln!("rustainer: {name}: {e:#}");
            }
        }
    }
}

/// `compose run`: a one-off container with a service's settings.
///
/// Peers come from the containers that are ALREADY RUNNING, read from
/// their recorded addresses, rather than from a fresh allocation for
/// the whole stack. A one-off that invented new addresses for services
/// that are already up would be unable to reach any of them, which is
/// the only thing anybody runs one for.
fn stack_run(paths: &Paths, st: &stack::Stack, args: &StackRunArgs) -> Result<ExitCode> {
    let Some(svc) = st.services.get(&args.service) else {
        bail!(
            "this stack has no service called {:?}. It defines: {}",
            args.service,
            st.services.keys().cloned().collect::<Vec<_>>().join(", ")
        );
    };
    let mut add_host: Vec<String> = Vec::new();
    for other in st.services.keys() {
        for (n, id) in st.container_names(other).into_iter().enumerate() {
            let Ok(cfg) = Config::load(paths, &id) else {
                continue;
            };
            let Some(ip) = cfg.ip.clone() else { continue };
            if n == 0 {
                add_host.push(format!("{other}:{ip}"));
            }
            add_host.push(format!("{id}:{ip}"));
        }
    }
    // A name of its own, so it cannot collide with the service's own
    // container and `ps` shows plainly what it is.
    let id = format!("{}-{}-run-{}", st.name, args.service, new_id()?);
    let image = if svc.image.trim().is_empty() {
        format!("{}-{}:latest", st.name, args.service)
    } else {
        svc.image.clone()
    };
    let mut command: Vec<OsString> = vec![image.into()];
    if args.command.is_empty() {
        command.extend(svc.command.iter().map(OsString::from));
    } else {
        command.extend(args.command.iter().cloned());
    }
    let run = RunArgs {
        name: Some(id.clone()),
        detach: false,
        network: svc.network.clone().unwrap_or_else(|| "bridge".into()),
        hostname: args.service.clone(),
        workdir: svc.workdir.clone(),
        volumes: {
            // A one-off gets the service's configs and secrets too: the
            // whole point of it is to be the service, with a different
            // command.
            let base_dir = args
                .file
                .first()
                .and_then(|f| f.parent())
                .unwrap_or(Path::new("."));
            let mut v = svc.volumes.clone();
            for (list, store, dest) in [
                (&svc.configs, &st.configs, ""),
                (&svc.secrets, &st.secrets, "/run/secrets"),
            ] {
                for n in list {
                    let src = base_dir.join(&store[n].file);
                    v.push(format!("{}:{dest}/{n}:ro", src.display()));
                }
            }
            v
        },
        add_host,
        stack_name: Some(st.name.clone()),
        service_name: Some(args.service.clone()),
        memory: svc.memory.clone(),
        cpus: svc.cpus,
        pids_limit: svc.pids_limit,
        user: svc.user.clone(),
        env: svc.env.clone(),
        command,
        ..Default::default()
    };
    let code = cmd_run(paths, run).with_context(|| {
        // The container is removed below whether or not it ran, so a
        // failure here does not leave a stopped one-off lying around
        // with a name nobody will recognise tomorrow.
        format!("running a one-off {:?}", args.service)
    });
    let _ = supervise::remove(paths, &id, true);
    code
}

fn stack_up(paths: &Paths, st: &stack::Stack, files: &[PathBuf]) -> Result<ExitCode> {
    let order = st.start_order()?;

    // Anything that has to be BUILT is built first, before a single
    // address is allocated or a single container starts. A stack that
    // starts three services and then fails to build the fourth has left
    // the operator worse off than one that refused at the start.
    for name in &order {
        let svc = &st.services[name];
        let Some(ctx) = &svc.build else { continue };
        // Relative to the stack FILE, not the working directory.
        let base = files
            .first()
            .and_then(|f| f.parent())
            .unwrap_or(Path::new("."));
        let context = base.join(ctx);
        let tag = if svc.image.trim().is_empty() {
            format!("{}-{name}:latest", st.name)
        } else {
            svc.image.clone()
        };
        println!("building {tag} from {}", context.display());
        // The exit code is the CLI's and is success here; a build that
        // failed comes back as an Err and is propagated. Discarded
        // explicitly so it is clear that is why, as with `cmd_run`.
        let _ = cmd_build(
            paths,
            BuildArgs {
                tag: tag.clone(),
                file: None,
                build_args: Vec::new(),
                network: svc.network.clone().unwrap_or_else(|| "bridge".into()),
                no_cache: false,
                context,
            },
        )
        .with_context(|| format!("building service {name:?}"))?;
    }

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
        for (n, id) in st.container_names(name).into_iter().enumerate() {
            // A container that already exists keeps the address it has.
            // Allocating a fresh one for it here put an address nothing
            // answers on into the /etc/hosts of every service started
            // afterwards - `up` on a half-running stack produced
            // services that could not find the ones already running.
            // The same holds for a container set aside by `update`:
            // its replacement takes over its address, so nothing that
            // already resolved it has to be told.
            let recorded = [id.clone(), set_aside(&id)]
                .iter()
                .find_map(|c| Config::load(paths, c).ok()?.ip);
            let ip = match recorded {
                Some(ip) => ip,
                None => rustainer_core::net::allocate(&paths_root(paths))
                    .with_context(|| format!("allocating an address for {id}"))?
                    .ip
                    .to_string(),
            };
            // Both names resolve: the container name, which is what `ps`
            // shows, and - for the FIRST replica - the service's own
            // short name, which is what a config file inside a container
            // will say.
            //
            // Replicas are where /etc/hosts runs out. A name can appear
            // more than once in that file, but every resolver takes the
            // first match, so the service name reaches replica 1 and
            // nothing round-robins. Each replica is reachable by its own
            // name; load balancing needs a resolver, and there is no
            // daemon here to be one. Said plainly in the README rather
            // than left for somebody to discover under load.
            if n == 0 {
                peers.push((name.clone(), ip.clone()));
            }
            peers.push((id.clone(), ip.clone()));
            address.insert(id, ip);
        }
    }

    // Relative to the stack file, like `build` and `.env`, so the same
    // file means the same thing from any working directory.
    let base_dir = files
        .first()
        .and_then(|f| f.parent())
        .unwrap_or(Path::new("."))
        .to_path_buf();
    // Checked once, before anything starts. A secret in a file the whole
    // machine can read is the most common way one leaks, and a tool that
    // mounts it without comment has helped it happen.
    for (name, src) in &st.secrets {
        let path = base_dir.join(&src.file);
        let md = std::fs::metadata(&path)
            .with_context(|| format!("secret {name:?}: reading {}", path.display()))?;
        let mode = std::os::unix::fs::PermissionsExt::mode(&md.permissions());
        if mode & 0o077 != 0 {
            bail!(
                "secret {name:?} is {} with mode {:o}, which lets somebody other than \
                 its owner read it. `chmod 600 {}` first.",
                path.display(),
                mode & 0o7777,
                path.display()
            );
        }
    }
    for (name, src) in &st.configs {
        let path = base_dir.join(&src.file);
        if !path.exists() {
            bail!("config {name:?}: {} does not exist", path.display());
        }
    }

    // Placement, checked for the whole stack before anything starts: a
    // stack that brings up its database and then finds the application
    // may not run here has left more behind than one that refused.
    let hostname = std::fs::read_to_string("/proc/sys/kernel/hostname").unwrap_or_default();
    for name in &order {
        for c in &st.services[name].placement {
            let parsed = stack::Constraint::parse(c).map_err(|e| anyhow::anyhow!(e))?;
            if !parsed.holds(
                hostname.trim(),
                std::env::consts::OS,
                std::env::consts::ARCH,
            ) {
                bail!(
                    "service {name:?} may not run on this machine: its placement \
                     constraint {c:?} does not hold here (hostname {:?}, {} {}). There \
                     is no other node to place it on.",
                    hostname.trim(),
                    std::env::consts::OS,
                    std::env::consts::ARCH
                );
            }
        }
    }

    let mut started = 0usize;
    let mut wanted = 0usize;
    for name in &order {
        for id in st.container_names(name) {
            wanted += 1;
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
            let image = if svc.image.trim().is_empty() {
                format!("{}-{name}:latest", st.name)
            } else {
                svc.image.clone()
            };
            let mut command: Vec<OsString> = vec![image.into()];
            command.extend(svc.command.iter().map(OsString::from));
            let args = RunArgs {
                rootfs: None,
                pull: false,
                detach: true,
                name: Some(id.clone()),
                network: svc.network.clone().unwrap_or_else(|| "bridge".into()),
                hostname: name.clone(),
                workdir: svc.workdir.clone(),
                volumes: {
                    // Configs and secrets are read-only binds, which is
                    // all they are anywhere - Compose mounts them the
                    // same way outside swarm. What differs is where
                    // they land and what is checked before they do.
                    let mut v = svc.volumes.clone();
                    for (list, store, dest) in [
                        (&svc.configs, &st.configs, ""),
                        (&svc.secrets, &st.secrets, "/run/secrets"),
                    ] {
                        for n in list {
                            let src = base_dir.join(&store[n].file);
                            v.push(format!("{}:{dest}/{n}:ro", src.display()));
                        }
                    }
                    v
                },
                add_host: peers
                    .iter()
                    .filter(|(n, _)| n != name && *n != id)
                    .map(|(n, ip)| format!("{n}:{ip}"))
                    .collect(),
                ip: address.get(&id).cloned(),
                stack_name: Some(st.name.clone()),
                service_name: Some(name.clone()),
                memory: svc.memory.clone(),
                cpus: svc.cpus,
                pids_limit: svc.pids_limit,
                user: svc.user.clone(),
                env: svc.env.clone(),
                publish: svc.ports.clone(),
                restart: svc.restart.clone().unwrap_or_else(|| "no".into()),
                command,
                ..Default::default()
            };
            // The exit code is the CLI's, and for a detached run it is always
            // success - the interesting failure comes back as an Err and is
            // propagated. Discarded explicitly so it is clear that is why.
            let _ = cmd_run(paths, args).with_context(|| format!("starting {id}"))?;
            started += 1;

            // If anything downstream waits for THIS service to be healthy,
            // the wait happens here, before the next service is started.
            // Checked against the services that depend on it rather than
            // against its own healthcheck, so a service that defines one
            // nobody waits on does not slow the stack down.
            let waited_on = st
                .services
                .iter()
                .any(|(_, other)| other.depends_on.wants_healthy(name));
            if waited_on && let Some(h) = &svc.healthcheck {
                wait_until_healthy(paths, &id, name, h)?;
            }
            // And then a watcher, for every service that defines a
            // healthcheck rather than only those something waits on.
            // The wait above answers "may the next service start"; this
            // answers "is it still well", which is the question an
            // operator asks of a page at three in the morning.
            if let Some(h) = &svc.healthcheck {
                let mut w = std::process::Command::new("/proc/self/exe");
                w.arg("--root")
                    .arg(paths_root(paths))
                    .arg("health-watch")
                    .arg(&id)
                    .arg("--interval")
                    .arg(h.interval.to_string())
                    .arg("--retries")
                    .arg(h.retries.to_string());
                w.arg("--");
                for t in &h.test {
                    w.arg(t);
                }
                // Detached on purpose: it outlives `stack up`, and it
                // exits by itself once the container is gone.
                let _ = w
                    .stdin(std::process::Stdio::null())
                    .stdout(std::process::Stdio::null())
                    .stderr(std::process::Stdio::null())
                    .spawn();
            }
        }
    }
    println!(
        "{} container(s) started, {} already running",
        started,
        wanted - started
    );
    Ok(ExitCode::SUCCESS)
}

/// The name a container is kept under while its replacement is tried.
fn set_aside(id: &str) -> String {
    format!("{id}-rollback")
}

/// Where a container's two directories live: its state, and its
/// writable layer. Moving both is moving the container.
fn container_dirs(paths: &Paths, id: &str) -> [PathBuf; 2] {
    [
        paths.dir(id),
        Store::default_root().join("containers").join(id),
    ]
}

fn move_container(paths: &Paths, from: &str, to: &str) -> Result<()> {
    for (a, b) in container_dirs(paths, from)
        .iter()
        .zip(container_dirs(paths, to))
    {
        if a.exists() {
            std::fs::rename(a, &b)
                .with_context(|| format!("moving {} to {}", a.display(), b.display()))?;
        }
    }
    Ok(())
}

fn stack_update(
    paths: &Paths,
    st: &stack::Stack,
    files: &[PathBuf],
    service: &str,
) -> Result<ExitCode> {
    let Some(svc) = st.services.get(service) else {
        bail!(
            "this stack has no service called {service:?}. It defines: {}",
            st.services.keys().cloned().collect::<Vec<_>>().join(", ")
        );
    };
    // Without one there is nothing to decide by. "It started" is not
    // evidence that it works, and rolling back on a guess is worse than
    // not offering to.
    let Some(health) = &svc.healthcheck else {
        bail!(
            "service {service:?} has no healthcheck, and an update is rolled back by \
             one. Add `[services.{service}.healthcheck]`, or use `stack down` and \
             `stack up` to replace it unconditionally."
        );
    };
    for id in st.container_names(service) {
        let aside = set_aside(&id);
        if container_dirs(paths, &aside).iter().any(|d| d.exists()) {
            bail!(
                "{aside} is left over from an update that did not finish. It is the \
                 previous {id}: delete {} to discard it, or move it back to keep it.",
                container_dirs(paths, &aside)
                    .iter()
                    .map(|d| d.display().to_string())
                    .collect::<Vec<_>>()
                    .join(" and ")
            );
        }
    }

    for id in st.container_names(service) {
        let aside = set_aside(&id);
        let had_old = paths.dir(&id).exists();
        if had_old {
            supervise::stop(paths, &id, Duration::from_secs(10))
                .with_context(|| format!("stopping {id}"))?;
            // Moved, not copied and not renamed in its config: the two
            // directories are the container, and moving them back is
            // an exact undo.
            move_container(paths, &id, &aside)?;
        }
        println!("{id}  replacing");
        let tried = stack_up(paths, st, files)
            .map(|_| ())
            .and_then(|()| wait_until_healthy(paths, &id, service, health));
        match tried {
            Ok(()) => {
                for d in container_dirs(paths, &aside) {
                    let _ = std::fs::remove_dir_all(d);
                }
                rustainer_core::events::record(paths, "update", &id, serde_json::json!({}));
                println!("{id}  updated");
            }
            Err(e) => {
                if paths.dir(&id).exists() {
                    let _ = supervise::remove(paths, &id, true);
                }
                if had_old {
                    move_container(paths, &aside, &id)?;
                    let _ = cmd_start(paths, &id).with_context(|| {
                        format!("the update failed AND the previous {id} would not start again")
                    })?;
                }
                rustainer_core::events::record(
                    paths,
                    "rollback",
                    &id,
                    serde_json::json!({ "why": format!("{e:#}") }),
                );
                bail!(
                    "{id} was rolled back: {e:#}. {}",
                    if had_old {
                        "The previous container is running again."
                    } else {
                        "There was no previous container to restore."
                    }
                );
            }
        }
    }
    Ok(ExitCode::SUCCESS)
}

/// One run of a healthcheck command inside the container.
///
/// Run as a CHILD PROCESS - `/proc/self/exe exec …` - and not by
/// calling `exec::exec` here. That is not a style choice. `exec` joins
/// the container's user and pid namespaces with `setns`, and `setns`
/// never moves the caller: it moves the caller's next CHILD. So an
/// in-process probe leaves `stack up` permanently attached to the
/// namespaces of whichever container it probed, and the next service it
/// starts is created inside them. The symptom is the next container
/// failing with ENOENT for paths that plainly exist, long after the
/// probe that caused it.
///
/// This is the same rule the API server follows for the same reason:
/// the process that supervises must be one that has done nothing to
/// itself first.
///
/// Output goes to /dev/null: a probe runs up to `retries` times and a
/// chatty one would bury the operator in the same three lines. What it
/// printed is reachable by running the command itself, which is in the
/// stack file.
fn health_probe(paths: &Paths, id: &str, argv: &[String]) -> Result<i32> {
    let status = std::process::Command::new("/proc/self/exe")
        .arg("--root")
        .arg(paths_root(paths))
        .arg("exec")
        .arg(id)
        .args(argv)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .context("running the healthcheck")?;
    Ok(status.code().unwrap_or(-1))
}

/// Run a service's healthcheck until it passes, or give up and say so.
///
/// The probe is `exec` in the container, which is the same mechanism a
/// person would use by hand - so a healthcheck that passes here passes
/// when they try it, and a failing one can be debugged with the command
/// in the file and nothing else.
fn wait_until_healthy(
    paths: &Paths,
    id: &str,
    service: &str,
    h: &stack::HealthCheck,
) -> Result<()> {
    if h.start_period > 0 {
        std::thread::sleep(Duration::from_secs(h.start_period));
    }
    let mut last: Option<String> = None;
    for attempt in 1..=h.retries {
        // A container that has died is never going to become healthy,
        // and waiting out thirty retries to say so wastes the operator's
        // time at exactly the moment they are watching.
        if let Ok(state) = State::load(paths, id)
            && !matches!(state.observed_status(), Status::Running | Status::Created)
        {
            bail!(
                "service {service:?} exited while its healthcheck was being waited on \
                 (status {}). `rustainer logs {id}` has its output.",
                state.observed_status()
            );
        }
        match health_probe(paths, id, &h.test) {
            Ok(0) => {
                if attempt > 1 {
                    println!("{id}  healthy after {attempt} attempts");
                }
                return Ok(());
            }
            Ok(code) => last = Some(format!("exit status {code}")),
            Err(e) => last = Some(e.to_string()),
        }
        std::thread::sleep(Duration::from_secs(h.interval));
    }
    bail!(
        "service {service:?} never became healthy: {} attempts of {:?}, {} every {}s. \
         Last: {}",
        h.retries,
        h.test,
        "retried",
        h.interval,
        last.unwrap_or_else(|| "no attempt ran".into())
    )
}

fn stack_down(paths: &Paths, st: &stack::Stack, timeout: u64) -> Result<ExitCode> {
    // Reverse of the start order: a service is stopped before the things
    // it depends on, which is the only order in which shutting down looks
    // like anything other than a cascade of connection errors.
    //
    // By LEVEL, though, not one service at a time. Services in a level
    // are independent by definition, so making them queue up behind each
    // other multiplies the grace period by their number - and the grace
    // period is nearly all of what `down` costs, because PID 1 in a new
    // PID namespace ignores a SIGTERM it has no handler for. Measured
    // against `docker compose down` on three independent services this
    // was the whole difference: 3.1s against 1.3s, for no reason except
    // that we were waiting three times instead of once.
    let mut levels = st.start_levels()?;
    levels.reverse();
    for level in levels {
        let ids: Vec<String> = level
            .iter()
            .flat_map(|n| st.container_names(n))
            .filter(|id| paths.dir(id).exists())
            .collect();
        // Results are collected and printed after the level finishes:
        // threads printing as they go interleave their lines, and a
        // shutdown report that arrives shuffled is worse than one that
        // arrives a moment later.
        let results: Vec<(String, Option<String>)> = std::thread::scope(|sc| {
            let handles: Vec<_> = ids
                .iter()
                .map(|id| {
                    sc.spawn(move || {
                        let _ = supervise::stop(paths, id, Duration::from_secs(timeout));
                        let err = supervise::remove(paths, id, true)
                            .err()
                            .map(|e| e.to_string());
                        (id.clone(), err)
                    })
                })
                .collect();
            handles.into_iter().filter_map(|h| h.join().ok()).collect()
        });
        for (id, err) in results {
            match err {
                None => println!("{id}  removed"),
                Some(e) => eprintln!("rustainer: {id}: {e}"),
            }
        }
    }
    Ok(ExitCode::SUCCESS)
}

fn stack_ps(paths: &Paths, st: &stack::Stack) -> Result<ExitCode> {
    println!(
        "{:<24} {:<9} {:<8} {:<15} COMMAND",
        "CONTAINER", "STATUS", "PID", "ADDRESS"
    );
    for name in st.start_order()? {
        for id in st.container_names(&name) {
            // The row is named after the CONTAINER, not the service, because
            // with replicas a service is several of them and a table that
            // prints the same name three times is not a table.
            let name = &id;
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
    }
    Ok(ExitCode::SUCCESS)
}

/// Where a container's current health is written.
///
/// A file beside the container, like the stop sentinel, and for the
/// same reason: two processes write the container's state and the
/// supervisor rewrites `state.json` wholesale from its own copy, so a
/// field there would be erased within milliseconds.
pub(crate) fn health_file(paths: &Paths, id: &str) -> PathBuf {
    paths.dir(id).join("health")
}

/// Run a container's healthcheck until the container goes away.
///
/// `starting` until the first pass, then `healthy` or `unhealthy`,
/// which is the same vocabulary Docker uses and means the same thing:
/// `starting` is "has not passed yet and has not run out of attempts",
/// not "unknown".
fn cmd_health_watch(
    paths: &Paths,
    id: &str,
    interval: u64,
    retries: u64,
    test: &[String],
) -> Result<ExitCode> {
    let _ = sys::set_process_name("rustainer-hc");
    let file = health_file(paths, id);
    let mut failures = 0u64;
    let mut state = "starting";
    let _ = std::fs::write(&file, "starting");
    loop {
        // The container's directory going away is the signal to stop.
        // Nothing has to tell this process anything: `rm` deletes the
        // directory, and a watcher writing into a directory that no
        // longer exists would recreate it.
        if !paths.dir(id).exists() {
            return Ok(ExitCode::SUCCESS);
        }
        match State::load(paths, id) {
            Ok(st) if matches!(st.observed_status(), Status::Running | Status::Created) => {}
            _ => {
                let _ = std::fs::remove_file(&file);
                return Ok(ExitCode::SUCCESS);
            }
        }
        let next = match health_probe(paths, id, test) {
            Ok(0) => {
                failures = 0;
                "healthy"
            }
            _ => {
                failures += 1;
                // `starting` only until the attempts run out: a probe
                // that has never passed is not the same as one that has
                // started failing, and reporting a container unhealthy
                // in its first second is how healthchecks get ignored.
                if state == "starting" && failures < retries {
                    "starting"
                } else {
                    "unhealthy"
                }
            }
        };
        if next != state {
            state = next;
            let _ = std::fs::write(&file, state);
        }
        std::thread::sleep(Duration::from_secs(interval.max(1)));
    }
}

fn cmd_forward(fd: i32, to: &str) -> Result<ExitCode> {
    use rustainer_core::firewall::Publish;
    use std::os::fd::FromRawFd;
    let (a, p) = to
        .rsplit_once(':')
        .ok_or_else(|| anyhow::anyhow!("{to:?} is not address:port"))?;
    let container_ip: std::net::Ipv4Addr = a
        .parse()
        .map_err(|_| anyhow::anyhow!("{a:?} is not an IPv4 address"))?;
    let container_port: u16 = p
        .parse()
        .map_err(|_| anyhow::anyhow!("{p:?} is not a port"))?;
    // SAFETY: the supervisor bound this listener and dup'd it onto this
    // descriptor immediately before the execve that produced us.
    let listener = unsafe { std::net::TcpListener::from_raw_fd(fd) };
    let local = listener.local_addr()?;
    let (host_ip, host_port) = match local {
        std::net::SocketAddr::V4(v4) => (*v4.ip(), v4.port()),
        _ => bail!("the inherited listener is not IPv4"),
    };
    let mut f = rustainer_core::proxy::Forwarder::from_listener(
        listener,
        Publish {
            host_ip,
            host_port,
            container_ip,
            container_port,
        },
    )
    .with_context(|| format!("forwarding {local} to {to}"))?;
    // Until killed. The supervisor owns this process and ends it when the
    // container's ports are torn down.
    loop {
        std::thread::park();
        let _ = &mut f;
    }
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

/// A volume's directory, from a name that is checked to be only a name:
/// `volume rm ../../etc` must not delete anything.
fn volume_dir(name: &str, must_exist: bool) -> Result<PathBuf> {
    if name.contains('/') || name.contains("..") || name.is_empty() || name.starts_with('.') {
        bail!("{name:?} is not a volume name");
    }
    let p = volumes_root().join(name);
    if must_exist && !p.is_dir() {
        bail!("no such volume: {name}");
    }
    Ok(p)
}

fn cmd_volume(paths: &Paths, cmd: VolumeCmd) -> Result<ExitCode> {
    let root = volumes_root();
    match cmd {
        VolumeCmd::Create { name } => {
            let p = volume_dir(&name, false)?;
            if p.exists() {
                bail!("volume {name} already exists");
            }
            std::fs::create_dir_all(&p).with_context(|| format!("creating {}", p.display()))?;
            println!("{name}");
        }
        VolumeCmd::Empty { name } => {
            let p = volume_dir(&name, true)?;
            // Refused rather than done under a running process: a
            // database whose files vanish while it holds them open does
            // not fail cleanly, it corrupts whatever it writes next.
            let users: Vec<String> = paths
                .list()
                .unwrap_or_default()
                .into_iter()
                .filter(|id| {
                    State::load(paths, id).is_ok_and(|s| s.observed_status() == Status::Running)
                        && Config::load(paths, id)
                            .is_ok_and(|c| c.volumes.iter().any(|v| v.source == p))
                })
                .collect();
            if !users.is_empty() {
                bail!(
                    "volume {name} is mounted by running container(s): {}. Stop them first.",
                    users.join(", ")
                );
            }
            for e in std::fs::read_dir(&p)?.flatten() {
                let path = e.path();
                let r = if e.file_type().is_ok_and(|t| t.is_dir()) {
                    std::fs::remove_dir_all(&path)
                } else {
                    std::fs::remove_file(&path)
                };
                r.with_context(|| format!("removing {}", path.display()))?;
            }
            println!("{name}");
        }
        VolumeCmd::Clone { from, to } => {
            let src = volume_dir(&from, true)?;
            let dst = volume_dir(&to, false)?;
            if dst.exists() {
                bail!("volume {to} already exists");
            }
            std::fs::create_dir_all(&dst)?;
            // ponytail: `cp -a` is the copy that already preserves owners,
            // modes, links and sparse files; reimplementing it is a bug farm.
            let ok = std::process::Command::new("cp")
                .arg("-a")
                .arg(src.join("."))
                .arg(&dst)
                .status()
                .context("running cp")?
                .success();
            if !ok {
                let _ = std::fs::remove_dir_all(&dst);
                bail!("copying {from} to {to} failed");
            }
            println!("{to}");
        }
        VolumeCmd::Export { name, output } => {
            let p = volume_dir(&name, true)?;
            let out: Box<dyn std::io::Write> = match &output {
                Some(f) => Box::new(
                    std::fs::File::create(f)
                        .with_context(|| format!("creating {}", f.display()))?,
                ),
                None => Box::new(std::io::stdout().lock()),
            };
            let mut tar = tar::Builder::new(out);
            // A link is archived as a link. Following it would export
            // whatever it points at on the HOST.
            tar.follow_symlinks(false);
            tar.append_dir_all(".", &p)
                .context("archiving the volume")?;
            tar.finish()?;
        }
        VolumeCmd::Import { name, input } => {
            let p = volume_dir(&name, false)?;
            std::fs::create_dir_all(&p)?;
            let reader: Box<dyn Read> = match &input {
                Some(f) => Box::new(
                    std::fs::File::open(f).with_context(|| format!("opening {}", f.display()))?,
                ),
                None => Box::new(std::io::stdin().lock()),
            };
            // The image extractor, because an archive is hostile input
            // whoever made it: it resolves every path inside the volume
            // and refuses entries that would land outside.
            let safe = rustainer_image::extract::SafeRoot::open(&p)?;
            let mut report = rustainer_image::extract::Report::default();
            rustainer_image::extract::apply_layer(
                &safe,
                reader,
                rustainer_image::extract::Limits::default(),
                false,
                &mut report,
            )
            .context("unpacking the archive")?;
            println!("{name}");
        }
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
            let p = volume_dir(&name, true)?;
            std::fs::remove_dir_all(&p).with_context(|| format!("removing the volume {name}"))?;
            println!("{name}");
        }
    }
    Ok(ExitCode::SUCCESS)
}

fn cmd_push(paths: &Paths, args: PushArgs) -> Result<ExitCode> {
    let target = args.to.clone().unwrap_or_else(|| args.image.clone());
    // The flag loses to the environment variable deliberately: a password
    // on a command line is in `ps` for everyone on the machine and in the
    // shell history forever.
    let creds = std::env::var("RUSTAINER_REGISTRY_AUTH")
        .ok()
        .filter(|s| !s.is_empty())
        .or(args.creds);
    let store = Store::new(Store::default_root());
    let digest = store
        // Printed verbatim: the message already says whether anything
        // was uploaded, and a wrapper that prefixes "pushing" onto
        // "is already there" is a progress report that contradicts
        // itself.
        .push(&args.image, &target, creds, |what| println!("{what}"))
        .with_context(|| format!("pushing {} to {target}", args.image))?;
    rustainer_core::events::record(
        paths,
        "push",
        &target,
        serde_json::json!({ "digest": digest }),
    );
    println!("{digest}");
    println!("pushed {target}");
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

fn cmd_scan(image: &str, as_json: bool) -> Result<ExitCode> {
    use rustainer_image::osv;
    let store = Store::new(Store::default_root());
    let local = store
        .local(image)
        .with_context(|| format!("{image} is not in the local store. Pull it first."))?;
    // Topmost layer first: the package database an upper layer wrote
    // is the one the container would see.
    let roots: Vec<String> = local
        .layers
        .iter()
        .rev()
        .map(|p| p.to_string_lossy().into_owned())
        .collect();
    let os_release = read_from_layers(&roots, "etc/os-release");
    let os_db = |roots: &[String]| {
        let apk = osv::parse_apk(&read_from_layers(roots, "lib/apk/db/installed"));
        if apk.is_empty() {
            osv::parse_dpkg(&read_from_layers(roots, "var/lib/dpkg/status"))
        } else {
            apk
        }
    };
    let os_packages = os_db(&roots);
    // Which layer brought a package in: each layer's own copy of the
    // database lists everything installed up to that point, so the
    // lowest one that names a package at this version is where it came
    // from - and the place to fix it.
    let per_layer: Vec<Vec<osv::Package>> = local
        .layers
        .iter()
        .map(|l| os_db(&[l.to_string_lossy().into_owned()]))
        .collect();
    let app = osv::app_packages(&local.layers);

    // (ecosystem, packages), one lookup each.
    let mut sets: Vec<(String, Vec<osv::Package>)> = Vec::new();
    let os_ecosystem = osv::ecosystem(&os_release);
    if let (Some(eco), false) = (&os_ecosystem, os_packages.is_empty()) {
        sets.push((eco.clone(), os_packages.clone()));
    }
    for eco in ["PyPI", "npm"] {
        let ps: Vec<osv::Package> = app
            .iter()
            .filter(|(e, _, _)| *e == eco)
            .map(|(_, p, _)| p.clone())
            .collect();
        if !ps.is_empty() {
            sets.push((eco.to_owned(), ps));
        }
    }
    if sets.is_empty() {
        if !os_packages.is_empty() {
            bail!(
                "{image} has {} packages, but its /etc/os-release does not name a release \
                 OSV has advisories for (Alpine, Debian and Ubuntu are supported).",
                os_packages.len()
            );
        }
        bail!(
            "{image} has no apk or dpkg package database and no Python or Node packages, \
             so there is nothing to look up. That is not the same as having no \
             vulnerabilities."
        );
    }
    let total: usize = sets.iter().map(|(_, p)| p.len()).sum();
    let base = std::env::var("RUSTAINER_OSV_URL").unwrap_or_else(|_| "https://api.osv.dev".into());
    if !as_json {
        eprintln!(
            "sending {total} package names and versions ({}) to {base}",
            sets.iter()
                .map(|(e, _)| e.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        );
    }
    let mut findings: Vec<serde_json::Value> = Vec::new();
    let mut truncated = false;
    for (eco, packages) in &sets {
        let report = osv::scan(&base, eco, packages).map_err(|e| anyhow::anyhow!(e))?;
        truncated |= report["details_truncated"] == true;
        for mut f in report["findings"].as_array().cloned().unwrap_or_default() {
            let p = osv::Package {
                name: f["package"].as_str().unwrap_or("").to_owned(),
                version: f["version"].as_str().unwrap_or("").to_owned(),
            };
            let layer = if Some(eco) == os_ecosystem.as_ref() {
                per_layer.iter().position(|l| l.contains(&p)).map(|i| i + 1)
            } else {
                app.iter()
                    .find(|(e, q, _)| e == eco && *q == p)
                    .map(|(_, _, l)| *l)
            };
            f["ecosystem"] = serde_json::json!(eco);
            f["layer"] = serde_json::json!(layer);
            findings.push(f);
        }
    }
    findings.sort_by(|a, b| {
        b["score"]
            .as_f64()
            .unwrap_or(-1.0)
            .total_cmp(&a["score"].as_f64().unwrap_or(-1.0))
    });
    // One advisory is often filed twice - by the ecosystem's own
    // database and by GitHub's - under the same CVE. Sorted worst first,
    // so the copy that is kept is the one that carries a score.
    let mut seen = std::collections::BTreeSet::new();
    findings.retain(|f| {
        seen.insert((
            f["cve"]
                .as_str()
                .or(f["id"].as_str())
                .unwrap_or("")
                .to_owned(),
            f["package"].as_str().unwrap_or("").to_owned(),
            f["version"].as_str().unwrap_or("").to_owned(),
        ))
    });
    if as_json {
        println!(
            "{}",
            serde_json::json!({
                "ecosystem": sets.iter().map(|(e, _)| e.as_str()).collect::<Vec<_>>().join(", "),
                "packages": total,
                "layers": local.layers.len(),
                "findings": findings,
                "details_truncated": truncated,
            })
        );
        return Ok(ExitCode::SUCCESS);
    }
    println!(
        "{:<6} {:<22} {:<24} {:<20} {:<16} LAYER",
        "SCORE", "ID", "PACKAGE", "INSTALLED", "FIXED IN"
    );
    for f in &findings {
        println!(
            "{:<6} {:<22} {:<24} {:<20} {:<16} {}",
            f["score"]
                .as_f64()
                .map_or("-".into(), |s| format!("{s:.1}")),
            f["cve"].as_str().or(f["id"].as_str()).unwrap_or("?"),
            f["package"].as_str().unwrap_or("?"),
            f["version"].as_str().unwrap_or("?"),
            f["fixed"].as_str().unwrap_or("-"),
            f["layer"].as_u64().map_or("-".into(), |l| l.to_string()),
        );
    }
    println!("{total} packages, {} known vulnerabilities", findings.len());
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

// ---------------------------------------------------------------------------
// build
// ---------------------------------------------------------------------------

/// Build an image, running each filesystem step as a real container.
///
/// The step's own overlay **upper directory is the new layer**: overlayfs
/// has already written down exactly what changed, including the
/// deletions, which it records as the same `0:0` character devices this
/// engine's layer store uses. Nothing has to diff anything.
///
/// Which means the build inherits the engine's isolation, cgroups,
/// networking and security posture for free, because it is not a second
/// implementation of any of them.
/// Build, and write down how it went either way.
///
/// A build that fails is recorded as well as one that succeeds: a
/// history that only lists the builds that worked cannot answer the
/// question it is opened for, which is what went wrong.
fn cmd_build(paths: &Paths, args: BuildArgs) -> Result<ExitCode> {
    let started = std::time::Instant::now();
    let tag = args.tag.clone();
    let about = serde_json::json!({
        "context": args.context.canonicalize().unwrap_or(args.context.clone()).display().to_string(),
        "file": args.file.as_ref().map(|f| f.display().to_string()),
        "network": args.network,
    });
    let result = build_image(paths, args);
    let mut detail = about;
    detail["duration_ms"] = serde_json::json!(started.elapsed().as_millis() as u64);
    match &result {
        Ok((digest, steps, reused)) => {
            detail["digest"] = serde_json::json!(digest);
            detail["steps"] = serde_json::json!(steps);
            detail["reused"] = serde_json::json!(reused);
            rustainer_core::events::record(paths, "build", &tag, detail);
        }
        Err(e) => {
            detail["error"] = serde_json::json!(format!("{e:#}"));
            rustainer_core::events::record(paths, "build-failed", &tag, detail);
        }
    }
    result.map(|_| ExitCode::SUCCESS)
}

/// The build itself: `(image digest, steps, steps reused)`.
fn build_image(paths: &Paths, args: BuildArgs) -> Result<(String, usize, usize)> {
    use crate::dockerfile::Instruction;
    use std::collections::BTreeMap;

    let context = args.context.canonicalize().with_context(|| {
        format!(
            "the build context {} does not exist",
            args.context.display()
        )
    })?;
    let file = args
        .file
        .clone()
        .unwrap_or_else(|| context.join("Dockerfile"));
    let text =
        std::fs::read_to_string(&file).with_context(|| format!("reading {}", file.display()))?;
    let instructions = crate::dockerfile::parse(&text)?;

    let mut vars: BTreeMap<String, String> = BTreeMap::new();
    for a in &args.build_args {
        let (k, v) = a
            .split_once('=')
            .ok_or_else(|| anyhow::anyhow!("--build-arg {a:?} is not KEY=value"))?;
        vars.insert(k.to_owned(), v.to_owned());
    }

    let store = Store::new(Store::default_root());
    let mut built = build::Built {
        layers: Vec::new(),
        diff_ids: Vec::new(),
        env: Vec::new(),
        workdir: None,
        user: None,
        cmd: Vec::new(),
        entrypoint: Vec::new(),
        labels: BTreeMap::new(),
        exposed: Vec::new(),
    };
    // Layers are sealed as they are produced, and their tars kept, so the
    // image can be pushed later without rebuilding it.
    let mut sealed: Vec<(String, PathBuf, Vec<u8>)> = Vec::new();
    let mut base_diff_ids: Vec<String> = Vec::new();
    let mut base_descriptors: Vec<rustainer_image::registry::Descriptor> = Vec::new();
    let staging = Store::default_root().join("build").join(new_id()?);
    std::fs::create_dir_all(&staging)?;
    let cleanup_staging = || {
        let _ = std::fs::remove_dir_all(&staging);
    };

    // The cache. A RUN step's result depends on the filesystem it ran
    // over and on how it was run, so its key is a hash of both: `chain`
    // is the identity of everything beneath it - the base image, every
    // COPY by the content it copied, every RUN by its own key - and the
    // step adds its command, environment, working directory, user and
    // network. Change any of those, or anything earlier, and the key
    // changes; that is the whole invalidation rule.
    //
    // What a key cannot see is the outside world. `RUN apk add curl` is
    // one key whatever the package index says today, here as in Docker,
    // and `--no-cache` is the answer to that.
    let cache_dir = Store::default_root().join("build-cache");
    let hash = |parts: &[&str]| rustainer_image::registry::sha256_hex(parts.join("\0").as_bytes());
    let mut chain = String::new();
    let mut reused = 0usize;

    let total = instructions.len();
    for (n, ins) in instructions.iter().enumerate() {
        println!("[{}/{}] {}", n + 1, total, build::describe(ins));
        match ins {
            Instruction::Arg(k, default) => {
                // A --build-arg wins over the file's default, which is the
                // point of having a default.
                if !vars.contains_key(k)
                    && let Some(d) = default
                {
                    vars.insert(k.clone(), d.clone());
                }
            }
            Instruction::From { image, .. } => {
                if !built.layers.is_empty() {
                    cleanup_staging();
                    bail!(
                        "a second FROM starts a new build stage, which is not \
                         implemented. Multi-stage builds need a graph where this \
                         has a list; build the stages separately and COPY between \
                         images instead."
                    );
                }
                let reference = build::expand(image, &vars);
                let base = match store.local(&reference) {
                    Some(i) => i,
                    None => store
                        .pull(&reference, false)
                        .with_context(|| format!("pulling the base image {reference}"))?,
                };
                built.layers = base.layers.clone();
                base_diff_ids = base.diff_ids.clone();
                base_descriptors = store
                    .manifest_for(&reference)
                    .map(|m| m.layers)
                    .unwrap_or_default();
                if base_descriptors.len() != base.layers.len() {
                    bail!(
                        "the base image {reference} has no usable manifest in the \
                         store, so a build on it could not be pushed. Pull it again \
                         with `rustainer pull {reference} --force`."
                    );
                }
                built.diff_ids = Vec::new();
                built.env = base.env.clone();
                built.workdir = base.working_dir.clone();
                built.user = base.user.clone();
                built.cmd = base.cmd.clone();
                built.entrypoint = base.entrypoint.clone();
                chain = hash(&["from", &base.digest, &base_diff_ids.join(",")]);
            }
            Instruction::Env(k, v) => {
                let v = build::expand(v, &vars);
                built
                    .env
                    .retain(|e| e.split('=').next() != Some(k.as_str()));
                built.env.push(format!("{k}={v}"));
                // ENV also feeds later expansions, as Docker's does.
                vars.insert(k.clone(), v);
            }
            Instruction::Label(k, v) => {
                built.labels.insert(k.clone(), build::expand(v, &vars));
            }
            Instruction::Workdir(w) => built.workdir = Some(build::expand(w, &vars)),
            Instruction::User(u) => built.user = Some(build::expand(u, &vars)),
            Instruction::Expose(p) => built.exposed.push(build::expand(p, &vars)),
            Instruction::Cmd(c) => {
                built.cmd = c.iter().map(|x| build::expand(x, &vars)).collect();
            }
            Instruction::Entrypoint(c) => {
                built.entrypoint = c.iter().map(|x| build::expand(x, &vars)).collect();
            }
            Instruction::Copy {
                sources,
                dest,
                from,
            } => {
                if from.is_some() {
                    cleanup_staging();
                    bail!("COPY --from needs multi-stage builds, which are not implemented");
                }
                let dest = build::expand(dest, &vars);
                let layer_dir = staging.join(format!("layer{n}"));
                std::fs::create_dir_all(&layer_dir)?;
                // A trailing slash means "into this directory", exactly as
                // `cp` reads it, and several sources always do.
                let into_dir = dest.ends_with('/') || sources.len() > 1;
                for src in sources {
                    let src = build::expand(src, &vars);
                    let from_path = build::resolve_source(&context, &src)?;
                    let rel = dest.trim_start_matches('/');
                    let target = if into_dir {
                        layer_dir.join(rel).join(
                            from_path
                                .file_name()
                                .ok_or_else(|| anyhow::anyhow!("{src:?} has no name"))?,
                        )
                    } else {
                        layer_dir.join(rel)
                    };
                    build::copy_into(&from_path, &target)
                        .with_context(|| format!("copying {src} into the image"))?;
                }
                // A COPY is not looked up - doing it is as cheap as
                // checking - but what it copied is part of the identity
                // of every step after it, and the diff_id is exactly a
                // hash of that.
                build::settle_directory_times(&layer_dir);
                let (diff_id, tar) = build::seal_layer(&layer_dir)?;
                chain = hash(&[&chain, "copy", &diff_id]);
                let layer_dir = store.commit_layer(&diff_id, &layer_dir)?;
                built.layers.push(layer_dir.clone());
                built.diff_ids.push(diff_id.clone());
                sealed.push((diff_id, layer_dir, tar));
            }
            Instruction::Run(argv) => {
                let expanded: Vec<String> = argv.iter().map(|a| build::expand(a, &vars)).collect();
                let key = hash(&[
                    &chain,
                    "run",
                    &expanded.join("\u{1f}"),
                    &built.env.join("\u{1f}"),
                    built.workdir.as_deref().unwrap_or(""),
                    built.user.as_deref().unwrap_or(""),
                    &args.network,
                ]);
                chain = key.clone();
                let record = cache_dir.join(key.trim_start_matches("sha256:"));
                // A hit needs three things to agree: a record of what
                // this step produced, that layer still being in the
                // store, and the layer still hashing to what the record
                // says. The third is what stops a damaged or edited
                // layer being built on as if it were the step's result.
                let hit = (!args.no_cache)
                    .then(|| std::fs::read_to_string(&record).ok())
                    .flatten()
                    .map(|d| d.trim().to_owned())
                    .and_then(|d| {
                        let dir = store.layer_path(&d);
                        let (now, tar) = dir.is_dir().then(|| build::seal_layer(&dir).ok())??;
                        (now == d).then_some((d, dir, tar))
                    });
                if let Some((diff_id, dir, tar)) = hit {
                    println!("        cached");
                    reused += 1;
                    built.layers.push(dir.clone());
                    built.diff_ids.push(diff_id.clone());
                    sealed.push((diff_id, dir, tar));
                    continue;
                }
                let step_id = format!("build-{}", new_id()?);
                let mut command: Vec<OsString> =
                    vec![built.layers.first().cloned().unwrap_or_default().into()];
                command.clear();
                command.push(OsString::from("/"));
                // The step is an ordinary container over the layers so
                // far. Run in the foreground so the build waits, and with
                // the engine's normal isolation - a build step is not
                // entitled to more than a container is.
                let run = RunArgs {
                    name: Some(step_id.clone()),
                    layer: built.layers.clone(),
                    network: args.network.clone(),
                    hostname: "build".into(),
                    workdir: built.workdir.clone(),
                    user: built.user.clone(),
                    env: built.env.clone(),
                    command: expanded.iter().map(OsString::from).collect(),
                    ..Default::default()
                };
                let _ = cmd_run(paths, run).with_context(|| format!("running step {}", n + 1))?;

                let st = State::load(paths, &step_id)
                    .with_context(|| format!("reading the result of step {}", n + 1))?;
                let code = st.exit_code.unwrap_or(-1);
                let upper = Store::default_root()
                    .join("containers")
                    .join(&step_id)
                    .join("upper");
                if code != 0 {
                    let _ = supervise::remove(paths, &step_id, true);
                    cleanup_staging();
                    bail!(
                        "step {} failed with exit status {code}: {}",
                        n + 1,
                        build::describe(ins)
                    );
                }
                // The upper IS the layer. Moved out before the container
                // is removed, because `rm` takes its overlay directory
                // with it.
                let layer_dir = staging.join(format!("layer{n}"));
                std::fs::rename(&upper, &layer_dir)
                    .with_context(|| format!("harvesting the layer of step {}", n + 1))?;
                let _ = supervise::remove(paths, &step_id, true);

                let (diff_id, tar) = build::seal_layer(&layer_dir)?;
                // Committed and recorded now, not when the image is
                // tagged: if a later step fails, the next build still
                // starts from here.
                let layer_dir = store.commit_layer(&diff_id, &layer_dir)?;
                let _ = std::fs::create_dir_all(&cache_dir);
                let _ = std::fs::write(&record, &diff_id);
                built.layers.push(layer_dir.clone());
                built.diff_ids.push(diff_id.clone());
                sealed.push((diff_id, layer_dir, tar));
            }
        }
    }
    if reused > 0 {
        println!("{reused} step(s) reused from an earlier build");
    }

    if built.layers.is_empty() {
        cleanup_staging();
        bail!("the build produced no layers, so there is no image to tag");
    }

    // The base image's diff_ids come first, taken from the base image
    // rather than from its directory NAMES: the store strips the
    // `sha256:` prefix when naming a directory, so reading them back
    // gives a string that looks like a diff_id and is not one.
    let mut diff_ids: Vec<String> = base_diff_ids.clone();
    diff_ids.extend(built.diff_ids.iter().cloned());

    let config = serde_json::json!({
        "architecture": match std::env::consts::ARCH {
            "x86_64" => "amd64",
            "aarch64" => "arm64",
            other => other,
        },
        "os": "linux",
        "config": {
            "Env": built.env,
            "Cmd": built.cmd,
            "Entrypoint": built.entrypoint,
            "WorkingDir": built.workdir,
            "User": built.user,
            "Labels": built.labels,
            "ExposedPorts": built.exposed.iter()
                .map(|p| (format!("{p}/tcp"), serde_json::json!({})))
                .collect::<serde_json::Map<_, _>>(),
        },
        "rootfs": { "type": "layers", "diff_ids": diff_ids },
    });
    let config_bytes = serde_json::to_vec_pretty(&config)?;
    let digest = store
        .adopt(&base_descriptors, &sealed, &config_bytes, &args.tag)
        .context("storing the built image")?;
    cleanup_staging();

    println!("{digest}");
    println!("tagged {}", args.tag);
    Ok((digest, total, reused))
}
