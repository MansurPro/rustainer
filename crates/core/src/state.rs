//! Runtime state: the directory layout, the lock, and the honest status.
//!
//! The **schema** here is deliberately provisional. The **shape** and the
//! **locking discipline** are not: every subcommand assumes them, and
//! changing them later means touching all of them (RND.md §18.11).
//!
//! One directory per container id, holding a state file, a lock file, the
//! start rendezvous FIFO, and the captured logs. All coordination between
//! the CLI, the supervisor and the container happens through this directory
//! — not through a protocol, and not through a daemon.

use std::fs::{File, OpenOptions};
use std::io::Write;
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

/// Where everything lives.
///
/// The root is `0700` and asserted on every startup. The FIFO inside is
/// necessarily writable by whoever can reach it, so the directory mode is
/// what actually contains it: otherwise any local user who learns a
/// container id could open `exec.fifo` and start someone else's container
/// (RND.md §5.5).
#[derive(Clone, Debug)]
pub struct Paths {
    root: PathBuf,
}

impl Paths {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    /// `$RUSTAINER_ROOT`, else `/run/rustainer` for root, else
    /// `$XDG_RUNTIME_DIR/rustainer`, else a per-uid directory under `/tmp`.
    pub fn default_root() -> PathBuf {
        if let Some(v) = std::env::var_os("RUSTAINER_ROOT") {
            return PathBuf::from(v);
        }
        if rustix::process::geteuid().is_root() {
            return PathBuf::from("/run/rustainer");
        }
        if let Some(x) = std::env::var_os("XDG_RUNTIME_DIR") {
            return PathBuf::from(x).join("rustainer");
        }
        PathBuf::from(format!(
            "/tmp/rustainer-{}",
            rustix::process::geteuid().as_raw()
        ))
    }

    pub fn containers(&self) -> PathBuf {
        self.root.join("containers")
    }
    pub fn dir(&self, id: &str) -> PathBuf {
        self.containers().join(id)
    }
    pub fn state_file(&self, id: &str) -> PathBuf {
        self.dir(id).join("state.json")
    }
    pub fn lock_file(&self, id: &str) -> PathBuf {
        self.dir(id).join("lock")
    }
    pub fn fifo(&self, id: &str) -> PathBuf {
        self.dir(id).join("exec.fifo")
    }
    pub fn stdout_log(&self, id: &str) -> PathBuf {
        self.dir(id).join("stdout.log")
    }
    pub fn stderr_log(&self, id: &str) -> PathBuf {
        self.dir(id).join("stderr.log")
    }

    /// Create the root `0700` and assert the mode if it already exists.
    pub fn ensure_root(&self) -> std::io::Result<()> {
        use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
        if !self.containers().exists() {
            std::fs::DirBuilder::new()
                .recursive(true)
                .mode(0o700)
                .create(self.containers())?;
        }
        let mode = std::fs::metadata(&self.root)?.permissions().mode() & 0o777;
        if mode & 0o077 != 0 {
            return Err(std::io::Error::other(format!(
                "{} is mode {mode:o}; it must not be group- or world-accessible, because the \
                 start FIFO inside it is what lets anyone who can reach it start a container",
                self.root.display()
            )));
        }
        Ok(())
    }

    /// Container ids, unsorted.
    pub fn list(&self) -> std::io::Result<Vec<String>> {
        let mut out = Vec::new();
        let dir = match std::fs::read_dir(self.containers()) {
            Ok(d) => d,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(out),
            Err(e) => return Err(e),
        };
        for entry in dir {
            let entry = entry?;
            if entry.file_type()?.is_dir()
                && let Some(name) = entry.file_name().to_str()
            {
                out.push(name.to_owned());
            }
        }
        Ok(out)
    }
}

/// Our status set is a superset of the OCI four. `Unknown` is the honest
/// answer when a container's supervisor died and nobody holds the exit
/// status — reporting `0` there is a lie, and it is the lie this stage
/// exists to not tell.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Status {
    Creating,
    Created,
    Running,
    Exited,
    Unknown,
}

impl Status {
    /// The projection onto the spec's four values, for `state` output.
    pub fn as_oci(self) -> &'static str {
        match self {
            Status::Creating => "creating",
            Status::Created => "created",
            Status::Running => "running",
            // The spec has no "unknown"; "stopped" is the nearest honest
            // projection, and our own field keeps the distinction.
            Status::Exited | Status::Unknown => "stopped",
        }
    }
}

impl std::fmt::Display for Status {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let s = match self {
            Status::Creating => "creating",
            Status::Created => "created",
            Status::Running => "running",
            Status::Exited => "exited",
            Status::Unknown => "unknown",
        };
        f.write_str(s)
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct State {
    pub id: String,
    pub status: Status,
    pub init_pid: Option<i32>,
    /// Field 22 of `/proc/<pid>/stat`, recorded at create.
    ///
    /// The PID-reuse defence, and needed **only** for the case where the
    /// supervisor is already dead and no pidfd exists. With a live pidfd it
    /// is redundant (RND.md §5.7).
    pub init_start_time: Option<u64>,
    pub supervisor_pid: Option<i32>,
    pub supervisor_start_time: Option<u64>,
    pub created_at: u64,
    pub started_at: Option<u64>,
    pub exited_at: Option<u64>,
    /// `None` means genuinely unknown.
    ///
    /// Deliberately not flattened into `128+N`, and deliberately not
    /// defaulted to `Some(0)`: a default of zero for a container whose exit
    /// nobody observed is a lie that only works because another field
    /// overrides it (RND.md §5.7).
    pub exit_code: Option<i32>,
    pub signal: Option<i32>,
    pub oom_killed: bool,
    pub command: Vec<String>,
    pub rootfs: Option<String>,
    pub hostname: Option<String>,
    /// Why the container never ran, when that is the reason it is gone.
    ///
    /// Kept in the state rather than only printed, so `ps` and a later
    /// `inspect` can explain a container that exited 127 three days ago.
    #[serde(default)]
    pub error: Option<String>,
}

impl State {
    pub fn new(id: String, command: Vec<String>) -> Self {
        Self {
            id,
            status: Status::Creating,
            init_pid: None,
            init_start_time: None,
            supervisor_pid: None,
            supervisor_start_time: None,
            created_at: now(),
            started_at: None,
            exited_at: None,
            exit_code: None,
            signal: None,
            oom_killed: false,
            command,
            rootfs: None,
            hostname: None,
            error: None,
        }
    }

    /// What is actually true, as opposed to what was last written.
    ///
    /// A recorded `running` only means the supervisor was alive when it
    /// wrote that. If both the init process and the supervisor are gone and
    /// no exit was ever recorded, nobody collected the status and the only
    /// honest answer is `Unknown`.
    pub fn observed_status(&self) -> Status {
        match self.status {
            Status::Exited | Status::Unknown => self.status,
            _ => {
                let init_alive = self
                    .init_pid
                    .is_some_and(|p| alive(p, self.init_start_time));
                let sup_alive = self
                    .supervisor_pid
                    .is_some_and(|p| alive(p, self.supervisor_start_time));
                if init_alive || sup_alive {
                    self.status
                } else {
                    Status::Unknown
                }
            }
        }
    }

    pub fn load(paths: &Paths, id: &str) -> std::io::Result<Self> {
        let bytes = std::fs::read(paths.state_file(id))?;
        serde_json::from_slice(&bytes).map_err(std::io::Error::other)
    }

    /// Write via a temporary file and `rename`, so a reader never sees a
    /// half-written document and a crash mid-write leaves the previous one.
    pub fn store(&self, paths: &Paths) -> std::io::Result<()> {
        let final_path = paths.state_file(&self.id);
        let tmp = final_path.with_extension("json.tmp");
        let mut f = OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o600)
            .open(&tmp)?;
        f.write_all(&serde_json::to_vec_pretty(self).map_err(std::io::Error::other)?)?;
        f.sync_all()?;
        drop(f);
        std::fs::rename(&tmp, &final_path)
    }
}

pub fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Field 22 of `/proc/<pid>/stat`: the process start time in clock ticks.
///
/// Split after the **last** `)`, never the first: `comm` is
/// attacker-influenced and a process named `evil) 0 R 1` shifts every later
/// field for anyone who splits on the first one.
pub fn start_time(pid: i32) -> Option<u64> {
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    let tail = stat.rsplit_once(')')?.1;
    tail.split_ascii_whitespace().nth(19)?.parse().ok()
}

/// Is this pid the same process we recorded?
///
/// Without the start-time comparison this is a PID-reuse bug: the id is
/// recycled, some unrelated process answers "alive", and a container that
/// died hours ago is reported as running.
pub fn alive(pid: i32, expected_start: Option<u64>) -> bool {
    match (start_time(pid), expected_start) {
        (Some(actual), Some(expected)) => actual == expected,
        (Some(_), None) => true,
        _ => false,
    }
}

/// The per-container exclusive lock.
///
/// `flock` is associated with an **open file description**, so a duplicate
/// created by `fork` or `dup` refers to the *same* lock and the lock is
/// released only when **all** such descriptors are closed. The naive
/// sequence — CLI takes the lock, CLI forks the supervisor, CLI exits —
/// therefore releases nothing: the supervisor inherited a duplicate and
/// holds the lock for the container's whole life, and every later `stop`,
/// `rm` or `logs` on that id blocks forever. Opening `O_CLOEXEC` is what
/// makes `execve` of the supervisor drop it.
///
/// The failure is total, silent until the *second* command, and invisible
/// to any test that runs one command. It is asserted, not assumed — see
/// `scripts/stage3-success-test.sh`.
///
/// Two more `flock` properties worth knowing: it is advisory only, and
/// descriptors obtained from separate `open` calls are treated
/// independently — so a process that opens the lock file twice can deadlock
/// against itself. This type being the only opener is what contains that.
#[derive(Debug)]
pub struct Lock {
    // Named, not a temporary: `File::open(p)?.lock()?` as a statement drops
    // the file at the semicolon and releases the lock immediately.
    _file: File,
}

impl Lock {
    pub fn acquire(path: &Path) -> std::io::Result<Self> {
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .custom_flags(libc::O_CLOEXEC)
            .mode(0o600)
            .open(path)?;
        file.lock()?;
        Ok(Self { _file: file })
    }

    /// Non-blocking, for commands that would rather report "busy" than wait.
    pub fn try_acquire(path: &Path) -> std::io::Result<Option<Self>> {
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .custom_flags(libc::O_CLOEXEC)
            .mode(0o600)
            .open(path)?;
        match file.try_lock() {
            Ok(()) => Ok(Some(Self { _file: file })),
            Err(std::fs::TryLockError::WouldBlock) => Ok(None),
            Err(std::fs::TryLockError::Error(e)) => Err(e),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp_paths(tag: &str) -> Paths {
        let p = Paths::new(std::env::temp_dir().join(format!("rustainer-state-{tag}")));
        let _ = std::fs::remove_dir_all(p.containers().parent().unwrap());
        p.ensure_root().unwrap();
        p
    }

    #[test]
    fn a_state_round_trips_through_the_file() {
        let p = tmp_paths("roundtrip");
        std::fs::create_dir_all(p.dir("abc")).unwrap();
        let mut s = State::new("abc".into(), vec!["/bin/sh".into()]);
        s.status = Status::Running;
        s.init_pid = Some(4242);
        s.store(&p).unwrap();
        let back = State::load(&p, "abc").unwrap();
        assert_eq!(back.id, "abc");
        assert_eq!(back.status, Status::Running);
        assert_eq!(back.init_pid, Some(4242));
        assert_eq!(back.exit_code, None, "never defaulted to 0");
    }

    /// The honesty rule, which is the whole point of the stage.
    #[test]
    fn a_dead_supervisor_makes_the_status_unknown_not_zero() {
        let mut s = State::new("x".into(), vec![]);
        s.status = Status::Running;
        // A pid that cannot be ours, with a start time that will not match.
        s.init_pid = Some(0x7FFF_FFFE);
        s.init_start_time = Some(1);
        s.supervisor_pid = Some(0x7FFF_FFFD);
        s.supervisor_start_time = Some(1);
        assert_eq!(s.observed_status(), Status::Unknown);
        assert_eq!(s.exit_code, None, "unknown must not invent an exit code");
    }

    #[test]
    fn a_live_process_keeps_the_recorded_status() {
        let me = std::process::id() as i32;
        let mut s = State::new("x".into(), vec![]);
        s.status = Status::Running;
        s.init_pid = Some(me);
        s.init_start_time = start_time(me);
        assert_eq!(s.observed_status(), Status::Running);
    }

    /// PID reuse: same pid, different process.
    #[test]
    fn a_recycled_pid_is_not_mistaken_for_the_original() {
        let me = std::process::id() as i32;
        assert!(alive(me, start_time(me)));
        assert!(
            !alive(me, start_time(me).map(|t| t + 1)),
            "a different start time must not count as the same process"
        );
    }

    #[test]
    fn an_exited_state_is_never_second_guessed() {
        let mut s = State::new("x".into(), vec![]);
        s.status = Status::Exited;
        s.exit_code = Some(42);
        s.init_pid = Some(0x7FFF_FFFE);
        assert_eq!(s.observed_status(), Status::Exited);
    }

    #[test]
    fn the_lock_is_exclusive_and_releases_on_drop() {
        let p = tmp_paths("lock");
        std::fs::create_dir_all(p.dir("c")).unwrap();
        let path = p.lock_file("c");
        let held = Lock::acquire(&path).unwrap();
        assert!(
            Lock::try_acquire(&path).unwrap().is_none(),
            "a second acquire must not succeed while the first is held"
        );
        drop(held);
        assert!(Lock::try_acquire(&path).unwrap().is_some());
    }

    #[test]
    fn a_group_readable_root_is_refused() {
        use std::os::unix::fs::PermissionsExt;
        let root = std::env::temp_dir().join("rustainer-state-mode");
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("containers")).unwrap();
        std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o750)).unwrap();
        let p = Paths::new(&root);
        assert!(p.ensure_root().is_err(), "0750 exposes the start FIFO");
        std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o700)).unwrap();
        assert!(p.ensure_root().is_ok());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn start_time_survives_a_hostile_comm() {
        // Same hazard as thread_count: split on the LAST ')'.
        let me = std::process::id() as i32;
        assert!(start_time(me).is_some());
    }

    #[test]
    fn listing_an_absent_root_is_empty_not_an_error() {
        let p = Paths::new(std::env::temp_dir().join("rustainer-state-missing"));
        let _ = std::fs::remove_dir_all(std::env::temp_dir().join("rustainer-state-missing"));
        assert_eq!(p.list().unwrap().len(), 0);
    }
}
