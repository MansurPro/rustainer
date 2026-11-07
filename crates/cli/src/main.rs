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
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use anyhow::{Context, Result, bail};
use clap::{Args, Parser, Subcommand};
use rustainer_core::{Exit, Namespaces, Privileges, Spec, mount};

#[derive(Parser)]
#[command(
    name = "rustainer",
    version,
    about = "A container engine written in Rust from first principles.",
    long_about = "A container engine written in Rust from first principles - owning the \
                  runtime core rather than shelling out to runc, crun or containerd.\n\n\
                  This is stage 2 of 11: a process in namespaces of its own, with its own \
                  root filesystem. There is no image, no layering, no resource limit and \
                  no network isolation yet, and the flags below refuse rather than pretend."
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
    /// Root filesystem for the container: a directory tree, for example one
    /// extracted from an image tarball with `tar`. `/` keeps the host's
    /// filesystem view, which is stage 1 behaviour.
    #[arg(long, default_value = "/")]
    rootfs: PathBuf,

    /// Network mode. Only `host` is supported, and it is required
    /// explicitly so that sharing the host's network stack is a decision
    /// rather than a default.
    #[arg(long)]
    network: String,

    /// Hostname inside the container's UTS namespace.
    #[arg(long, default_value = "container")]
    hostname: OsString,

    /// The command to run, and its arguments. Resolved inside the
    /// container's root filesystem.
    #[arg(required = true, num_args = 1.., trailing_var_arg = true)]
    command: Vec<OsString>,
}

fn main() -> ExitCode {
    // RND.md §18.10, and the first thing this binary does. A container that
    // can write to `/proc/self/exe` overwrites the runtime binary itself -
    // CVE-2019-5736. Re-executing from a sealed memfd makes the path refer
    // to a copy nothing can modify.
    //
    // "We will design so that we never re-execute into the container, which
    // eliminates the whole class" is false and tempting: crun does
    // re-execute, and all three mature runtimes carry this mitigation
    // regardless of design.
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
    match cli.command {
        Command::Run(args) => run(args),
    }
}

fn run(args: RunArgs) -> Result<ExitCode> {
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

    let mut spec = Spec::new(exe, args.command);
    spec.hostname = Some(args.hostname);

    // `/` means "keep the host's filesystem view" - stage 1. Anything else
    // builds a mount set and pivots onto it.
    let mounts = if args.rootfs == Path::new("/") {
        spec.namespaces = Namespaces::STAGE1;
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
        let plan = mount::plan(&args.rootfs, privs).context("planning the container's mounts")?;

        // Say what is missing, and why, rather than producing a container
        // that is quietly less than it looks. "Detect and degrade silently"
        // is a lie told to the operator (RND.md §18.5).
        for d in &plan.degradations {
            eprintln!("rustainer: degraded: {} - {}", d.what, d.because);
        }

        spec.namespaces = Namespaces::STAGE2;
        Some(plan)
    };

    let exit = rustainer_core::run(&spec, mounts).context("running the container")?;

    // Report a signal death as a signal death. Collapsing it into an exit
    // code here would be the same class of lie as reporting an unobserved
    // exit as 0, and the whole ladder is built on not telling that kind.
    if let Exit::Signal(sig) = exit {
        eprintln!("rustainer: container killed by signal {sig}");
    }
    Ok(ExitCode::from(exit.to_shell_status().min(255) as u8))
}
