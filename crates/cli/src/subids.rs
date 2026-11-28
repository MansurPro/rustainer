//! Sub-ids: being more than one user without being root.
//!
//! An unprivileged user's namespace maps exactly one id, their own, to
//! root. Everything in an image owned by anyone else collapses onto
//! that one id, `chown` to another user is impossible, and a process
//! cannot drop to a service account. `/etc/subuid` and `/etc/subgid`
//! exist to fix that: they grant a user a range of ids nobody else has,
//! and the setuid helpers `newuidmap` and `newgidmap` will write a map
//! that includes it. It is what rootless Docker and Podman both rest on.
//!
//! # The shape
//!
//! One user namespace per user, not per command. Namespaces a container
//! owns can only be entered from the user namespace that owns them or
//! its parent, so if every invocation made a fresh one, `exec` and
//! `stop` could never reach a container an earlier invocation started.
//! A small process holds the namespace open - Podman calls its
//! equivalent the pause process - and every later invocation joins it.
//!
//! # When none of this happens
//!
//! Already root; the helpers are not installed; the user has no range;
//! or `RUSTAINER_SUBIDS=0`. Then the engine carries on as it always
//! has, as one id, and nothing here is an error.

use std::io::{Read, Write};
use std::os::fd::AsRawFd;
use std::path::{Path, PathBuf};

/// Join this user's namespace, creating it first if need be. Called
/// before anything else looks at who the caller is, and while the
/// process still has one thread - `setns` to a user namespace needs
/// that.
pub fn enter() {
    let uid = rustix::process::geteuid().as_raw();
    let gid = rustix::process::getegid().as_raw();
    if uid == 0 || std::env::var_os("RUSTAINER_SUBIDS").is_some_and(|v| v == "0") {
        return;
    }
    let (Some(newuidmap), Some(newgidmap)) = (helper("newuidmap"), helper("newgidmap")) else {
        return;
    };
    let name = user_name(uid);
    let (Some(uids), Some(gids)) = (
        range("/etc/subuid", name.as_deref(), uid),
        range("/etc/subgid", name.as_deref(), gid),
    ) else {
        return;
    };

    // Where things are kept is decided by who the caller is, and that
    // is about to read as root. Pin the answers first.
    for (var, value) in [
        (
            "RUSTAINER_ROOT",
            rustainer_core::state::Paths::default_root(),
        ),
        (
            "RUSTAINER_IMAGE_ROOT",
            rustainer_image::Store::default_root(),
        ),
    ] {
        if std::env::var_os(var).is_none() {
            // SAFETY: single-threaded, at the very start of the program.
            unsafe { std::env::set_var(var, value) };
        }
    }

    let pidfile = match std::env::var_os("XDG_RUNTIME_DIR") {
        Some(d) => PathBuf::from(d).join("rustainer-userns.pid"),
        None => PathBuf::from(format!("/tmp/rustainer-{uid}-userns.pid")),
    };
    let recorded = std::fs::read_to_string(&pidfile)
        .ok()
        .and_then(|s| s.trim().parse::<i32>().ok());
    // What the caller reads as from here on is root; who they are is
    // kept for anything that has to say so.
    // SAFETY: single-threaded, at the very start of the program.
    unsafe { std::env::set_var("RUSTAINER_REAL_UID", uid.to_string()) };
    if recorded.is_some_and(|pid| join(pid, uid)) {
        return;
    }
    let Some(pid) = hold(uid, gid, uids, gids, &newuidmap, &newgidmap) else {
        // SAFETY: as above.
        unsafe { std::env::remove_var("RUSTAINER_REAL_UID") };
        return;
    };
    // Replaced, never written through: in /tmp the name is guessable,
    // and a link left there by someone else must not be followed.
    let _ = std::fs::remove_file(&pidfile);
    if let Ok(mut f) = std::fs::File::options()
        .write(true)
        .create_new(true)
        .open(&pidfile)
    {
        let _ = write!(f, "{pid}");
    }
    if !join(pid, uid) {
        // SAFETY: as above.
        unsafe { std::env::remove_var("RUSTAINER_REAL_UID") };
    }
}

fn helper(name: &str) -> Option<PathBuf> {
    ["/usr/bin", "/bin", "/usr/sbin", "/usr/local/bin"]
        .iter()
        .map(|d| Path::new(d).join(name))
        .find(|p| p.is_file())
}

fn user_name(uid: u32) -> Option<String> {
    let passwd = std::fs::read_to_string("/etc/passwd").ok()?;
    passwd.lines().find_map(|l| {
        let f: Vec<&str> = l.split(':').collect();
        (f.get(2)?.parse::<u32>().ok()? == uid).then(|| f[0].to_owned())
    })
}

/// The first range a file grants this user, by name or by number.
fn range(file: &str, name: Option<&str>, id: u32) -> Option<(u32, u32)> {
    parse_range(&std::fs::read_to_string(file).ok()?, name, id)
}

fn parse_range(text: &str, name: Option<&str>, id: u32) -> Option<(u32, u32)> {
    text.lines().find_map(|l| {
        let f: Vec<&str> = l.trim().split(':').collect();
        let [who, start, count] = f.as_slice() else {
            return None;
        };
        if Some(*who) != name && *who != id.to_string() {
            return None;
        }
        let (start, count) = (start.parse::<u32>().ok()?, count.parse::<u32>().ok()?);
        (count > 0).then_some((start, count.min(65536)))
    })
}

/// Enter the user namespace `pid` is holding. False for anything that
/// is not ours to enter: a stale file, another user's process, a
/// process that is no longer the holder.
fn join(pid: i32, uid: u32) -> bool {
    use std::os::unix::fs::MetadataExt;
    let proc = PathBuf::from(format!("/proc/{pid}"));
    if std::fs::metadata(&proc).map(|m| m.uid()).ok() != Some(uid) {
        return false;
    }
    if !std::fs::read_to_string(proc.join("comm")).is_ok_and(|c| c.trim() == HOLDER) {
        return false;
    }
    let Ok(ns) = std::fs::File::open(proc.join("ns/user")) else {
        return false;
    };
    // SAFETY: a valid descriptor, and this thread is the only one.
    unsafe { libc::setns(ns.as_raw_fd(), libc::CLONE_NEWUSER) == 0 }
}

// Fifteen characters at most: the kernel keeps no more of a process's
// name, and a longer one here would never compare equal to it.
const HOLDER: &str = "rustainer-ns";

/// Start the process that holds the namespace, give it its maps, and
/// return its pid. It is nobody's child: an intermediate process exits
/// at once, so nothing in this program ever has to reap it.
fn hold(
    uid: u32,
    gid: u32,
    uids: (u32, u32),
    gids: (u32, u32),
    newuidmap: &Path,
    newgidmap: &Path,
) -> Option<i32> {
    let (ready_r, ready_w) = rustix::pipe::pipe().ok()?;
    let (go_r, go_w) = rustix::pipe::pipe().ok()?;
    // SAFETY: one thread, and each branch below only makes system calls
    // until it either returns here in the parent or never returns.
    match unsafe { libc::fork() } {
        -1 => return None,
        0 => unsafe {
            if libc::fork() != 0 {
                libc::_exit(0);
            }
            drop(ready_r);
            drop(go_w);
            if libc::unshare(libc::CLONE_NEWUSER) != 0 {
                libc::_exit(1);
            }
            let me = libc::getpid().to_ne_bytes();
            libc::write(ready_w.as_raw_fd(), me.as_ptr().cast(), me.len());
            let mut byte = 0u8;
            if libc::read(go_r.as_raw_fd(), (&raw mut byte).cast(), 1) != 1 {
                libc::_exit(1);
            }
            // Out of the caller's session and off its descriptors: a
            // holder that kept the caller's stdout open would keep
            // every `$(rustainer ...)` waiting for an end of file.
            libc::setsid();
            let null = libc::open(c"/dev/null".as_ptr(), libc::O_RDWR);
            for fd in 0..3 {
                libc::dup2(null, fd);
            }
            // The system call itself: musl's bindings have no wrapper
            // for it, and the static binary is built against musl.
            libc::syscall(libc::SYS_close_range, 3u32, u32::MAX, 0u32);
            libc::prctl(libc::PR_SET_NAME, c"rustainer-ns".as_ptr());
            loop {
                libc::pause();
            }
        },
        middle => unsafe {
            libc::waitpid(middle, std::ptr::null_mut(), 0);
        },
    }
    drop(ready_w);
    drop(go_r);
    let mut pid = [0u8; 4];
    std::fs::File::from(ready_r).read_exact(&mut pid).ok()?;
    let pid = i32::from_ne_bytes(pid);
    let map = |tool: &Path, own: u32, sub: (u32, u32)| {
        std::process::Command::new(tool)
            .arg(pid.to_string())
            .args(["0", &own.to_string(), "1"])
            .args(["1", &sub.0.to_string(), &sub.1.to_string()])
            .stderr(std::process::Stdio::null())
            .status()
            .is_ok_and(|s| s.success())
    };
    if !(map(newuidmap, uid, uids) && map(newgidmap, gid, gids)) {
        // SAFETY: the pid is the process this function just started.
        unsafe { libc::kill(pid, libc::SIGKILL) };
        return None;
    }
    let mut go = std::fs::File::from(go_w);
    go.write_all(b"g").ok()?;
    drop(go);
    // Its name is set after it reads that byte; `join` checks the name.
    for _ in 0..200 {
        if std::fs::read_to_string(format!("/proc/{pid}/comm")).is_ok_and(|c| c.trim() == HOLDER) {
            return Some(pid);
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    None
}

#[cfg(test)]
mod tests {
    use super::parse_range;

    #[test]
    fn a_range_is_found_by_name_or_number_and_capped() {
        let text = "root:1:1\nalice:100000:65536\n1001:300000:1000000\nbroken\nzero:5:0\n";
        assert_eq!(
            parse_range(text, Some("alice"), 1000),
            Some((100000, 65536))
        );
        assert_eq!(parse_range(text, Some("bob"), 1001), Some((300000, 65536)));
        assert_eq!(parse_range(text, Some("carol"), 1002), None);
        assert_eq!(parse_range(text, Some("zero"), 7), None);
    }
}
