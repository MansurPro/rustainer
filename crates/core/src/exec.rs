//! `exec`: a second process inside a container that already exists.
//!
//! Everything else in this crate *creates* namespaces. This module is the
//! only one that *joins* them, and the asymmetry between those two verbs is
//! the whole content of the module:
//!
//! - **`clone3` takes a flag word; `setns(2)` takes one descriptor at a
//!   time.** There is no atomic "join these seven". A failure half way
//!   through leaves the caller in a mixture, which is why the caller here is
//!   a process we are willing to throw away.
//! - **`setns(CLONE_NEWPID)` does not move the caller.** It sets where the
//!   caller's *next child* is created. Joining a pid namespace is therefore
//!   `setns` **then fork**, and the fork is not an implementation detail —
//!   it is the join.
//! - **`setns(CLONE_NEWUSER)` and `setns(CLONE_NEWNS)` refuse a
//!   multi-threaded caller** with `EINVAL`. The no-async rule this project
//!   adopted for `clone3`'s sake (RND.md §15.6) is what makes `exec`
//!   possible at all; a runtime with a thread pool would have had to
//!   re-exec a helper here.
//!
//! The split of who joins what is forced, not stylistic:
//!
//! | Namespace | Joined by | Why not the other one |
//! |---|---|---|
//! | `user` | parent | Everything below needs the capabilities it confers |
//! | `pid` | parent | The caller is never moved; only its next child is |
//! | `ipc`, `uts`, `net` | child | Nothing in the parent needs them |
//! | `mnt` | child | `setns` repoints the caller's root and cwd, and the parent still has to read `/proc/self/stat` |
//!
//! That last row is the one that bites. [`crate::spawn`] checks the
//! single-thread invariant by reading `/proc/self/stat`. Join the container's
//! mount namespace in the parent and `/proc` becomes the container's — mounted
//! against the container's pid namespace, which the parent is not in — so
//! `/proc/self` resolves to nothing and the check fails `ENOENT` while
//! reporting a thread-count problem. The parent keeps its own filesystem
//! view; the child joins last, immediately before it stops needing one.

use crate::state::{Paths, State, Status};
use crate::supervise::{Config, Error};
use crate::{Attach, Exit, Namespaces, Spec, cgroup, security::UserSpec, sys};

/// What to run inside the container, and as whom.
#[derive(Clone, Debug, Default)]
pub struct Request {
    /// `argv[0]` is the program, resolved inside the container.
    pub argv: Vec<String>,
    /// `KEY=VALUE` entries that win over the container's own.
    pub env: Vec<String>,
    /// Defaults to the container's working directory, then to `/`.
    pub workdir: Option<String>,
    /// Defaults to the user the container itself runs as.
    pub user: Option<UserSpec>,
}

/// The directories searched when the command is not an absolute path.
///
/// The same list the `run` path uses, searched in the same order — but
/// through `/proc/<init>/root`, which is the one view that is correct here.
/// A container's root is an overlay mounted *inside its own mount
/// namespace*, so the merged directory has no pathname on the host and
/// searching the image's layers on disk would miss everything the container
/// has written since it started.
const PATH_DIRS: [&str; 6] = [
    "/usr/local/sbin",
    "/usr/local/bin",
    "/usr/sbin",
    "/usr/bin",
    "/sbin",
    "/bin",
];

/// Run `req` inside container `id` and wait for it.
pub fn exec(paths: &Paths, id: &str, req: &Request) -> Result<Exit, Error> {
    if req.argv.is_empty() {
        return Err(Error::Io(std::io::Error::other("no command given")));
    }

    let state = State::load(paths, id)?;
    let observed = state.observed_status();
    let init = match (observed, state.init_pid) {
        (Status::Running, Some(pid)) => pid,
        _ => {
            return Err(Error::NotRunning {
                id: id.to_owned(),
                status: observed.as_oci(),
            });
        }
    };

    let mut config = Config::load(paths, id)?;
    if req.user.is_some() {
        config.user = req.user.clone();
    }
    // Asked and answered here, where the answer is a sentence. Deeper in,
    // it is `setresuid` returning EINVAL from a process that can no longer
    // allocate a message, reported as step 9 of a child that failed.
    if let Some(u) = &config.user
        && let Some(why) = u.unmapped_in(init)
    {
        return Err(Error::Io(std::io::Error::other(why)));
    }

    // Everything that needs the host's filesystem view happens here, above
    // the first `setns`. After the joins this process can still read its own
    // `/proc`, but `/sys/fs/cgroup` and the state directory are gone.
    let exe = resolve(init, &req.argv[0])?;
    let security = config.security()?;
    let cgroup_fd = cgroup::open_existing(init);

    let ns = config.to_spec().namespaces;
    // Opened before any join, for the same reason: `/proc/<init>/ns/*` is
    // reachable now and the descriptors stay valid afterwards.
    let parent_fds = open_namespaces(init, [("user", ns.user), ("pid", ns.pid)])?;
    let child_fds = open_namespaces(
        init,
        [
            ("ipc", ns.ipc),
            ("uts", ns.uts),
            ("net", ns.net),
            // Last. See the module comment.
            ("mnt", ns.mount),
        ],
    )?;

    for (name, fd) in &parent_fds {
        sys::setns(fd, 0).map_err(|source| Error::JoinNamespace { ns: name, source })?;
    }

    let envp = merge_env(&config.envp, &req.env);

    let spec = Spec {
        exe: exe.into(),
        argv: req.argv.iter().map(Into::into).collect(),
        envp: envp.into_iter().map(Into::into).collect(),
        hostname: None,
        // Always set, never left to chance: a child that does not `chdir`
        // keeps the directory the CLI was run in, which is a live reference
        // into the host filesystem from inside the container. The kernel
        // happens to close that hole for us — `setns(CLONE_NEWNS)` calls
        // `set_fs_pwd` — but relying on a side effect of the join for a
        // containment property is not a thing to do twice.
        workdir: Some(
            req.workdir
                .clone()
                .or_else(|| config.workdir.clone())
                .unwrap_or_else(|| "/".to_owned())
                .into(),
        ),
        net_messages: Vec::new(),
        // Nothing is created. Every namespace this process ends up in, it
        // was let into.
        namespaces: Namespaces::default(),
    };

    let mut spawned = crate::spawn(
        &spec,
        None,
        Attach {
            security: Some(security),
            // `None`, so the command inherits the terminal the CLI was given.
            stdio: None,
            exec_fifo: None,
            cgroup_fd: cgroup_fd.as_ref(),
            join: child_fds.into_iter().map(|(_, fd)| fd).collect(),
        },
    )?;

    // There is nothing for the parent to do between the clone and the
    // go-ahead: no uid map to write (we created no user namespace) and no
    // cgroup to join (`CLONE_INTO_CGROUP` did it). The rendezvous still
    // happens, because the child waits on it unconditionally.
    spawned.release();
    let failure = spawned.start_failure();
    let status = crate::reap_pid(spawned.pid);
    if let Some(e) = failure {
        return Err(e.into());
    }
    Ok(status?)
}

/// The container's environment, with the caller's entries replacing rather
/// than shadowing.
///
/// Prepending the caller's entries would also "work" — `getenv(3)` returns
/// the first match — but only by accident of glibc's implementation, and
/// `env` inside the container would print the variable twice, which is how
/// an operator debugging a container concludes the engine is broken.
fn merge_env(container: &[String], extra: &[String]) -> Vec<String> {
    let key = |e: &str| e.split('=').next().unwrap_or("").to_owned();
    let overridden: Vec<String> = extra.iter().map(|e| key(e)).collect();
    let mut out: Vec<String> = container
        .iter()
        .filter(|e| !overridden.contains(&key(e)))
        .cloned()
        .collect();
    out.extend(extra.iter().cloned());
    out
}

/// Open `/proc/<pid>/ns/<name>` for each namespace that is wanted.
fn open_namespaces<const N: usize>(
    pid: i32,
    wanted: [(&'static str, bool); N],
) -> Result<Vec<(&'static str, sys::OwnedFd)>, Error> {
    let mut out = Vec::new();
    for (name, yes) in wanted {
        if !yes {
            continue;
        }
        let path = std::ffi::CString::new(format!("/proc/{pid}/ns/{name}"))
            .map_err(|_| Error::Io(std::io::Error::other("nul in a proc path")))?;
        let fd = sys::open_namespace(&path)
            .map_err(|source| Error::JoinNamespace { ns: name, source })?;
        out.push((name, fd));
    }
    Ok(out)
}

/// Find the command inside the running container.
fn resolve(init: i32, cmd: &str) -> Result<String, Error> {
    if cmd.starts_with('/') {
        return Ok(cmd.to_owned());
    }
    if cmd.contains('/') {
        return Err(Error::NotFoundInContainer {
            cmd: cmd.to_owned(),
        });
    }
    for dir in PATH_DIRS {
        // `symlink_metadata`, not `is_file`: in a real image `/bin/sh` is a
        // symlink to `/bin/busybox`, whose absolute target only resolves
        // inside the container. Following it from here looks for the host's
        // busybox and concludes the command does not exist.
        if std::fs::symlink_metadata(format!("/proc/{init}/root{dir}/{cmd}")).is_ok() {
            return Ok(format!("{dir}/{cmd}"));
        }
    }
    Err(Error::NotFoundInContainer {
        cmd: cmd.to_owned(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_override_replaces_rather_than_shadows() {
        let out = merge_env(
            &["PATH=/bin".into(), "HOME=/root".into()],
            &["PATH=/zzz".into(), "NEW=1".into()],
        );
        assert_eq!(out, vec!["HOME=/root", "PATH=/zzz", "NEW=1"]);
        assert_eq!(out.iter().filter(|e| e.starts_with("PATH=")).count(), 1);
    }

    #[test]
    fn a_bare_name_with_no_equals_still_has_a_key() {
        // `env -u`-shaped input, and entries the image config may carry.
        let out = merge_env(&["TERM".into()], &["TERM=xterm".into()]);
        assert_eq!(out, vec!["TERM=xterm"]);
    }

    #[test]
    fn an_absolute_command_is_taken_as_given() {
        // No /proc read: the early return happens before any lookup, which
        // is what lets `exec <id> /bin/sh` work in a container whose
        // /proc is not mounted.
        assert_eq!(resolve(1, "/bin/sh").unwrap(), "/bin/sh");
    }

    #[test]
    fn a_relative_path_is_refused_rather_than_searched() {
        // `exec <id> ./configure` must not silently become /bin/./configure.
        assert!(matches!(
            resolve(1, "./configure"),
            Err(Error::NotFoundInContainer { .. })
        ));
    }
}
