//! Capabilities, user, and seccomp — the step from process isolation to
//! something that can honestly be called a sandbox.
//!
//! Everything here is decided in the parent and executed in the child, for
//! the usual reason: the child cannot allocate. The capability sets are
//! three `u64`s, the seccomp program is a `Vec<sock_filter>` compiled before
//! the clone, and the user is three numbers. No library owns the ordering.

use serde::{Deserialize, Serialize};

use crate::sys::{self, Error as Errno};

pub use rustix::thread::CapabilitySet;

/// Docker's default capability set, which is also ours.
///
/// Fourteen of the forty-odd. Notably **not** `CAP_SYS_ADMIN`: its absence
/// is half of what makes masked paths a boundary rather than decoration
/// (RND.md §13.4).
pub fn default_capabilities() -> CapabilitySet {
    CapabilitySet::CHOWN
        | CapabilitySet::DAC_OVERRIDE
        | CapabilitySet::FSETID
        | CapabilitySet::FOWNER
        | CapabilitySet::MKNOD
        | CapabilitySet::NET_RAW
        | CapabilitySet::SETGID
        | CapabilitySet::SETUID
        | CapabilitySet::SETFCAP
        | CapabilitySet::SETPCAP
        | CapabilitySet::NET_BIND_SERVICE
        | CapabilitySet::SYS_CHROOT
        | CapabilitySet::KILL
        | CapabilitySet::AUDIT_WRITE
}

/// Resolve a capability name like `NET_ADMIN` or `CAP_NET_ADMIN`.
pub fn capability_by_name(name: &str) -> Option<CapabilitySet> {
    let n = name.trim().to_ascii_uppercase();
    let n = n.strip_prefix("CAP_").unwrap_or(&n);
    CapabilitySet::all().iter().find(|c| {
        // bitflags' Debug prints the flag's name.
        format!("{c:?}")
            .trim_start_matches("CapabilitySet(")
            .trim_end_matches(')')
            == n
    })
}

/// Who the container's process runs as.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct UserSpec {
    pub uid: u32,
    pub gid: u32,
    #[serde(default)]
    pub groups: Vec<u32>,
}

/// Is `id` reachable from this user namespace?
///
/// In a single-ID namespace - `unshare --user --map-root-user`, and the
/// whole of stage 11 - exactly one id is mapped, and `setresuid` to any
/// other returns `EINVAL`. Answering this in the parent turns an obscure
/// failure deep in the child into a sentence.
pub fn id_is_mapped(map_file: &str, id: u32) -> bool {
    let Ok(text) = std::fs::read_to_string(map_file) else {
        // No map file means no user namespace, so every id is reachable.
        return true;
    };
    if text.trim().is_empty() {
        return true;
    }
    text.lines().any(|l| {
        let f: Vec<u32> = l
            .split_whitespace()
            .filter_map(|x| x.parse().ok())
            .collect();
        matches!(f.as_slice(), [inside, _outside, count]
            if id >= *inside && (id - *inside) < *count)
    })
}

impl UserSpec {
    /// Why this user cannot be entered in the container, if it cannot.
    ///
    /// `new_userns` is stage 11: the container is about to be cloned into
    /// a user namespace of its own whose map we are about to write. Our
    /// own map is then the wrong file to consult - it is the initial
    /// namespace's, where every id is mapped - and reading it is how
    /// `--user 1000:1000` became `changing group id failed with EINVAL`
    /// deep in the child instead of a sentence here.
    pub fn unmapped_reason(&self, new_userns: bool) -> Option<String> {
        let (u, g) = if new_userns {
            // We write a single extent, `0 <our euid> 1`. No file to read:
            // we know the answer because we are the one who decides it.
            (self.uid == 0, self.gid == 0)
        } else {
            (
                id_is_mapped("/proc/self/uid_map", self.uid),
                id_is_mapped("/proc/self/gid_map", self.gid),
            )
        };
        if u && g {
            return None;
        }
        let which = match (u, g) {
            (false, false) => format!("uid {} and gid {}", self.uid, self.gid),
            (false, true) => format!("uid {}", self.uid),
            _ => format!("gid {}", self.gid),
        };
        Some(format!(
            "{which} is not mapped into the container's user namespace, so the container \
             cannot switch to it - setresuid would return EINVAL. A single-ID namespace \
             maps exactly one id, and running unprivileged is a single-ID namespace: the \
             only id inside it is 0, which is you. Mapping a range needs /etc/subuid and \
             the setuid helpers, which are deliberately not implemented. Run as root, or \
             use --user with the id that is mapped."
        ))
    }
}

impl UserSpec {
    /// `uid`, or `uid:gid`. Names are deliberately not resolved: that needs
    /// the container's own `/etc/passwd`, which does not exist until after
    /// the pivot, and resolving the host's would be a different user.
    pub fn parse(s: &str) -> Result<Self, String> {
        let (u, g) = match s.split_once(':') {
            Some((u, g)) => (u, Some(g)),
            None => (s, None),
        };
        let uid: u32 = u.parse().map_err(|_| {
            format!(
                "{u:?} is not a numeric uid. Names are not resolved yet: that \
                                  needs the container's own /etc/passwd, and the host's would \
                                  be a different user."
            )
        })?;
        let gid: u32 = match g {
            Some(g) => g
                .parse()
                .map_err(|_| format!("{g:?} is not a numeric gid"))?,
            None => uid,
        };
        Ok(Self {
            uid,
            gid,
            groups: Vec::new(),
        })
    }
}

// ---------------------------------------------------------------------------
// seccomp
// ---------------------------------------------------------------------------

/// A seccomp profile, in the shape of the OCI runtime spec's
/// `linux.seccomp`, so a profile written for `runc` or Docker loads here.
///
/// Argument filtering (`args`) is **not** implemented, and a profile that
/// uses it is rejected rather than silently applied without it — a rule
/// quietly losing its condition is how "deny `clone` with `CLONE_NEWUSER`"
/// becomes "deny `clone`" or "allow everything".
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Profile {
    #[serde(rename = "defaultAction")]
    pub default_action: Action,
    #[serde(default, rename = "defaultErrnoRet")]
    pub default_errno: Option<u32>,
    #[serde(default)]
    pub syscalls: Vec<SyscallRule>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SyscallRule {
    pub names: Vec<String>,
    pub action: Action,
    #[serde(default, rename = "errnoRet")]
    pub errno: Option<u32>,
    /// Present only so a profile that uses it can be refused rather than
    /// silently misapplied.
    #[serde(default)]
    pub args: Option<serde_json::Value>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Action {
    #[serde(rename = "SCMP_ACT_ALLOW")]
    Allow,
    #[serde(rename = "SCMP_ACT_ERRNO")]
    Errno,
    #[serde(rename = "SCMP_ACT_KILL")]
    Kill,
    #[serde(rename = "SCMP_ACT_KILL_PROCESS")]
    KillProcess,
    #[serde(rename = "SCMP_ACT_LOG")]
    Log,
    #[serde(rename = "SCMP_ACT_TRAP")]
    Trap,
}

/// The default profile: a **deny list**.
///
/// Weaker than the allow list Docker ships, and said plainly rather than
/// implied: an allow list fails closed for a syscall nobody thought about,
/// and this does not. It is here because the mechanism — the fail-closed
/// architecture check, the errno action, the install position — is what this
/// stage is for, and because `--security-opt seccomp=<file>` takes Docker's
/// own profile unchanged for anyone who wants the stronger shape.
///
/// What it denies is the set the research keeps returning to: namespace
/// creation (`clone3` **wholesale**, because it takes a *pointer* to its
/// flags and is therefore structurally unfilterable), the mount family that
/// would otherwise let a container remount `/proc` and walk around masked
/// paths, module loading, kexec, and the keyring.
pub fn default_profile() -> Profile {
    const DENIED: &[&str] = &[
        // Namespace creation. clone3 cannot be argument-filtered at all.
        "clone3",
        "setns",
        "unshare",
        // The mount family. Without these, masked and read-only paths are
        // not a boundary: procfs gets a fresh superblock per mount, so a
        // container that can mount gets an unmasked /proc (RND.md §13.4).
        "mount",
        "umount2",
        "move_mount",
        "fsopen",
        "fsconfig",
        "fsmount",
        "fspick",
        "open_tree",
        "mount_setattr",
        "pivot_root",
        "chroot",
        // Kernel code and memory.
        "init_module",
        "finit_module",
        "delete_module",
        "kexec_load",
        "kexec_file_load",
        "bpf",
        "perf_event_open",
        "process_vm_readv",
        "process_vm_writev",
        // Host-wide state.
        "reboot",
        "swapon",
        "swapoff",
        "settimeofday",
        "clock_settime",
        "adjtimex",
        "clock_adjtime",
        "acct",
        "quotactl",
        // Keyring and file handles.
        "add_key",
        "request_key",
        "keyctl",
        "open_by_handle_at",
        // Userfaultfd: the standard way to win a kernel race by stalling a
        // page fault for as long as you like.
        "userfaultfd",
    ];
    Profile {
        default_action: Action::Allow,
        default_errno: None,
        syscalls: vec![SyscallRule {
            names: DENIED.iter().map(|s| (*s).to_owned()).collect(),
            action: Action::Errno,
            errno: Some(libc::EPERM as u32),
            args: None,
        }],
    }
}

#[derive(Debug, thiserror::Error)]
pub enum SeccompError {
    #[error("seccomp profile: {0}")]
    Parse(String),
    #[error(
        "the profile uses per-argument rules (`args`), which are not implemented. \
         Applying it without them would silently change what it means - a rule that \
         loses its condition either denies far more than intended or nothing at all - \
         so this is a refusal."
    )]
    ArgsUnsupported,
    #[error("compiling the seccomp filter: {0}")]
    Compile(String),
    #[error(
        "this architecture has no seccomp support here. The filter carries a \
         fail-closed architecture check as its first instruction, and we will not \
         install one that cannot name the architecture it is checking for."
    )]
    UnsupportedArch,
}

/// Compile a profile into a cBPF program, parent-side.
///
/// `seccompiler` emits the architecture validation sequence as the **first**
/// instructions, which is the part that matters: a filter without a
/// fail-closed architecture check can be bypassed entirely by entering
/// through a different syscall ABI, where the numbers mean something else.
pub fn compile(profile: &Profile) -> Result<Vec<libc::sock_filter>, SeccompError> {
    use seccompiler::TargetArch;

    if profile.syscalls.iter().any(|r| r.args.is_some()) {
        return Err(SeccompError::ArgsUnsupported);
    }

    let arch = if cfg!(target_arch = "x86_64") {
        TargetArch::x86_64
    } else if cfg!(target_arch = "aarch64") {
        TargetArch::aarch64
    } else if cfg!(target_arch = "riscv64") {
        TargetArch::riscv64
    } else {
        return Err(SeccompError::UnsupportedArch);
    };

    // seccompiler takes one action for the whole matched set, so a profile
    // mixing actions across rules cannot be expressed. Reject it rather
    // than applying the first one to everything.
    let mut match_action: Option<serde_json::Value> = None;
    let mut names: Vec<&str> = Vec::new();
    for rule in &profile.syscalls {
        let a = action_json(rule.action, rule.errno);
        match &match_action {
            None => match_action = Some(a),
            Some(prev) if *prev == a => {}
            Some(_) => {
                return Err(SeccompError::Compile(
                    "this profile uses more than one action across its rules, which is not \
                     supported yet. Split it, or use one action."
                        .into(),
                ));
            }
        }
        names.extend(rule.names.iter().map(String::as_str));
    }

    // Translate into seccompiler's own JSON rather than reimplementing its
    // syscall table: the name-to-number mapping is per-architecture, long,
    // and exactly the kind of thing to get subtly wrong by hand. Its table
    // is private, its JSON frontend is not.
    let filter = serde_json::json!({
        "f": {
            "default_action": action_json(profile.default_action, profile.default_errno),
            "filter_action": match_action.unwrap_or(serde_json::json!("allow")),
            "filter": names
                .iter()
                .map(|n| serde_json::json!({ "syscall": n }))
                .collect::<Vec<_>>(),
        }
    });

    let text = filter.to_string();
    let mut map = match seccompiler::compile_from_json(text.as_bytes(), arch) {
        Ok(m) => m,
        Err(e) => {
            let msg = e.to_string();
            // An unknown name is only safe to skip when everything else is
            // allowed by default. In an allow-list profile, dropping a name
            // silently denies that syscall, which is a different policy.
            if msg.contains("Invalid syscall name") && profile.default_action == Action::Allow {
                return compile_skipping_unknown(profile, arch, &names);
            }
            return Err(SeccompError::Compile(msg));
        }
    };
    let prog = map
        .remove("f")
        .ok_or_else(|| SeccompError::Compile("seccompiler produced no filter".into()))?;
    Ok(to_libc(prog))
}

/// Retry without the names this kernel's architecture does not know.
///
/// Profiles are shared across kernels and architectures, and a syscall this
/// one has never heard of cannot be called anyway. Only reached when the
/// default action is Allow — see the caller.
fn compile_skipping_unknown(
    profile: &Profile,
    arch: seccompiler::TargetArch,
    names: &[&str],
) -> Result<Vec<libc::sock_filter>, SeccompError> {
    let mut kept: Vec<&str> = Vec::new();
    for n in names {
        let probe = serde_json::json!({
            "f": { "default_action": "allow", "filter_action": "allow",
                   "filter": [{ "syscall": n }] }
        })
        .to_string();
        if seccompiler::compile_from_json(probe.as_bytes(), arch).is_ok() {
            kept.push(n);
        }
    }
    let action = profile
        .syscalls
        .first()
        .map(|r| action_json(r.action, r.errno))
        .unwrap_or(serde_json::json!("allow"));
    let filter = serde_json::json!({
        "f": {
            "default_action": action_json(profile.default_action, profile.default_errno),
            "filter_action": action,
            "filter": kept.iter().map(|n| serde_json::json!({ "syscall": n })).collect::<Vec<_>>(),
        }
    })
    .to_string();
    let mut map = seccompiler::compile_from_json(filter.as_bytes(), arch)
        .map_err(|e| SeccompError::Compile(e.to_string()))?;
    let prog = map
        .remove("f")
        .ok_or_else(|| SeccompError::Compile("seccompiler produced no filter".into()))?;
    Ok(to_libc(prog))
}

fn to_libc(prog: seccompiler::BpfProgram) -> Vec<libc::sock_filter> {
    // seccompiler's sock_filter and libc's are the same four fields in the
    // same order; convert rather than transmute so a layout change is a
    // compile error.
    prog.into_iter()
        .map(|i| libc::sock_filter {
            code: i.code,
            jt: i.jt,
            jf: i.jf,
            k: i.k,
        })
        .collect()
}

fn action_json(a: Action, errno: Option<u32>) -> serde_json::Value {
    match a {
        Action::Allow => serde_json::json!("allow"),
        Action::Errno => serde_json::json!({ "errno": errno.unwrap_or(libc::EPERM as u32) }),
        Action::Kill | Action::KillProcess => serde_json::json!("kill_process"),
        Action::Log => serde_json::json!("log"),
        Action::Trap => serde_json::json!("trap"),
    }
}

// ---------------------------------------------------------------------------
// What the child applies
// ---------------------------------------------------------------------------

/// The security posture, fully resolved parent-side.
pub struct Security {
    /// Capabilities to drop from the bounding set, one per entry, each a
    /// single bit. Built here so the child iterates rather than computes.
    pub bounding_drop: Vec<CapabilitySet>,
    /// What stays in permitted and effective after the uid change.
    pub keep: CapabilitySet,
    pub user: Option<UserSpec>,
    pub groups: Vec<rustix::process::Gid>,
    /// A compiled cBPF program, or `None` for no filter.
    pub seccomp: Vec<libc::sock_filter>,
    pub no_new_privs: bool,
}

impl Security {
    pub fn build(
        keep: CapabilitySet,
        user: Option<UserSpec>,
        seccomp: Vec<libc::sock_filter>,
    ) -> Self {
        let bounding_drop = (CapabilitySet::all() & !keep).iter().collect();
        let groups = user
            .as_ref()
            .map(|u| {
                u.groups
                    .iter()
                    .map(|g| rustix::process::Gid::from_raw(*g))
                    .collect()
            })
            .unwrap_or_default();
        Self {
            bounding_drop,
            keep,
            user,
            groups,
            seccomp,
            no_new_privs: true,
        }
    }
}

/// Which of the thirteen steps failed, so the parent can say which.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum Step {
    DropBounding = 1,
    ClearInheritable = 2,
    KeepCaps = 3,
    SetGroups = 4,
    SetGid = 5,
    SetUid = 6,
    ClearKeepCaps = 7,
    SetCaps = 8,
    NoNewPrivs = 11,
    Seccomp = 12,
}

impl Step {
    pub fn from_u8(v: u8) -> Option<Self> {
        Some(match v {
            1 => Step::DropBounding,
            2 => Step::ClearInheritable,
            3 => Step::KeepCaps,
            4 => Step::SetGroups,
            5 => Step::SetGid,
            6 => Step::SetUid,
            7 => Step::ClearKeepCaps,
            8 => Step::SetCaps,
            11 => Step::NoNewPrivs,
            12 => Step::Seccomp,
            _ => return None,
        })
    }
    pub fn describe(self) -> &'static str {
        match self {
            Step::DropBounding => "dropping the capability bounding set",
            Step::ClearInheritable => "clearing the inheritable capability set",
            Step::KeepCaps => "setting PR_SET_KEEPCAPS",
            Step::SetGroups => "setting supplementary groups",
            Step::SetGid => "changing group id",
            Step::SetUid => "changing user id",
            Step::ClearKeepCaps => "clearing PR_SET_KEEPCAPS",
            Step::SetCaps => "restoring the permitted and effective capability sets",
            Step::NoNewPrivs => "setting no_new_privs",
            Step::Seccomp => "installing the seccomp filter",
        }
    }
}

/// Apply the security posture, in the only order that works.
///
/// # Safety
///
/// Child-side only, immediately before `execve`.
///
/// The ordering is the part that is wrong in the obvious implementation, so
/// it is written out here rather than delegated:
///
/// 1. **bounding drop before the uid change** — `PR_CAPBSET_DROP` needs
///    `CAP_SETPCAP` *effective*, and changing to a non-zero uid wipes the
///    effective set. Getting this backwards fails `EPERM` for a non-root
///    container user and **silently succeeds for `USER root`**, which is why
///    a test suite that only runs stock images proves nothing.
/// 2. **inheritable cleared first** — the root special case computes
///    `P'(permitted) = P(inheritable) | P(bounding)`, not `P(bounding)`. A
///    non-empty inheritable set silently reopens the policy.
/// 3. `KEEPCAPS` preserves **permitted** across the uid change, never
///    effective: `capabilities(7)` says the effective set is cleared even
///    with it set.
/// 8. `capset` is what undoes that wipe, restoring effective from the
///    permitted set `KEEPCAPS` kept. It needs no capability to do so.
///
/// And 11 must precede 12: `seccomp(2)` requires `CAP_SYS_ADMIN` **or**
/// `no_new_privs`, and we have just given away the former.
pub unsafe fn apply(sec: &Security) -> Result<(), (Step, Errno)> {
    use rustix::thread;

    // 1. Everything not wanted leaves the bounding set, irreversibly, while
    //    CAP_SETPCAP is still effective.
    for cap in &sec.bounding_drop {
        if let Err(e) = thread::remove_capability_from_bounding_set(*cap) {
            // EINVAL means this kernel does not know the capability, which
            // is fine: it cannot be used either.
            if e != Errno::INVAL {
                return Err((Step::DropBounding, e));
            }
        }
    }

    // 2. Close the inheritable hole before anything depends on it.
    let current = thread::capabilities(None).map_err(|e| (Step::ClearInheritable, e))?;
    thread::set_capabilities(
        None,
        thread::CapabilitySets {
            effective: current.effective,
            permitted: current.permitted,
            inheritable: CapabilitySet::empty(),
        },
    )
    .map_err(|e| (Step::ClearInheritable, e))?;

    if let Some(u) = &sec.user {
        // 3.
        thread::set_keep_capabilities(true).map_err(|e| (Step::KeepCaps, e))?;
        // 4. Before setgid, because it needs CAP_SETGID.
        //
        // Dropping supplementary groups matters: without it the container
        // inherits the engine's. But in a user namespace created by an
        // unprivileged process, `/proc/self/setgroups` is forced to `deny`
        // before `gid_map` may be written, and after that `setgroups(2)` is
        // permanently EPERM - including the empty call. There is nothing to
        // drop in that case, because the only groups reachable are the
        // mapped ones, so an EPERM on an empty list is already the state we
        // wanted rather than a failure.
        if let Err(e) = thread::set_thread_groups(&sec.groups)
            && !(e == Errno::PERM && sec.groups.is_empty())
        {
            return Err((Step::SetGroups, e));
        }
        // 5, 6. uid last: it is the one that loses the privilege.
        let gid = rustix::process::Gid::from_raw(u.gid);
        thread::set_thread_res_gid(Some(gid), Some(gid), Some(gid))
            .map_err(|e| (Step::SetGid, e))?;
        let uid = rustix::process::Uid::from_raw(u.uid);
        thread::set_thread_res_uid(Some(uid), Some(uid), Some(uid))
            .map_err(|e| (Step::SetUid, e))?;
        // 7.
        thread::set_keep_capabilities(false).map_err(|e| (Step::ClearKeepCaps, e))?;
        // 8. Restore effective from the preserved permitted set.
        thread::set_capabilities(
            None,
            thread::CapabilitySets {
                effective: sec.keep,
                permitted: sec.keep,
                inheritable: CapabilitySet::empty(),
            },
        )
        .map_err(|e| (Step::SetCaps, e))?;
        // 9. Ambient: nothing is raised. Raising the whole wanted set into
        //    inheritable to make it ambient is gratuitous and reopens the
        //    hole step 2 just closed.
    }

    // 11.
    if sec.no_new_privs {
        thread::set_no_new_privs(true).map_err(|e| (Step::NoNewPrivs, e))?;
    }

    // 12. As close to execve as possible, so as few syscalls as possible
    //     happen under the filter before the user's program starts.
    if !sec.seccomp.is_empty() {
        unsafe { sys::install_seccomp(&sec.seccomp) }.map_err(|e| (Step::Seccomp, e))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_unmapped_id_is_detected_from_the_map_file() {
        let dir = std::env::temp_dir().join("rustainer-uidmap-test");
        std::fs::create_dir_all(&dir).unwrap();
        // A single-ID namespace: exactly one id, and it is not 1000.
        let single = dir.join("single");
        std::fs::write(&single, "         0       1002          1\n").unwrap();
        assert!(id_is_mapped(single.to_str().unwrap(), 0));
        assert!(!id_is_mapped(single.to_str().unwrap(), 1000));

        // A subuid range.
        let range = dir.join("range");
        std::fs::write(&range, "0 1002 1\n1 100000 65536\n").unwrap();
        assert!(id_is_mapped(range.to_str().unwrap(), 0));
        assert!(id_is_mapped(range.to_str().unwrap(), 1000));
        assert!(!id_is_mapped(range.to_str().unwrap(), 70000));

        // No file at all means no user namespace: everything is reachable.
        assert!(id_is_mapped("/nonexistent/uid_map", 1000));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn the_default_set_excludes_sys_admin() {
        let d = default_capabilities();
        assert!(
            !d.contains(CapabilitySet::SYS_ADMIN),
            "CAP_SYS_ADMIN in the default set would make masked paths decoration: \
             a container that can mount gets a fresh procfs superblock with nothing masked"
        );
        assert!(d.contains(CapabilitySet::CHOWN));
        assert_eq!(d.iter().count(), 14, "Docker's default set is fourteen");
    }

    #[test]
    fn bounding_drop_is_everything_not_kept() {
        let keep = CapabilitySet::CHOWN | CapabilitySet::KILL;
        let s = Security::build(keep, None, Vec::new());
        for c in &s.bounding_drop {
            assert!(!keep.contains(*c), "{c:?} was meant to be kept");
        }
        assert_eq!(
            s.bounding_drop.len() + 2,
            CapabilitySet::all().iter().count()
        );
    }

    #[test]
    fn capability_names_resolve_with_and_without_the_prefix() {
        assert_eq!(
            capability_by_name("net_admin"),
            Some(CapabilitySet::NET_ADMIN)
        );
        assert_eq!(
            capability_by_name("CAP_SYS_ADMIN"),
            Some(CapabilitySet::SYS_ADMIN)
        );
        assert_eq!(capability_by_name("NOT_A_CAP"), None);
    }

    #[test]
    fn user_parses_uid_and_gid() {
        assert_eq!(
            UserSpec::parse("1000").unwrap(),
            UserSpec {
                uid: 1000,
                gid: 1000,
                groups: vec![]
            }
        );
        assert_eq!(
            UserSpec::parse("1000:2000").unwrap(),
            UserSpec {
                uid: 1000,
                gid: 2000,
                groups: vec![]
            }
        );
        // A name must say why it is refused, not just that it is.
        let e = UserSpec::parse("nobody").unwrap_err();
        assert!(e.contains("/etc/passwd"), "{e}");
    }

    #[test]
    fn the_default_profile_compiles_and_starts_with_an_arch_check() {
        let prog = compile(&default_profile()).expect("compile");
        assert!(prog.len() > 10);
        // seccompiler emits the architecture validation sequence first; the
        // very first instruction loads from offset 4 of seccomp_data, which
        // is `arch`. A filter that checks the syscall number first can be
        // bypassed through a different syscall ABI.
        const BPF_LD_W_ABS: u16 = 0x20;
        assert_eq!(prog[0].code, BPF_LD_W_ABS);
        assert_eq!(prog[0].k, 4, "the first load must be seccomp_data.arch");
    }

    #[test]
    fn clone3_is_denied_wholesale() {
        // It takes a POINTER to its flags, so a filter cannot examine them:
        // "deny namespace creation" is inexpressible for clone3 and the only
        // correct answer is to deny the call.
        let p = default_profile();
        assert!(p.syscalls[0].names.iter().any(|n| n == "clone3"));
        assert!(p.syscalls[0].names.iter().any(|n| n == "mount"));
    }

    #[test]
    fn a_profile_using_arg_rules_is_refused_not_silently_weakened() {
        let mut p = default_profile();
        p.syscalls[0].args = Some(serde_json::json!([{"index": 0}]));
        assert!(matches!(compile(&p), Err(SeccompError::ArgsUnsupported)));
    }

    #[test]
    fn an_unknown_syscall_name_is_skipped_rather_than_fatal() {
        let p = Profile {
            default_action: Action::Allow,
            default_errno: None,
            syscalls: vec![SyscallRule {
                names: vec!["definitely_not_a_syscall".into(), "mount".into()],
                action: Action::Errno,
                errno: Some(1),
                args: None,
            }],
        };
        assert!(compile(&p).is_ok(), "profiles are shared across kernels");
    }

    #[test]
    fn an_oci_profile_round_trips() {
        let json = r#"{"defaultAction":"SCMP_ACT_ALLOW",
                       "syscalls":[{"names":["mount"],"action":"SCMP_ACT_ERRNO","errnoRet":1}]}"#;
        let p: Profile = serde_json::from_str(json).expect("parse");
        assert_eq!(p.default_action, Action::Allow);
        assert_eq!(p.syscalls[0].errno, Some(1));
        assert!(compile(&p).is_ok());
    }

    #[test]
    fn every_step_describes_itself() {
        for v in [1u8, 2, 3, 4, 5, 6, 7, 8, 11, 12] {
            let s = Step::from_u8(v).expect("known step");
            assert!(!s.describe().is_empty());
        }
        assert!(Step::from_u8(99).is_none());
    }

    /// The ordering trap, asserted as a property of the code rather than
    /// hoped for: bounding must be dropped before the uid changes.
    #[test]
    fn the_documented_order_puts_bounding_before_uid() {
        assert!((Step::DropBounding as u8) < (Step::SetUid as u8));
        assert!((Step::ClearInheritable as u8) < (Step::KeepCaps as u8));
        assert!((Step::SetGroups as u8) < (Step::SetGid as u8));
        assert!((Step::SetGid as u8) < (Step::SetUid as u8));
        assert!((Step::ClearKeepCaps as u8) < (Step::SetCaps as u8));
        assert!((Step::NoNewPrivs as u8) < (Step::Seccomp as u8));
    }

    /// The stage 11 trap: our own map says every id is reachable, because
    /// we are in the initial namespace. The container's will not.
    #[test]
    fn a_new_single_id_namespace_maps_only_root() {
        let root = UserSpec {
            uid: 0,
            gid: 0,
            groups: Vec::new(),
        };
        assert!(root.unmapped_reason(true).is_none());

        for spec in [
            UserSpec {
                uid: 1000,
                gid: 1000,
                groups: Vec::new(),
            },
            UserSpec {
                uid: 0,
                gid: 1000,
                groups: Vec::new(),
            },
            UserSpec {
                uid: 1000,
                gid: 0,
                groups: Vec::new(),
            },
        ] {
            let why = spec
                .unmapped_reason(true)
                .expect("only id 0 exists in a single-extent map");
            assert!(why.contains("not mapped"), "{why}");
        }

        // And without a new namespace the question is about OUR map, where
        // in the initial namespace every id is reachable.
        if std::fs::read_to_string("/proc/self/uid_map")
            .is_ok_and(|m| m.split_whitespace().eq(["0", "0", "4294967295"]))
        {
            let spec = UserSpec {
                uid: 1000,
                gid: 1000,
                groups: Vec::new(),
            };
            assert!(spec.unmapped_reason(false).is_none());
        }
    }
}
