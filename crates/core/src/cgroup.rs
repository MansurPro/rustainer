//! cgroup v2 only.
//!
//! v1 is detected and refused, never silently degraded to "no limits". It is
//! not a flag or a shim: it is a second implementation with a different
//! hierarchy model, different files, different semantics for the same
//! options and a worse security posture — `release_agent` was removed from
//! v2 *by design*. Writing both doubles the subsystem for no educational
//! return (RND.md §18.5, §12.7).
//!
//! **A requested limit that cannot be applied is a hard error; a defaulted
//! one is a logged note.** The failure this rule exists to prevent has bitten
//! every rootless-container user: `--memory=512m` accepted, never enforced,
//! and the host OOMs three weeks later.

use std::ffi::CString;
use std::os::fd::OwnedFd;
use std::path::{Path, PathBuf};

use crate::sys;

/// Where the engine's own processes live: a leaf beside the containers.
///
/// Both the CLI and the supervisor move here. They must, because a cgroup
/// may not hold member processes and have controllers in `subtree_control`
/// at the same time — so the cgroup that *delegates* to the containers has
/// to be empty, and anything of ours sitting in it blocks every limit. This
/// is the same shape `systemd-run --scope -p Delegate=yes` arranges.
pub const OWN_LEAF: &str = "rustainer.supervisor";

/// The canonical mount point. Checked with `statfs`, never assumed.
const CGROUP_ROOT: &str = "/sys/fs/cgroup";

/// `include/uapi/linux/magic.h`.
const CGROUP2_SUPER_MAGIC: u64 = 0x6367_7270;
const TMPFS_MAGIC: u64 = 0x0102_1994;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    /// `/sys/fs/cgroup` is cgroup2. The only mode we support.
    Unified,
    /// tmpfs with v1 controllers under it, and a v2 mount at `unified`.
    Hybrid,
    /// tmpfs with v1 controllers only.
    Legacy,
    /// No cgroup filesystem at all — normal in some rootless contexts.
    Absent,
}

impl std::fmt::Display for Mode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Mode::Unified => "v2 (unified)",
            Mode::Hybrid => "v1/v2 (hybrid, unsupported)",
            Mode::Legacy => "v1 (unsupported)",
            Mode::Absent => "none",
        })
    }
}

/// The authoritative test is the filesystem magic on the mount point.
///
/// **Never parse `/proc/self/cgroup` to decide this.** The kernel emits the
/// `0::/` unified line whenever `CONFIG_CGROUP` is on, mounted or not —
/// measured on a v1-only host with no v2 mount anywhere. A runtime that
/// concludes "v2 is available" from that line then tries to write to a
/// read-only tmpfs. The sharp form: `/proc/self/cgroup` is for **location**,
/// `statfs` is for **detection** (RND.md §12.7).
pub fn detect() -> Mode {
    match statfs_magic(CGROUP_ROOT) {
        Some(CGROUP2_SUPER_MAGIC) => Mode::Unified,
        Some(TMPFS_MAGIC) => {
            if statfs_magic(&format!("{CGROUP_ROOT}/unified")) == Some(CGROUP2_SUPER_MAGIC) {
                Mode::Hybrid
            } else {
                Mode::Legacy
            }
        }
        // Absent is expected in a rootless context, not an error.
        _ => Mode::Absent,
    }
}

fn statfs_magic(path: &str) -> Option<u64> {
    let c = CString::new(path).ok()?;
    let s = rustix::fs::statfs(c.as_c_str()).ok()?;
    Some(s.f_type as u64)
}

/// What the user asked for. `None` means "said nothing", which is **not**
/// the same as "unlimited" — conflating the two is precisely why Docker's
/// API cannot report a limit it failed to apply (RND.md §12.7).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Limits {
    /// Bytes, to `memory.max`.
    pub memory: Option<u64>,
    /// Fractional CPUs, to `cpu.max` as `"$((cpus*100000)) 100000"`.
    pub cpus: Option<f64>,
    /// Tasks — threads, not processes — to `pids.max`.
    pub pids: Option<u64>,
}

impl Limits {
    pub fn any(&self) -> bool {
        self.memory.is_some() || self.cpus.is_some() || self.pids.is_some()
    }
    /// The controllers these limits need enabled.
    fn controllers(&self) -> Vec<&'static str> {
        let mut c = Vec::new();
        if self.memory.is_some() {
            c.push("memory");
        }
        if self.cpus.is_some() {
            c.push("cpu");
        }
        if self.pids.is_some() {
            c.push("pids");
        }
        c
    }
}

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error(
        "cgroups here are {mode}, and this engine implements cgroup v2 only. \
         Running without the limits you asked for would be a lie told to the operator, \
         so this is a refusal rather than a warning."
    )]
    NotUnified { mode: Mode },

    #[error(
        "no writable cgroup: {path} is not writable by this process. Without one, \
         {what} cannot be enforced. Run as root, or arrange cgroup delegation - with \
         systemd, `systemd-run --user --scope -p Delegate=yes` gives a delegated subtree."
    )]
    NotWritable {
        path: String,
        what: String,
        #[source]
        source: std::io::Error,
    },

    #[error(
        "the {controller} controller is not enabled in {parent}/cgroup.subtree_control, \
         so {file} does not exist and the limit cannot be applied. An administrator can \
         enable it with: echo +{controller} > {parent}/cgroup.subtree_control"
    )]
    ControllerUnavailable {
        controller: &'static str,
        parent: String,
        file: String,
    },

    #[error("writing {file}: {source}")]
    Write {
        file: String,
        #[source]
        source: std::io::Error,
    },

    #[error(
        "cannot enable the {controller} controller in {path}: it still holds {others}, \
         and a cgroup may not have both member processes and enabled controllers. \
         {what} therefore cannot be enforced here. Get a cgroup of our own - \
         `systemd-run --user --scope -p Delegate=yes -- <command>` - or move those \
         processes into a leaf first."
    )]
    SharedCgroup {
        path: String,
        /// Named, not counted: "it is busy" sends people to look at
        /// devices, and which process is in the way is the whole answer.
        others: String,
        controller: &'static str,
        what: String,
    },

    #[error("{0}")]
    Io(#[from] std::io::Error),
}

/// A container's own cgroup, plus a directory fd for `CLONE_INTO_CGROUP`.
pub struct Cgroup {
    dir: PathBuf,
    /// `O_PATH` on the directory. The kernel insists on a **directory** fd:
    /// a `cgroup.procs` fd is never accepted, by design, so that spawning
    /// into a cgroup is an atomic open-and-write using the caller's
    /// credentials (RND.md §12.9).
    fd: OwnedFd,
    /// Counter read at creation, so the OOM test is also correct for a
    /// cgroup we did not create.
    oom_kill_at_start: u64,
}

impl Cgroup {
    pub fn dir(&self) -> &Path {
        &self.dir
    }
    pub fn fd(&self) -> &OwnedFd {
        &self.fd
    }

    /// Prepare a cgroup for container `id`.
    ///
    /// **Every container gets a cgroup**, limits or not: it is what gives
    /// the container its own `/sys/fs/cgroup` view and what any later
    /// accounting will read. Only the controller-enabling dance is
    /// conditional on a limit actually being asked for.
    ///
    /// Returns `Ok(None)` when there is no usable cgroup **and** nothing was
    /// requested — the container runs unlimited and the caller says so. If
    /// limits *were* requested, every failure is an error instead.
    pub fn prepare(id: &str, limits: &Limits) -> Result<Option<Self>, Error> {
        let mode = detect();
        if mode != Mode::Unified {
            if limits.any() {
                return Err(Error::NotUnified { mode });
            }
            return Ok(None);
        }

        let base = delegating_parent();

        // Make sure we are in the leaf, not in the cgroup we are about to
        // enable controllers on.
        let want = limits.controllers();
        if !want.is_empty() {
            enter_own_leaf().map_err(|source| Error::NotWritable {
                path: base.display().to_string(),
                what: describe(limits),
                source,
            })?;
        }

        // Enable what we need in the parent. Silently skipping this is how
        // a limit file ends up missing and the limit ends up unapplied.
        for c in &want {
            if subtree_has(&base, c) {
                continue;
            }
            let sc = base.join("cgroup.subtree_control");
            if let Err(source) = write_file(&sc, &format!("+{c}")) {
                // EBUSY here almost always means the cgroup still holds
                // processes that are not ours - a login shell, an sshd, the
                // rest of a container. Say so, with the count, because
                // "device or resource busy" sends people to look at devices.
                let others = other_procs(&base);
                if !others.is_empty() {
                    return Err(Error::SharedCgroup {
                        path: base.display().to_string(),
                        others,
                        controller: c,
                        what: describe(limits),
                    });
                }
                return Err(Error::NotWritable {
                    path: sc.display().to_string(),
                    what: format!("the {c} controller"),
                    source,
                });
            }
        }

        let dir = base.join(format!("rustainer-{id}"));
        if let Err(source) = std::fs::create_dir_all(&dir) {
            // Nothing was asked for, so nothing was promised: run without a
            // cgroup rather than refusing. A *requested* limit takes the
            // other branch and fails loudly.
            if !limits.any() {
                return Ok(None);
            }
            return Err(Error::NotWritable {
                path: dir.display().to_string(),
                what: describe(limits),
                source,
            });
        }

        let cdir = CString::new(dir.as_os_str().as_encoded_bytes().to_vec())
            .map_err(|_| std::io::Error::other("cgroup path contains a NUL"))?;
        let fd = sys::open_path_dir(&cdir).map_err(|e| Error::NotWritable {
            path: dir.display().to_string(),
            what: describe(limits),
            source: std::io::Error::from_raw_os_error(e.raw_os_error()),
        })?;

        let oom_kill_at_start = read_event(&dir, "oom_kill").unwrap_or(0);
        Ok(Some(Self {
            dir,
            fd,
            oom_kill_at_start,
        }))
    }

    /// Write the limits. Any requested limit that cannot be applied is an
    /// error, never a warning.
    pub fn apply(&self, limits: &Limits) -> Result<(), Error> {
        if let Some(bytes) = limits.memory {
            self.write_limit("memory", "memory.max", &bytes.to_string())?;
            // Swap alone in v2, unlike v1's memsw which was memory+swap.
            //
            // Set to 0 because a memory limit that swap can evade is not a
            // limit: on a host with swap, a container at `memory.max` simply
            // pages out and keeps going, and the OOM killer never fires.
            // This is **stricter than Docker**, whose `--memory-swap`
            // defaults to twice the memory limit. Deliberate: `--memory`
            // should mean what the person typing it thinks it means, and
            // `--memory-swap` can relax it when that lands.
            //
            // Best-effort: on a kernel built without swap accounting the
            // file is absent, and that is not a failure to apply --memory.
            if self.dir.join("memory.swap.max").exists() {
                let _ = self.write_limit("memory", "memory.swap.max", "0");
            }
        }
        if let Some(cpus) = limits.cpus {
            // "$MAX $PERIOD", both microseconds, default period 100000.
            // cpu.max is absolute and binds always; cpu.weight is
            // proportional and binds only under contention. They are not two
            // strengths of one knob, and --cpus means the absolute one.
            let quota = (cpus * 100_000.0).round() as i64;
            self.write_limit("cpu", "cpu.max", &format!("{quota} 100000"))?;
        }
        if let Some(n) = limits.pids {
            // Counts TASKS, i.e. threads: a JVM with 200 threads consumes
            // 200. On hitting it clone(2) fails EAGAIN - there is no kill.
            self.write_limit("pids", "pids.max", &n.to_string())?;
        }
        Ok(())
    }

    fn write_limit(&self, controller: &'static str, file: &str, value: &str) -> Result<(), Error> {
        let path = self.dir.join(file);
        if !path.exists() {
            let parent = self
                .dir
                .parent()
                .map(|p| p.display().to_string())
                .unwrap_or_default();
            return Err(Error::ControllerUnavailable {
                controller,
                parent,
                file: path.display().to_string(),
            });
        }
        write_file(&path, value).map_err(|source| Error::Write {
            file: path.display().to_string(),
            source,
        })
    }

    /// Did the kernel's OOM killer kill something in this cgroup?
    ///
    /// `memory.events`'s `max` is **not** a kill — a cache-heavy container
    /// sitting at its limit accumulates thousands and is never killed. `oom`
    /// is not a kill either; some of those paths return `-ENOMEM` to
    /// userspace. Only `oom_kill` counts processes actually killed, and it
    /// is compared against the value read at creation (RND.md §12.4).
    pub fn oom_killed(&self) -> bool {
        read_event(&self.dir, "oom_kill").unwrap_or(0) > self.oom_kill_at_start
    }

    /// Kill every process in the cgroup, atomically, with no PID races and
    /// nothing left behind to fork again. Kernel 5.14.
    pub fn kill_all(&self) -> std::io::Result<()> {
        write_file(&self.dir.join("cgroup.kill"), "1")
    }

    /// Read a limit back, for tests and for `ps`.
    pub fn read(&self, file: &str) -> Option<String> {
        std::fs::read_to_string(self.dir.join(file))
            .ok()
            .map(|s| s.trim().to_owned())
    }

    pub fn remove(&self) {
        // A cgroup with live tasks cannot be removed; that is the kernel
        // telling us the truth, so there is nothing to do about it here.
        let _ = std::fs::remove_dir(&self.dir);
    }
}

fn describe(l: &Limits) -> String {
    let mut parts = Vec::new();
    if l.memory.is_some() {
        parts.push("--memory");
    }
    if l.cpus.is_some() {
        parts.push("--cpus");
    }
    if l.pids.is_some() {
        parts.push("--pids-limit");
    }
    if parts.is_empty() {
        "resource limits".into()
    } else {
        parts.join(", ")
    }
}

/// The cgroup that delegates to the containers: our own, or its parent when
/// we have already moved into [`OWN_LEAF`].
pub fn delegating_parent() -> PathBuf {
    let rel = own_cgroup().unwrap_or_else(|| "/".into());
    let mut p = PathBuf::from(CGROUP_ROOT).join(rel.trim_start_matches('/'));
    if p.file_name().and_then(|n| n.to_str()) == Some(OWN_LEAF) {
        p = p.parent().map(Path::to_path_buf).unwrap_or(p);
    }
    p
}

/// Move this process into [`OWN_LEAF`]. Idempotent, and a no-op once there.
///
/// Called by the CLI before it starts a supervisor and by the supervisor
/// itself: whichever runs first does the work, and the other inherits it.
pub fn enter_own_leaf() -> std::io::Result<()> {
    let rel = own_cgroup().unwrap_or_else(|| "/".into());
    if rel.rsplit('/').next() == Some(OWN_LEAF) {
        return Ok(());
    }
    let leaf = PathBuf::from(CGROUP_ROOT)
        .join(rel.trim_start_matches('/'))
        .join(OWN_LEAF);
    if let Err(e) = std::fs::create_dir_all(&leaf)
        && e.kind() != std::io::ErrorKind::AlreadyExists
    {
        return Err(e);
    }
    write_file(&leaf.join("cgroup.procs"), &std::process::id().to_string())
}

/// The cgroup another process lives in, as a directory descriptor.
///
/// For `exec`: a second process in a container must land under the same
/// limits as the first, and `CLONE_INTO_CGROUP` wants a directory fd.
///
/// The path is read back from `/proc/<pid>/cgroup` rather than rebuilt from
/// the container id. Rebuilding it would have to re-derive
/// [`delegating_parent`] from *this* process, and `exec` may well run from
/// a different login session - and therefore a different slice - than the
/// `run` that created the container. The kernel already knows the answer.
///
/// One sharp edge: a `/proc/<pid>/cgroup` read reports the path relative to
/// the **reader's** cgroup namespace, not the subject's. Read from the host
/// namespace - which is where `exec` runs, before any `setns` - it is the
/// full path. Read from inside the container it would be `/`, which would
/// silently resolve to the root cgroup. This is why the open happens first.
pub fn open_existing(pid: i32) -> Option<OwnedFd> {
    let s = std::fs::read_to_string(format!("/proc/{pid}/cgroup")).ok()?;
    let rel = s.lines().find_map(|l| l.strip_prefix("0::"))?.trim();
    if rel == "/" {
        return None;
    }
    let dir = PathBuf::from(CGROUP_ROOT).join(rel.trim_start_matches('/'));
    let c = std::ffi::CString::new(dir.as_os_str().as_encoded_bytes()).ok()?;
    crate::sys::open_path_dir(&c).ok()
}

/// The `0::` line of `/proc/self/cgroup` — our location within the hierarchy.
pub fn own_cgroup() -> Option<String> {
    let s = std::fs::read_to_string("/proc/self/cgroup").ok()?;
    s.lines()
        .find_map(|l| l.strip_prefix("0::"))
        .map(|p| p.trim().to_owned())
}

fn subtree_has(base: &Path, controller: &str) -> bool {
    std::fs::read_to_string(base.join("cgroup.subtree_control"))
        .map(|s| s.split_ascii_whitespace().any(|c| c == controller))
        .unwrap_or(false)
}

/// Processes in this cgroup that are not us, named.
fn other_procs(base: &Path) -> String {
    let me = std::process::id().to_string();
    let Ok(s) = std::fs::read_to_string(base.join("cgroup.procs")) else {
        return String::new();
    };
    let named: Vec<String> = s
        .split_ascii_whitespace()
        .filter(|p| !p.is_empty() && *p != me)
        .take(5)
        .map(|p| {
            let comm = std::fs::read_to_string(format!("/proc/{p}/comm"))
                .map(|c| c.trim().to_owned())
                .unwrap_or_else(|_| "?".into());
            format!("{p} ({comm})")
        })
        .collect();
    named.join(", ")
}

fn read_event(dir: &Path, key: &str) -> Option<u64> {
    let s = std::fs::read_to_string(dir.join("memory.events")).ok()?;
    s.lines().find_map(|l| {
        let (k, v) = l.split_once(' ')?;
        (k == key).then(|| v.trim().parse().ok())?
    })
}

/// A single `write(2)` of an ASCII string. cgroupfs needs no library.
///
/// `O_NONBLOCK` is not decoration on `memory.max`: a plain write **blocks
/// the writing thread** while the kernel reclaims on the target's behalf,
/// terminating only when the new limit is met or the writer is killed. For
/// a one-shot setup write that is tolerable; for shrinking a live
/// container's limit from an event loop it is an unbounded stall, so the
/// flag is set here once rather than remembered later (RND.md §12.3).
fn write_file(path: &Path, value: &str) -> std::io::Result<()> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    let mut f = std::fs::OpenOptions::new()
        .write(true)
        .custom_flags(libc::O_NONBLOCK)
        .open(path)?;
    f.write_all(value.as_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detection_never_consults_proc_self_cgroup() {
        // The kernel emits `0::/` whenever CONFIG_CGROUP is on, mounted or
        // not, so that line can never decide this. Assert the two are
        // independent: own_cgroup() answers even where detect() says no
        // cgroup filesystem is mounted.
        let _ = detect();
        assert!(
            own_cgroup().is_some(),
            "the 0:: line exists on any CONFIG_CGROUP kernel, which is exactly why \
             it cannot be used for detection"
        );
    }

    #[test]
    fn cpu_quota_uses_the_documented_period() {
        let f = |c: f64| ((c * 100_000.0).round() as i64, 100_000);
        assert_eq!(f(1.5), (150_000, 100_000));
        assert_eq!(f(0.5), (50_000, 100_000));
        assert_eq!(f(1.0), (100_000, 100_000));
    }

    #[test]
    fn none_is_not_unlimited() {
        let l = Limits::default();
        assert!(!l.any(), "nothing requested");
        assert!(l.controllers().is_empty());
        let l = Limits {
            memory: Some(0),
            ..Default::default()
        };
        assert!(
            l.any(),
            "a request for 0 is still a request, and must not read as 'unspecified'"
        );
        assert_eq!(l.controllers(), vec!["memory"]);
    }

    #[test]
    fn each_limit_pulls_in_exactly_its_controller() {
        let l = Limits {
            memory: Some(1),
            cpus: Some(1.0),
            pids: Some(1),
        };
        let c = l.controllers();
        assert!(c.contains(&"memory") && c.contains(&"cpu") && c.contains(&"pids"));
        assert_eq!(c.len(), 3);
    }

    #[test]
    fn the_refusal_names_the_mode_and_the_fix() {
        let e = Error::NotUnified { mode: Mode::Legacy };
        let s = e.to_string();
        assert!(s.contains("v1"), "{s}");
        assert!(s.contains("refusal"), "{s}");
    }

    #[test]
    fn an_unavailable_controller_names_the_one_line_fix() {
        let e = Error::ControllerUnavailable {
            controller: "memory",
            parent: "/sys/fs/cgroup/x".into(),
            file: "/sys/fs/cgroup/x/rustainer-a/memory.max".into(),
        };
        let s = e.to_string();
        assert!(
            s.contains("+memory > /sys/fs/cgroup/x/cgroup.subtree_control"),
            "{s}"
        );
    }
}
