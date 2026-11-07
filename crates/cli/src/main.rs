//! The transient front end.
//!
//! This process parses arguments, hands a [`Spec`] to the core, and prints.
//! It owns no container state and no background work, because the engine is
//! daemonless by design: a per-container supervisor solves the one problem
//! the kernel actually forces (keeping a container's I/O attached after the
//! client detaches), and a daemon that owns containers cannot later have the
//! daemon removed, since process ancestry is not refactorable
//! (RND.md §18.1).

use std::ffi::OsString;
use std::process::ExitCode;

use anyhow::{Context, Result, bail};
use clap::{Args, Parser, Subcommand};
use rustainer_core::{Exit, Namespaces, Spec};

#[derive(Parser)]
#[command(
    name = "rustainer",
    version,
    about = "A container engine written in Rust from first principles.",
    long_about = "A container engine written in Rust from first principles - owning the \
                  runtime core rather than shelling out to runc, crun or containerd.\n\n\
                  This is stage 1 of 11: a process in namespaces of its own. There is no \
                  image, no root filesystem change, no resource limit and no network \
                  isolation yet, and the flags below refuse rather than pretend."
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Run a command in a new set of namespaces.
    Run(RunArgs),
}

#[derive(Args)]
struct RunArgs {
    /// Root filesystem for the container. Stage 1 supports only `/`.
    #[arg(long, default_value = "/")]
    rootfs: OsString,

    /// Network mode. Stage 1 supports only `host`, and requires it
    /// explicitly so that sharing the host's network stack is a decision
    /// rather than a default.
    #[arg(long)]
    network: String,

    /// Hostname inside the container's UTS namespace.
    #[arg(long, default_value = "container")]
    hostname: OsString,

    /// The command to run, and its arguments.
    #[arg(required = true, num_args = 1.., trailing_var_arg = true)]
    command: Vec<OsString>,
}

fn main() -> ExitCode {
    // ponytail: `/proc/self/exe` is not sealed into a memfd yet. RND.md
    // §18.10 wants `pentacle::ensure_sealed()` as the first statement here,
    // against the CVE-2019-5736 class in which a container overwrites the
    // runtime binary through `/proc/self/exe`. It is deferred exactly one
    // stage, on the research's own reasoning that the cost of adding it
    // later is low: the attack needs a container-controlled binary, and
    // until stage 2 brings `--rootfs <dir>` there is none. Add it with the
    // first commit of stage 2, before pivot_root works.

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
    match cli.command {
        Command::Run(args) => run(args),
    }
}

fn run(args: RunArgs) -> Result<ExitCode> {
    if args.rootfs != *"/" {
        bail!(
            "--rootfs {:?} is not supported yet. Stage 1 runs in the host's filesystem \
             view; giving the container its own root is stage 2, which needs a mount \
             namespace, a propagation change and pivot_root. Pass --rootfs / .",
            args.rootfs
        );
    }
    if args.network != "host" {
        bail!(
            "--network {} is not supported yet. Stage 1 shares the host's network \
             namespace, and the flag is mandatory so that is visible. A private network \
             stack is stage 8 and bridge networking is stage 9. Pass --network host .",
            args.network
        );
    }

    let exe = args.command[0].clone();
    if !std::path::Path::new(&exe).is_absolute() {
        bail!(
            "{exe:?} must be an absolute path. PATH is not searched: the search would \
             have to run in the child, between clone3 and execve, where it cannot \
             allocate."
        );
    }

    let mut spec = Spec::new(exe, args.command);
    spec.hostname = Some(args.hostname);
    spec.namespaces = Namespaces::STAGE1;

    let exit = rustainer_core::run(&spec).context("running the container")?;

    // Report a signal death as a signal death. Collapsing it into an exit
    // code here would be the same class of lie as reporting an unobserved
    // exit as 0, and the whole ladder is built on not telling that kind.
    if let Exit::Signal(sig) = exit {
        eprintln!("rustainer: container killed by signal {sig}");
    }
    Ok(ExitCode::from(exit.to_shell_status().min(255) as u8))
}
