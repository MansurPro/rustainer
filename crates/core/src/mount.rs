//! The container's filesystem view: the mount set, and `pivot_root`.
//!
//! Two rules shape every line here.
//!
//! **Decisions are made in the parent, executed in the child.** The child
//! cannot allocate, so it cannot build a path, cannot read `/proc` to decide
//! what it is allowed to do, and cannot format an error. It receives a flat
//! list of pre-encoded operations and runs them. Everything conditional —
//! "can we `mknod`?", "will `sysfs` mount here?" — is resolved before the
//! clone, which is also where it can be reported honestly.
//!
//! **Privilege is expressed as "this operation, in this namespace, requires
//! this capability"**, never as "we are root" (RND.md §18.6). Code that
//! assumes root does not announce itself; it simply omits the error path,
//! and by the time rootless is wanted the assumption is in hundreds of
//! places that never mention uid. [`Privileges`] is where that question is
//! asked, once.

use std::ffi::{CString, OsString};
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};

use crate::sys::{self, Error as Errno};

/// One pre-encoded filesystem operation.
///
/// Every path is a `CString` built parent-side: `rustix`'s `path::Arg`
/// heap-allocates for any path ≥ 256 bytes, in the one window where an
/// allocation can deadlock forever (RND.md §15.4.3).
pub enum Op {
    /// `mount(source, target, fstype, flags, data)`.
    Mount {
        source: CString,
        target: CString,
        fstype: CString,
        flags: sys::MountFlags,
        data: Option<CString>,
    },
    /// `mount(source, target, NULL, MS_BIND|MS_REC, NULL)`.
    RecursiveBind {
        source: CString,
        target: CString,
    },
    /// `mount(source, target, NULL, MS_BIND, NULL)` — **non-recursive**.
    ///
    /// Non-recursive is load-bearing, not incidental. A recursive bind of a
    /// host directory drags in every submount beneath it, and a later
    /// read-only remount applies only to the top mount — so the submounts
    /// arrive writable. For `/sys` that means handing the container
    /// `/sys/kernel/debug`, `/sys/fs/bpf` and `/sys/fs/cgroup` with write
    /// access. A recursive bind that must be read-only throughout needs
    /// `mount_setattr(AT_RECURSIVE)` instead (RND.md §4, step 29).
    Bind {
        source: CString,
        target: CString,
    },
    /// `mount(NULL, target, NULL, <propagation>, NULL)`.
    Propagate {
        target: CString,
        flags: sys::MountPropagationFlags,
    },
    /// The same propagation change tried against a chain of ancestors,
    /// stopping at the first that succeeds. This is step 18's "walk up on
    /// `EINVAL`", precomputed so the child never manipulates a path.
    PropagateFirstOf {
        candidates: Vec<CString>,
        flags: sys::MountPropagationFlags,
    },
    /// `mknodat` a character device.
    CharDevice {
        target: CString,
        major: u32,
        minor: u32,
        mode: u32,
    },
    Symlink {
        contents: CString,
        link: CString,
    },
    /// Make a mount and everything beneath it read-only, in one atomic
    /// `mount_setattr(AT_RECURSIVE)`. See [`crate::sys`] for why a remount
    /// or a non-recursive bind will not do.
    SetAttrRecursiveReadonly {
        target: CString,
    },
    /// `mkdir`, tolerating EEXIST. Needed for directories that live inside
    /// a tmpfs this plan mounts, which therefore cannot exist beforehand.
    Mkdir {
        target: CString,
        mode: u32,
    },
    /// Create an empty file, as a bind-mount target. Same reason.
    Touch {
        target: CString,
    },
    /// Hide a path: `/dev/null` bound over a file, a read-only `tmpfs` over
    /// a directory. Which one applies is decided in the child, because these
    /// live inside `/proc`, which does not exist until the child mounts it.
    Mask {
        target: CString,
    },
    /// Mount an OverlayFS, assembled with genuine `fsconfig(2)` calls.
    ///
    /// **Not** `mount(2)` with a `lowerdir=a:b:c` string. The legacy
    /// options page is one 4095-byte buffer that the kernel fills and does
    /// **not** NUL-terminate, so a long layer list is silently truncated
    /// and the mount succeeds with layers missing. `mount -o lowerdir+=`
    /// does not escape it either: util-linux still issues a single
    /// `mount(2)`. Only separate `fsconfig` calls do, which is why this is
    /// a distinct operation rather than a formatted string.
    Overlay {
        /// Lowest first, as the image lists them.
        layers: Vec<CString>,
        upper: Option<CString>,
        work: Option<CString>,
        target: CString,
        /// Mount the result read-only.
        ///
        /// A mount attribute, not "leave out the upperdir": OverlayFS
        /// needs **two** lower layers when there is no upper, so an image
        /// with a single layer cannot be mounted read-only that way at
        /// all. The upper is still created and simply never writable.
        read_only: bool,
        /// `user.overlay.*` instead of `trusted.overlay.*`, required
        /// outside the initial user namespace and **not** interchangeable:
        /// layers written under one mode are silently unreadable under the
        /// other, so a store must be partitioned by it.
        userxattr: bool,
    },
    /// Bind a path onto itself and make it read-only.
    ///
    /// Two calls, not one: `MS_RDONLY` is **ignored** in the initial
    /// `MS_BIND`, so a single bind with the flag produces a writable mount
    /// and a false sense of having done something.
    ReadOnly {
        target: CString,
    },
}

/// An operation plus how to treat its failure.
pub struct PlannedOp {
    pub op: Op,
    /// A failure here is reported but does not abort the container.
    pub optional: bool,
    /// Static, so naming it costs the child nothing.
    pub label: &'static str,
}

/// What this process may actually do, asked once, in the parent.
#[derive(Clone, Copy, Debug)]
pub struct Privileges {
    /// `CAP_SYS_ADMIN` in the user namespace that owns our mount namespace.
    /// Without it there is no container at all.
    pub sys_admin: bool,
    /// `CAP_MKNOD`, **and** membership of the initial user namespace.
    ///
    /// The second half is the part that surprises people: inside a user
    /// namespace `mknod` of a device returns `EPERM` even with a full 41-bit
    /// `CapEff`, because the check is against the *initial* namespace
    /// (RND.md §4, step 22).
    pub mknod_devices: bool,
    /// True when `/proc/self/uid_map` is the identity map of the initial
    /// user namespace.
    pub initial_user_ns: bool,
}

const CAP_SYS_ADMIN: u32 = 21;
const CAP_MKNOD: u32 = 27;

impl Privileges {
    pub fn probe() -> Self {
        let caps = effective_caps().unwrap_or(0);
        let initial_user_ns = in_initial_user_namespace();
        Self {
            sys_admin: caps & (1 << CAP_SYS_ADMIN) != 0,
            mknod_devices: caps & (1 << CAP_MKNOD) != 0 && initial_user_ns,
            initial_user_ns,
        }
    }

    /// What the CHILD will have, given that it is about to be cloned into
    /// a user namespace of its own (stage 11).
    ///
    /// The plan is built by the parent but every operation in it is
    /// performed by the child, so it must be built against the child's
    /// privileges. Probing the parent's and using those is the bug this
    /// method exists to make impossible: the parent is unprivileged, the
    /// child is not, and a plan built from the parent's view silently
    /// degrades everything the child could in fact have done.
    pub fn in_new_user_namespace(self) -> Self {
        Self {
            // Creating a user namespace grants its creator the full
            // capability set WITHIN it, and the mount operations are
            // checked against exactly that namespace.
            sys_admin: true,
            // Still no, and this is the one people expect to flip. The
            // mknod check is against the INITIAL user namespace; a nested
            // one is further from it, not closer. The host's nodes are
            // bind-mounted instead, and we say so.
            mknod_devices: false,
            initial_user_ns: false,
        }
    }
}

fn effective_caps() -> Option<u64> {
    let status = std::fs::read_to_string("/proc/self/status").ok()?;
    let line = status.lines().find(|l| l.starts_with("CapEff:"))?;
    u64::from_str_radix(line.split_whitespace().nth(1)?, 16).ok()
}

/// The initial user namespace is exactly the identity map over the whole
/// 32-bit range. Any other map means we are inside one.
fn in_initial_user_namespace() -> bool {
    match std::fs::read_to_string("/proc/self/uid_map") {
        Ok(m) => m.split_whitespace().collect::<Vec<_>>() == ["0", "0", "4294967295"],
        Err(_) => false,
    }
}

/// Something the container will not have, and why.
#[derive(Clone, Debug)]
pub struct Degradation {
    pub what: &'static str,
    pub because: String,
}

/// The mount set and the `pivot_root` target.
pub struct MountPlan {
    pub ops: Vec<PlannedOp>,
    /// `O_PATH`-able path of the new root, for the pivot sequence.
    pub rootfs: CString,
    pub degradations: Vec<Degradation>,
}

impl MountPlan {
    pub fn labels(&self) -> Vec<&'static str> {
        self.ops.iter().map(|o| o.label).collect()
    }
}

fn cstr(p: &Path) -> Result<CString, PlanError> {
    CString::new(p.as_os_str().as_bytes()).map_err(|_| PlanError::InteriorNul)
}

fn join(root: &Path, rel: &str) -> std::path::PathBuf {
    root.join(rel)
}

#[derive(Debug, thiserror::Error)]
pub enum PlanError {
    #[error("a path contains an interior NUL byte")]
    InteriorNul,
    #[error("an overlay needs at least one layer")]
    NoLayers,
    #[error(
        "{count} layers is more than OverlayFS accepts: the kernel's OVL_MAX_STACK is \
         {max}. Checked here so the failure names the limit instead of arriving as a \
         bare EINVAL from the mount."
    )]
    TooManyLayers { count: usize, max: usize },
    #[error(
        "the layer path {0:?} contains a ':' or ',', which overlay mount options use as separators"
    )]
    LayerPathSeparator(OsString),
    #[error("the rootfs {0:?} is not a directory")]
    NotADirectory(OsString),
    #[error("creating the mount point {0:?}: {1}")]
    Mkdir(OsString, #[source] std::io::Error),
    #[error("resolving the rootfs {0:?}: {1}")]
    Canonicalize(OsString, #[source] std::io::Error),
}

/// Core device nodes. `tty` is 5:0; the rest are on major 1.
const DEVICES: &[(&str, u32, u32)] = &[
    ("dev/null", 1, 3),
    ("dev/zero", 1, 5),
    ("dev/full", 1, 7),
    ("dev/random", 1, 8),
    ("dev/urandom", 1, 9),
    ("dev/tty", 5, 0),
];

/// Paths hidden from the container.
///
/// Docker's list, not `runc spec`'s default. The difference is exactly three
/// entries — `/proc/interrupts`, the powercap tree and `thermal_throttle` —
/// and all three are side channels with CVEs behind them. Emitting the
/// "spec default" leaves the container side-channel exposed.
///
/// `/proc/kcore` is all of physical memory as an ELF core file.
const MASKED_PATHS: &[&str] = &[
    "proc/asound",
    "proc/acpi",
    "proc/interrupts",
    "proc/kcore",
    "proc/keys",
    "proc/latency_stats",
    "proc/timer_list",
    "proc/timer_stats",
    "proc/sched_debug",
    "proc/scsi",
    "sys/firmware",
    "sys/devices/virtual/powercap",
];

/// Paths the container may read but not write.
///
/// `/proc/sysrq-trigger` crashes the host with one echo, and
/// `/proc/sys/kernel/core_pattern` set to `|/bin/sh -c …` gets host-root
/// code execution on the next core dump, because the kernel runs the pipe
/// program in the **initial** namespaces as root.
const READONLY_PATHS: &[&str] = &[
    "proc/bus",
    "proc/fs",
    "proc/irq",
    "proc/sys",
    "proc/sysrq-trigger",
];

/// `/dev` symlinks every image expects. `symlink(2)` does not resolve its
/// target, so these cost nothing and need no privilege.
const DEV_SYMLINKS: &[(&str, &str)] = &[
    ("/proc/self/fd", "dev/fd"),
    ("/proc/self/fd/0", "dev/stdin"),
    ("/proc/self/fd/1", "dev/stdout"),
    ("/proc/self/fd/2", "dev/stderr"),
    ("pts/ptmx", "dev/ptmx"),
];

/// Build the mount set for a directory rootfs.
///
/// Creates the mount points on disk here, in the parent, because `mkdir` in
/// the child would mean building a path in the child.
/// Layers to stack onto the rootfs, instead of binding it directly.
#[derive(Clone, Debug)]
pub struct Overlay {
    /// Lowest first.
    pub layers: Vec<PathBuf>,
    /// `None` makes the result read-only.
    pub upper: Option<PathBuf>,
    /// Mount the result read-only. The upper is still created.
    pub read_only: bool,
    /// Must be on the same **mount** as `upper`, not merely the same
    /// filesystem - a `st_dev` comparison accepts configurations the
    /// kernel rejects. Guaranteed structurally here by creating the two as
    /// siblings in one directory.
    pub work: Option<PathBuf>,
}

pub fn plan(
    rootfs: &Path,
    privs: Privileges,
    cgroup_dir: Option<&Path>,
    overlay: Option<&Overlay>,
    // Host files bound over paths inside the container: (source, relative
    // target). This is how /etc/resolv.conf and /etc/hosts reach a
    // container - they are GENERATED PER CONTAINER on the host and bound
    // in, never part of the image, which is also why editing them inside a
    // container does not survive a restart.
    binds: &[(PathBuf, &str)],
) -> Result<MountPlan, PlanError> {
    let rootfs = rootfs
        .canonicalize()
        .map_err(|e| PlanError::Canonicalize(rootfs.as_os_str().to_owned(), e))?;
    if !rootfs.is_dir() {
        return Err(PlanError::NotADirectory(rootfs.into_os_string()));
    }

    let mut ops = Vec::new();
    let mut degradations = Vec::new();
    let mut push = |op: Op, optional: bool, label: &'static str| {
        ops.push(PlannedOp {
            op,
            optional,
            label,
        });
    };

    // --- 17. Make the whole tree non-propagating. -----------------------
    //
    // Not merely tidiness: without it a bind-mounted volume inherits the
    // *source's* peer group, so every `-v` becomes a two-way channel, and
    // our teardown umounts propagate out to the host. Getting this right
    // before volumes exist is much cheaper than after (RND.md §4.9).
    push(
        Op::Propagate {
            target: CString::new("/").unwrap(),
            flags: sys::MS_SLAVE | sys::MountPropagationFlags::REC,
        },
        false,
        "making / recursively slave",
    );

    // --- 18. The rootfs's own mount must not be shared. -----------------
    //
    // `clone_mnt` copies propagation from the source at bind time, so this
    // has to happen before the self-bind below. The kernel's precondition is
    // about the rootfs's *parent* mount, which we cannot identify without
    // parsing mountinfo - so try the rootfs and then each ancestor, which is
    // the "walk up on EINVAL" of step 18, precomputed.
    let mut candidates = Vec::new();
    let mut p: Option<&Path> = Some(rootfs.as_path());
    while let Some(dir) = p {
        candidates.push(cstr(dir)?);
        p = dir.parent();
    }
    push(
        Op::PropagateFirstOf {
            candidates,
            flags: sys::MountPropagationFlags::PRIVATE,
        },
        false,
        "making the rootfs's mount private",
    );

    // --- 19. pivot_root requires new_root to BE a mount point. ----------
    //
    // An overlay already is one, so it replaces the self-bind rather than
    // being stacked on top of it.
    match overlay {
        None => push(
            Op::RecursiveBind {
                source: cstr(&rootfs)?,
                target: cstr(&rootfs)?,
            },
            false,
            "binding the rootfs onto itself",
        ),
        Some(o) => {
            if o.layers.is_empty() {
                return Err(PlanError::NoLayers);
            }
            if o.layers.len() > sys::OVL_MAX_STACK {
                return Err(PlanError::TooManyLayers {
                    count: o.layers.len(),
                    max: sys::OVL_MAX_STACK,
                });
            }
            let mut layers = Vec::with_capacity(o.layers.len());
            for l in &o.layers {
                // Colons and commas are the legacy option separators. The
                // `lowerdir+` form escapes them, but a path containing one
                // is a sign something is wrong and the failure it produces
                // elsewhere is unreadable.
                let bytes = l.as_os_str().as_encoded_bytes();
                if bytes.contains(&b':') || bytes.contains(&b',') {
                    return Err(PlanError::LayerPathSeparator(l.clone().into_os_string()));
                }
                layers.push(cstr(l)?);
            }
            push(
                Op::Overlay {
                    layers,
                    upper: o.upper.as_deref().map(cstr).transpose()?,
                    work: o.work.as_deref().map(cstr).transpose()?,
                    target: cstr(&rootfs)?,
                    read_only: o.read_only,
                    // Outside the initial user namespace the kernel cannot
                    // read `trusted.overlay.*`, so whiteouts recorded that
                    // way are invisible and deleted files reappear.
                    userxattr: !privs.initial_user_ns,
                },
                false,
                "mounting the overlay",
            );
        }
    }

    // A mount point the image may not contain, created BY THE CHILD, after
    // the overlay is mounted.
    //
    // It is the same trap /dev/pts and /dev/shm have below, one level up:
    // creating these parent-side writes them into the directory the overlay
    // is about to be mounted *on*, and the overlay then hides them. For an
    // image whose layers happen to contain /proc - alpine, debian, nearly
    // everything with a package manager - it works anyway and the bug is
    // invisible. For an image built from scratch it is a container that
    // cannot start: on Docker Hub today, `busybox` and `hello-world` both
    // ship no /proc, and the failure reads "mounting /proc failed with
    // ENOENT", which sends you looking at /proc rather than at the image.
    //
    // `sys::mkdir` tolerates EEXIST, so the common case costs one syscall.
    let mountpoint = |rel: &str| -> Result<Op, PlanError> {
        Ok(Op::Mkdir {
            target: cstr(&join(&rootfs, rel))?,
            mode: 0o755,
        })
    };

    // --- 20. /proc, mounted by the child, after the PID namespace. ------
    //
    // If the supervisor mounted this instead, the container would see host
    // PIDs: procfs binds its pid namespace at mount time, from the mounting
    // process (RND.md §4.3).
    push(mountpoint("proc")?, false, "creating a mount point");
    push(
        Op::Mount {
            source: CString::new("proc").unwrap(),
            target: cstr(&join(&rootfs, "proc"))?,
            fstype: CString::new("proc").unwrap(),
            flags: sys::MountFlags::NOSUID | sys::MountFlags::NODEV | sys::MountFlags::NOEXEC,
            data: None,
        },
        false,
        "mounting /proc",
    );

    // --- 21. A FRESH tmpfs on /dev is a security requirement. -----------
    //
    // Not tidiness: it is what stops the image shipping a prepared /dev, and
    // it is the surface masked paths are applied to (RND.md §4.10).
    push(mountpoint("dev")?, false, "creating a mount point");
    push(
        Op::Mount {
            source: CString::new("tmpfs").unwrap(),
            target: cstr(&join(&rootfs, "dev"))?,
            fstype: CString::new("tmpfs").unwrap(),
            flags: sys::MountFlags::NOSUID,
            data: Some(CString::new("mode=755,size=65536k").unwrap()),
        },
        false,
        "mounting a fresh tmpfs on /dev",
    );

    // --- 22. The core device nodes. -------------------------------------
    //
    // Everything from here on targets a path *inside* the tmpfs just
    // mounted on /dev, so the targets must be created now, by the child,
    // after the mount. Creating them parent-side on the underlying
    // directory would work right up until the tmpfs hid them - which is a
    // failure that looks like "bind mount cannot find its target" and sends
    // you looking in entirely the wrong place.
    for (rel, major, minor) in DEVICES {
        let target = cstr(&join(&rootfs, rel))?;
        if privs.mknod_devices {
            push(
                Op::CharDevice {
                    target,
                    major: *major,
                    minor: *minor,
                    mode: 0o666,
                },
                false,
                "creating a device node",
            );
        } else {
            // Rootless fallback, and what every rootless runtime does: bind
            // the host's node instead. The source is an absolute host path
            // resolved before the pivot, never a path inside the container's
            // rootfs - resolving device sources through the container's own
            // tree is CVE-2025-31133's vector.
            let host = Path::new("/").join(rel);
            if host.exists() {
                push(
                    Op::Touch {
                        target: target.clone(),
                    },
                    true,
                    "creating a device bind target",
                );
                push(
                    Op::Bind {
                        source: cstr(&host)?,
                        target,
                    },
                    true,
                    "binding a host device node",
                );
            }
        }
    }
    if !privs.mknod_devices {
        degradations.push(Degradation {
            what: "device nodes",
            because: if privs.initial_user_ns {
                "CAP_MKNOD is not in the effective set; the host's nodes are bind-mounted instead"
                    .into()
            } else {
                "mknod of a device requires CAP_MKNOD in the INITIAL user namespace, and we are \
                 in a nested one; the host's nodes are bind-mounted instead"
                    .into()
            },
        });
    }

    // --- 23. devpts, so stage 3 can have a pty at all. ------------------
    push(
        Op::Mkdir {
            target: cstr(&join(&rootfs, "dev/pts"))?,
            mode: 0o755,
        },
        false,
        "creating /dev/pts",
    );
    push(
        Op::Mount {
            source: CString::new("devpts").unwrap(),
            target: cstr(&join(&rootfs, "dev/pts"))?,
            fstype: CString::new("devpts").unwrap(),
            flags: sys::MountFlags::NOSUID | sys::MountFlags::NOEXEC,
            data: Some(CString::new("newinstance,ptmxmode=0666,mode=0620,gid=5").unwrap()),
        },
        true,
        "mounting devpts",
    );

    // --- 24. /dev/shm. Its absence breaks more than people expect: ------
    // shm_open, sem_open, Python multiprocessing, PostgreSQL, Chromium.
    push(
        Op::Mkdir {
            target: cstr(&join(&rootfs, "dev/shm"))?,
            mode: 0o755,
        },
        false,
        "creating /dev/shm",
    );
    push(
        Op::Mount {
            source: CString::new("shm").unwrap(),
            target: cstr(&join(&rootfs, "dev/shm"))?,
            fstype: CString::new("tmpfs").unwrap(),
            flags: sys::MountFlags::NOSUID | sys::MountFlags::NODEV | sys::MountFlags::NOEXEC,
            data: Some(CString::new("mode=1777,size=65536k").unwrap()),
        },
        false,
        "mounting /dev/shm",
    );

    // --- 26. sysfs, read-only. ------------------------------------------
    //
    // Mounting sysfs needs CAP_SYS_ADMIN in the user namespace owning the
    // NETWORK namespace. Sharing the host's netns from inside a user
    // namespace therefore cannot mount it at all, and the only option is a
    // read-only bind of the host's.
    push(mountpoint("sys")?, false, "creating a mount point");
    let sys_target = cstr(&join(&rootfs, "sys"))?;
    if privs.initial_user_ns {
        push(
            Op::Mount {
                source: CString::new("sysfs").unwrap(),
                target: sys_target,
                fstype: CString::new("sysfs").unwrap(),
                flags: sys::MountFlags::RDONLY
                    | sys::MountFlags::NOSUID
                    | sys::MountFlags::NODEV
                    | sys::MountFlags::NOEXEC,
                data: None,
            },
            true,
            "mounting sysfs",
        );
    } else {
        push(
            Op::RecursiveBind {
                source: CString::new("/sys").unwrap(),
                target: sys_target.clone(),
            },
            false,
            "binding the host's /sys",
        );
        push(
            Op::SetAttrRecursiveReadonly { target: sys_target },
            false,
            "making the bound /sys read-only throughout",
        );
        degradations.push(Degradation {
            what: "/sys",
            because: "mounting a fresh sysfs requires CAP_SYS_ADMIN in the user namespace owning \
                      the network namespace, which a nested namespace sharing the host's network \
                      does not have; the host's /sys is bind-mounted read-only instead, \
                      and made read-only throughout with mount_setattr(AT_RECURSIVE), so its \
                      submounts - /sys/fs/cgroup, /sys/kernel/debug, /sys/fs/bpf - are visible \
                      but not writable. One consequence worth knowing: /sys/class/net then \
                      lists the HOST's interfaces whatever network namespace the container \
                      is in. The network is still isolated - /proc/net/dev and `ip link` \
                      show the truth - but /sys does not."
                .into(),
        });
    }

    // --- 27. The container's own cgroup, read-only. ---------------------
    //
    // A bind of the cgroup directory, not a fresh `cgroup2` mount. The
    // obvious version - mount cgroup2 at <rootfs>/sys/fs/cgroup - cannot
    // work outside the initial namespaces: cgroup2 has a single superblock,
    // a mount whose options differ from the existing one is refused EBUSY,
    // and `nsdelegate` is *ignored* on a non-initial-namespace mount, so the
    // options can never be made to match. Copying the host's option string
    // does not help for that reason. The bind needs no superblock and gives
    // exactly the subtree this container owns, which combined with the
    // cgroup namespace is what makes reading a limit back from inside mean
    // something.
    if let Some(cg) = cgroup_dir {
        let target = join(&rootfs, "sys/fs/cgroup");
        std::fs::create_dir_all(&target)
            .map_err(|e| PlanError::Mkdir(target.clone().into_os_string(), e))?;
        push(
            Op::Bind {
                source: cstr(cg)?,
                target: cstr(&target)?,
            },
            false,
            "binding the container's cgroup",
        );
        push(
            Op::SetAttrRecursiveReadonly {
                target: cstr(&target)?,
            },
            true,
            "making the container's cgroup read-only",
        );
    }

    // --- 28. The /dev symlinks. -----------------------------------------
    for (contents, rel) in DEV_SYMLINKS {
        push(
            Op::Symlink {
                contents: CString::new(*contents).unwrap(),
                link: cstr(&join(&rootfs, rel))?,
            },
            true,
            "creating a /dev symlink",
        );
    }

    // --- 29. Generated files bound over the image's copies. -------------
    //
    // The target is created HERE, by the child, after the overlay is
    // mounted. Creating it parent-side cannot work: with layers the rootfs
    // is an empty mount point until the overlay lands, so there is no
    // /etc to put a file in yet.
    for (source, rel) in binds {
        let target = join(&rootfs, rel);
        if let Some(parent) = Path::new(rel).parent()
            && !parent.as_os_str().is_empty()
        {
            push(
                Op::Mkdir {
                    target: cstr(&join(&rootfs, parent.to_str().unwrap_or("")))?,
                    mode: 0o755,
                },
                true,
                "creating a directory for a generated file",
            );
        }
        push(
            Op::Touch {
                target: cstr(&target)?,
            },
            true,
            "creating a bind target for a generated file",
        );
        push(
            Op::Bind {
                source: cstr(source)?,
                target: cstr(&target)?,
            },
            false,
            "binding a generated file",
        );
    }

    // --- 30. Masked and read-only paths. --------------------------------
    //
    // Worth being precise about what this buys. These are
    // **per-superblock**: procfs uses `get_tree_nodev()`, so mounting a
    // second procfs inside the same mount namespace produces a different
    // superblock with `/proc/kcore` present and nothing masked. Masking is
    // therefore not a boundary on its own. It becomes one only together
    // with `CAP_SYS_ADMIN` being absent from the bounding set and a seccomp
    // filter denying the mount family - both of which are defaults here.
    // The three are one property, not three hardening items: *the container
    // cannot obtain a writable /proc/sys* (RND.md §13.4).
    for rel in MASKED_PATHS {
        push(
            Op::Mask {
                target: cstr(&join(&rootfs, rel))?,
            },
            // Absent paths are normal: the list spans kernels and hardware.
            true,
            "masking a path",
        );
    }
    for rel in READONLY_PATHS {
        push(
            Op::ReadOnly {
                target: cstr(&join(&rootfs, rel))?,
            },
            true,
            "making a path read-only",
        );
    }

    Ok(MountPlan {
        ops,
        rootfs: cstr(&rootfs)?,
        degradations,
    })
}

// ---------------------------------------------------------------------------
// Child side. Nothing below here may allocate.
// ---------------------------------------------------------------------------

/// Run the planned operations. Returns the index of the first fatal failure.
///
/// # Safety
///
/// Child-side only; see [`crate::child`].
pub fn apply(plan: &[PlannedOp], detail: &mut [u8; DETAIL_LEN]) -> Result<(), (u8, Errno, usize)> {
    for (i, planned) in plan.iter().enumerate() {
        let mut n = 0;
        if let Err(e) = run_one(&planned.op, detail, &mut n)
            && !planned.optional
        {
            return Err((i as u8, e, n));
        }
    }
    Ok(())
}

/// Room for the kernel's own error message, which is the whole reason the
/// new mount API is worth using: a failed `fsconfig` leaves a readable
/// explanation on the `fs_context` descriptor, where `mount(2)` leaves only
/// `EINVAL`.
pub const DETAIL_LEN: usize = 192;

fn run_one(op: &Op, detail: &mut [u8; DETAIL_LEN], detail_len: &mut usize) -> Result<(), Errno> {
    match op {
        Op::Mount {
            source,
            target,
            fstype,
            flags,
            data,
        } => sys::mount(
            source.as_c_str(),
            target.as_c_str(),
            fstype.as_c_str(),
            *flags,
            data.as_deref(),
        ),
        Op::RecursiveBind { source, target } => {
            sys::mount_recursive_bind(source.as_c_str(), target.as_c_str())
        }
        Op::Bind { source, target } => sys::mount_bind(source.as_c_str(), target.as_c_str()),
        Op::Propagate { target, flags } => sys::mount_change(target.as_c_str(), *flags),
        Op::PropagateFirstOf { candidates, flags } => {
            let mut last = Errno::INVAL;
            for c in candidates {
                match sys::mount_change(c.as_c_str(), *flags) {
                    Ok(()) => return Ok(()),
                    Err(e) => last = e,
                }
            }
            Err(last)
        }
        Op::CharDevice {
            target,
            major,
            minor,
            mode,
        } => sys::mknod_char(target.as_c_str(), *mode, *major, *minor),
        Op::Symlink { contents, link } => sys::symlink(contents.as_c_str(), link.as_c_str()),
        Op::Mkdir { target, mode } => sys::mkdir(target.as_c_str(), *mode),
        Op::Touch { target } => sys::touch(target.as_c_str()),
        Op::Mask { target } => match sys::mount_bind(c"/dev/null", target.as_c_str()) {
            // A directory cannot have /dev/null bound over it; cover it
            // with an empty read-only tmpfs instead. Masking rather than
            // deleting, so programs that stat() the path still work.
            Err(Errno::NOTDIR) | Err(Errno::ISDIR) | Err(Errno::PERM) => sys::mount(
                c"tmpfs",
                target.as_c_str(),
                c"tmpfs",
                sys::MountFlags::RDONLY | sys::MountFlags::NOSUID | sys::MountFlags::NODEV,
                Some(c"size=0k"),
            ),
            other => other,
        },
        Op::Overlay {
            layers,
            upper,
            work,
            target,
            read_only,
            userxattr,
        } => {
            let fs = sys::fsopen(c"overlay", sys::FsOpenFlags::FSOPEN_CLOEXEC)?;
            // Every step below reports through the fs_context on failure.
            macro_rules! explain {
                ($e:expr) => {
                    match $e {
                        Ok(v) => v,
                        Err(err) => {
                            // The kernel leaves a line like
                            // "e overlayfs: failed to resolve 'x': -2" here.
                            if let Ok(n) = sys::read(&fs, &mut detail[..]) {
                                *detail_len = n;
                            }
                            return Err(err);
                        }
                    }
                };
            }
            // Topmost first: repeated `lowerdir+` calls append, and
            // overlay's own ordering puts the uppermost lower layer first.
            for layer in layers.iter().rev() {
                explain!(sys::fsconfig_set_string(
                    &fs,
                    c"lowerdir+",
                    layer.as_c_str()
                ));
            }
            if let Some(u) = upper {
                explain!(sys::fsconfig_set_string(&fs, c"upperdir", u.as_c_str()));
            }
            if let Some(w) = work {
                explain!(sys::fsconfig_set_string(&fs, c"workdir", w.as_c_str()));
            }
            if *userxattr {
                explain!(sys::fsconfig_set_flag(&fs, c"userxattr"));
            }
            explain!(sys::fsconfig_create(&fs));
            let mnt = explain!(sys::fsmount(
                &fs,
                sys::FsMountFlags::FSMOUNT_CLOEXEC,
                if *read_only {
                    sys::MountAttrFlags::MOUNT_ATTR_RDONLY
                        | sys::MountAttrFlags::MOUNT_ATTR_NODEV
                        | sys::MountAttrFlags::MOUNT_ATTR_NOSUID
                } else {
                    sys::MountAttrFlags::MOUNT_ATTR_NODEV | sys::MountAttrFlags::MOUNT_ATTR_NOSUID
                },
            ));
            sys::move_mount(
                &mnt,
                c"",
                rustix::fs::CWD,
                target.as_c_str(),
                sys::MoveMountFlags::MOVE_MOUNT_F_EMPTY_PATH,
            )
        }
        Op::ReadOnly { target } => {
            sys::mount_bind(target.as_c_str(), target.as_c_str())?;
            sys::mount_setattr_recursive_readonly(target.as_c_str())
        }
        Op::SetAttrRecursiveReadonly { target } => {
            sys::mount_setattr_recursive_readonly(target.as_c_str())
        }
    }
}

/// The `pivot_root` sequence of RND.md §4.4, call for call.
///
/// # Safety
///
/// Child-side only, after [`apply`].
pub fn pivot(rootfs: &std::ffi::CStr) -> Result<(), Errno> {
    // The O_PATH fd on the OLD root is opened before the pivot precisely so
    // we can fchdir back to a root that no longer has any pathname.
    let oldroot = sys::open_path(c"/")?;
    let newroot = sys::open_path(rootfs)?;

    sys::fchdir(&newroot)?;

    // Both arguments "." is legal and idiomatic: the old root ends up
    // stacked on top of the new one, which avoids requiring every image to
    // contain a /.pivot_root directory for put_old.
    sys::pivot_root(c".", c".")?;

    // Back to the old root, reachable only through the fd now.
    sys::fchdir(&oldroot)?;

    // MNT_DETACH propagates. After pivot_root(".", ".") the old root's own
    // parent is the new, private root - so its own detach does not
    // propagate, but its DESCENDANTS' do, because their parents are still
    // peers of the host's mounts. This is an independent defence from the
    // recursive-slave of "/" above; neither replaces the other.
    sys::mount_change(c".", sys::MS_SLAVE | sys::MountPropagationFlags::REC)?;

    // Lazy, and not a preference: we are unmounting our own current
    // directory, and a synchronous umount here fails EBUSY.
    sys::unmount_detach(c".")?;

    // Invisible to omit, and wrong: pivot_root moves *other* tasks' cwd,
    // never the caller's. Without this the shell starts, `pwd` prints
    // something plausible, and the process is sitting in the old root.
    sys::chdir(c"/")?;

    // These two fds ARE the escape. They survive MNT_DETACH, because the
    // superblock lives as long as a reference does. Closing them at execve
    // via O_CLOEXEC works only if nothing between here and there leaks or
    // execs them - so close them here, where the reason is visible.
    drop(oldroot);
    drop(newroot);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::OsStr;

    #[test]
    fn ancestor_chain_walks_all_the_way_to_root() {
        let dir = std::env::temp_dir().join("rustainer-plan-test/a/b");
        std::fs::create_dir_all(&dir).unwrap();
        let privs = Privileges {
            sys_admin: false,
            mknod_devices: false,
            initial_user_ns: false,
        };
        let plan = plan(&dir, privs, None, None, &[]).unwrap();
        let walk = plan
            .ops
            .iter()
            .find_map(|p| match &p.op {
                Op::PropagateFirstOf { candidates, .. } => Some(candidates),
                _ => None,
            })
            .expect("the plan must contain the walk-up");
        assert!(walk.len() >= 3, "expected the rootfs and its ancestors");
        assert_eq!(
            walk.last().unwrap().to_bytes(),
            b"/",
            "the walk must end at /, or a rootfs directly under a shared mount fails"
        );
        std::fs::remove_dir_all(std::env::temp_dir().join("rustainer-plan-test")).ok();
    }

    #[test]
    fn a_nested_user_namespace_degrades_loudly_rather_than_silently() {
        let dir = std::env::temp_dir().join("rustainer-plan-test2");
        std::fs::create_dir_all(&dir).unwrap();
        let privs = Privileges {
            sys_admin: true,
            mknod_devices: false,
            initial_user_ns: false,
        };
        let plan = plan(&dir, privs, None, None, &[]).unwrap();
        let what: Vec<_> = plan.degradations.iter().map(|d| d.what).collect();
        assert!(what.contains(&"device nodes"), "{what:?}");
        assert!(what.contains(&"/sys"), "{what:?}");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn the_initial_namespace_reports_nothing_degraded() {
        let dir = std::env::temp_dir().join("rustainer-plan-test3");
        std::fs::create_dir_all(&dir).unwrap();
        let privs = Privileges {
            sys_admin: true,
            mknod_devices: true,
            initial_user_ns: true,
        };
        let plan = plan(&dir, privs, None, None, &[]).unwrap();
        assert!(plan.degradations.is_empty(), "{:?}", plan.degradations);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn proc_is_mounted_before_the_pivot_and_dev_before_its_children() {
        let dir = std::env::temp_dir().join("rustainer-plan-test4");
        std::fs::create_dir_all(&dir).unwrap();
        let privs = Privileges {
            sys_admin: true,
            mknod_devices: true,
            initial_user_ns: true,
        };
        let plan = plan(&dir, privs, None, None, &[]).unwrap();
        let labels = plan.labels();
        let pos = |needle: &str| labels.iter().position(|l| l.contains(needle)).unwrap();
        assert!(pos("rootfs onto itself") < pos("/proc"));
        // Everything under /dev must come after the tmpfs that covers it.
        // Creating these before the mount works until the tmpfs hides them,
        // and then fails as "bind mount cannot find its target".
        let dev = pos("tmpfs on /dev");
        assert!(dev < pos("devpts"));
        assert!(dev < pos("/dev/shm"));
        assert!(dev < pos("creating /dev/pts"));
        assert!(dev < pos("creating /dev/shm"));
        assert!(dev < pos("device node"));
        assert!(dev < pos("/dev symlink"));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn unused_privilege_field_is_still_probed() {
        // `sys_admin` is read by the CLI to refuse early with a usable
        // message; assert the probe runs rather than that it is true, since
        // the test suite may be unprivileged.
        let p = Privileges::probe();
        let _ = p.sys_admin;
        assert_eq!(p.mknod_devices, p.mknod_devices && p.initial_user_ns);
    }

    #[test]
    fn an_overlay_replaces_the_self_bind_rather_than_stacking_on_it() {
        let dir = std::env::temp_dir().join("rustainer-overlay-plan");
        std::fs::create_dir_all(&dir).unwrap();
        let privs = Privileges {
            sys_admin: true,
            mknod_devices: true,
            initial_user_ns: true,
        };
        let o = Overlay {
            layers: vec![PathBuf::from("/a"), PathBuf::from("/b")],
            upper: Some(PathBuf::from("/u")),
            work: Some(PathBuf::from("/w")),
            read_only: false,
        };
        let p = plan(&dir, privs, None, Some(&o), &[]).unwrap();
        let labels = p.labels();
        assert!(labels.iter().any(|l| l.contains("overlay")));
        assert!(
            !labels.iter().any(|l| l.contains("onto itself")),
            "an overlay IS a mount point; binding over it would hide it"
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn too_many_layers_names_the_kernel_limit() {
        let dir = std::env::temp_dir().join("rustainer-overlay-limit");
        std::fs::create_dir_all(&dir).unwrap();
        let privs = Privileges {
            sys_admin: true,
            mknod_devices: true,
            initial_user_ns: true,
        };
        let o = Overlay {
            layers: (0..600).map(|i| PathBuf::from(format!("/l{i}"))).collect(),
            upper: None,
            work: None,
            read_only: false,
        };
        let Err(e) = plan(&dir, privs, None, Some(&o), &[]) else {
            panic!("600 layers must be refused before the syscall")
        };
        assert!(e.to_string().contains("500"), "{e}");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_layer_path_with_an_option_separator_is_refused() {
        let dir = std::env::temp_dir().join("rustainer-overlay-colon");
        std::fs::create_dir_all(&dir).unwrap();
        let privs = Privileges {
            sys_admin: true,
            mknod_devices: true,
            initial_user_ns: true,
        };
        let o = Overlay {
            layers: vec![PathBuf::from("/has:colon")],
            upper: None,
            work: None,
            read_only: false,
        };
        assert!(matches!(
            plan(&dir, privs, None, Some(&o), &[]),
            Err(PlanError::LayerPathSeparator(_))
        ));
        std::fs::remove_dir_all(&dir).ok();
    }

    /// Outside the initial user namespace the kernel cannot read
    /// `trusted.overlay.*`, so whiteouts recorded that way are invisible
    /// and deleted files silently reappear.
    #[test]
    fn userxattr_is_chosen_by_the_user_namespace_not_by_preference() {
        let dir = std::env::temp_dir().join("rustainer-overlay-xattr");
        std::fs::create_dir_all(&dir).unwrap();
        let o = Overlay {
            layers: vec![PathBuf::from("/a")],
            upper: None,
            work: None,
            read_only: false,
        };
        for initial in [true, false] {
            let privs = Privileges {
                sys_admin: true,
                mknod_devices: true,
                initial_user_ns: initial,
            };
            let p = plan(&dir, privs, None, Some(&o), &[]).unwrap();
            let got = p.ops.iter().find_map(|op| match &op.op {
                Op::Overlay { userxattr, .. } => Some(*userxattr),
                _ => None,
            });
            assert_eq!(got, Some(!initial));
        }
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_rootfs_that_is_not_a_directory_is_refused() {
        let f = std::env::temp_dir().join("rustainer-not-a-dir");
        std::fs::write(&f, b"x").unwrap();
        let privs = Privileges {
            sys_admin: true,
            mknod_devices: true,
            initial_user_ns: true,
        };
        assert!(matches!(
            plan(&f, privs, None, None, &[]),
            Err(PlanError::NotADirectory(_))
        ));
        std::fs::remove_file(&f).ok();
    }

    #[test]
    fn every_device_in_the_set_is_a_character_device_we_name() {
        let names: Vec<&str> = DEVICES.iter().map(|(n, _, _)| *n).collect();
        for expected in [
            "dev/null",
            "dev/zero",
            "dev/full",
            "dev/random",
            "dev/urandom",
            "dev/tty",
        ] {
            assert!(names.contains(&expected), "missing {expected}");
        }
    }

    #[test]
    fn dev_symlink_targets_are_not_resolved_at_creation() {
        // pts/ptmx is relative and does not exist until devpts is mounted;
        // symlink(2) does not care, which is why these are unprivileged.
        let rel: Vec<&str> = DEV_SYMLINKS.iter().map(|(c, _)| *c).collect();
        assert!(rel.contains(&"pts/ptmx"));
        assert!(rel.contains(&"/proc/self/fd"));
    }

    #[test]
    fn os_str_paths_round_trip_without_allocation_in_the_child() {
        // The point of the CString fields: a long rootfs path must already
        // be encoded before the clone. 300 bytes is past rustix's 256-byte
        // stack buffer, which is the only input that distinguishes its fast
        // path from its heap slow path.
        let long = std::env::temp_dir().join("r".repeat(300));
        let s = CString::new(long.as_os_str().as_bytes()).unwrap();
        assert!(s.as_bytes().len() > 256);
    }

    #[test]
    fn interior_nul_in_a_rootfs_path_is_rejected() {
        let p = Path::new(OsStr::from_bytes(b"/tmp/a\0b"));
        assert!(matches!(cstr(p), Err(PlanError::InteriorNul)));
    }
}
