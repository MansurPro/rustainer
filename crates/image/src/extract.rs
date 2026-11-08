//! Layer extraction — the most dangerous code in the project.
//!
//! The threat model, stated plainly: the layer tarball is
//! **attacker-controlled input**, parsed by a root-privileged process,
//! whose output becomes the root filesystem of a container. Every hazard
//! guarded against below has produced a real CVE, several of them in the
//! reference implementations, several of them recently.
//!
//! Two mitigations carry the weight.
//!
//! **M1 — every filesystem operation goes through a pinned directory
//! handle**, with paths resolved by `openat2(RESOLVE_IN_ROOT |
//! RESOLVE_NO_MAGICLINKS | RESOLVE_NO_XDEV)`. This is the only mitigation
//! that is not a race: it moves the containment decision into the kernel's
//! path walk instead of our string comparisons. Lexical checks cannot close
//! the symlink-swap hole, because between "I verified this path is inside
//! the root" and "I opened it" another entry of the same tarball can change
//! the answer. Moby's own extractor was still lexical in August 2026
//! (CVE-2026-17106), so "how the mature implementations do it" is a
//! statement of intent, not evidence that an approach suffices.
//!
//! **M2 — extraction goes into a fresh, empty, per-attempt directory.**
//! Most escapes need something pre-existing to aim at, and cleanup becomes
//! one recursive remove.

use std::ffi::{CString, OsStr};
use std::io::Read;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::PermissionsExt;
use std::path::{Component, Path, PathBuf};

use rustix::fs::{AtFlags, Mode, OFlags, ResolveFlags};

/// Limits, all three cheap, all three independent.
///
/// A compressed layer that expands to a disk-filling tree is
/// CVE-2017-14992's shape. The descriptor's declared `size` bounds the
/// compressed stream; these bound what comes out of it.
#[derive(Clone, Copy, Debug)]
pub struct Limits {
    pub max_bytes: u64,
    pub max_entries: u64,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            max_bytes: 16 * 1024 * 1024 * 1024,
            max_entries: 2_000_000,
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("layer entry {path:?}: {reason}")]
    Rejected { path: String, reason: &'static str },
    #[error("layer is larger than the {limit} byte limit after decompression")]
    TooBig { limit: u64 },
    #[error("layer has more than {limit} entries")]
    TooManyEntries { limit: u64 },
    #[error("reading the layer archive: {0}")]
    Archive(#[source] std::io::Error),
    #[error("{op} {path:?}: {source}")]
    Io {
        op: &'static str,
        path: String,
        #[source]
        source: std::io::Error,
    },
}

fn rejected(path: &Path, reason: &'static str) -> Error {
    Error::Rejected {
        path: path.display().to_string(),
        reason,
    }
}

/// What was skipped, so a strict mode can fail where this one warns.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Report {
    pub entries: u64,
    pub bytes: u64,
    /// Device nodes refused. Never created from layer content.
    pub devices_skipped: Vec<String>,
    /// `security.*` and `system.*` extended attributes dropped.
    pub xattrs_dropped: u64,
    /// Ownership we could not set, because the uid is not mapped here.
    pub ownership_skipped: u64,
    pub whiteouts: u64,
    pub opaque_dirs: u64,
    /// Directories whose mode was widened so extraction could continue.
    pub modes_relaxed: u64,
}

/// A pinned root. Every path in this module resolves through it.
pub struct SafeRoot {
    fd: rustix::fd::OwnedFd,
    path: PathBuf,
}

impl SafeRoot {
    pub fn open(dir: &Path) -> Result<Self, Error> {
        let c = cpath(dir)?;
        let fd = rustix::fs::open(
            c.as_c_str(),
            OFlags::DIRECTORY | OFlags::RDONLY | OFlags::CLOEXEC,
            Mode::empty(),
        )
        .map_err(|e| Error::Io {
            op: "opening the staging directory",
            path: dir.display().to_string(),
            source: e.into(),
        })?;
        Ok(Self {
            fd,
            path: dir.to_path_buf(),
        })
    }

    /// Resolve `rel` with the kernel treating our root as `/`.
    ///
    /// `RESOLVE_IN_ROOT` reinterprets absolute symlinks and `..` against
    /// the handle rather than refusing them, which is what makes a layer
    /// containing `etc -> /` harmless. `RESOLVE_NO_MAGICLINKS` is passed
    /// explicitly: the man page says `RESOLVE_IN_ROOT` currently implies it
    /// and immediately adds "this may change in the future".
    /// `RESOLVE_NO_XDEV` stops mount-point traversal, which
    /// `RESOLVE_IN_ROOT` does not.
    fn openat(
        &self,
        rel: &Path,
        flags: OFlags,
        mode: Mode,
    ) -> rustix::io::Result<rustix::fd::OwnedFd> {
        let c = CString::new(rel.as_os_str().as_bytes()).map_err(|_| rustix::io::Errno::INVAL)?;
        rustix::fs::openat2(
            &self.fd,
            c.as_c_str(),
            flags | OFlags::CLOEXEC,
            mode,
            ResolveFlags::IN_ROOT | ResolveFlags::NO_MAGICLINKS | ResolveFlags::NO_XDEV,
        )
    }

    /// A handle on the parent directory of `rel`, creating it if needed.
    ///
    /// Walked one component at a time so each step's failure means
    /// something specific. In particular a component that exists as a
    /// symlink whose target does not resolve **inside** the layer is
    /// refused rather than created: that is the symlink-escape shape, and
    /// the whole point is that it does not quietly succeed somewhere else.
    /// A symlink pointing *within* the layer - `/lib -> usr/lib`, which
    /// every modern distribution image has - resolves normally.
    fn parent_dir(&self, rel: &Path) -> Result<rustix::fd::OwnedFd, Error> {
        let parent = rel.parent().unwrap_or(Path::new(""));
        let mut cur = self
            .openat(
                Path::new("."),
                OFlags::DIRECTORY | OFlags::RDONLY,
                Mode::empty(),
            )
            .map_err(|e| Error::Io {
                op: "opening the staging root",
                path: self.path.display().to_string(),
                source: e.into(),
            })?;

        let mut walked = PathBuf::new();
        for comp in parent.components() {
            let Component::Normal(c) = comp else {
                return Err(rejected(rel, "a path component that is not a plain name"));
            };
            walked.push(c);
            let name =
                CString::new(c.as_bytes()).map_err(|_| rejected(rel, "an interior NUL byte"))?;

            // Resolve relative to the staging ROOT each time, not to the
            // component we just opened: `RESOLVE_IN_ROOT` reinterprets
            // against whichever handle it is given, and we always mean the
            // layer's root.
            match self.openat(&walked, OFlags::DIRECTORY | OFlags::RDONLY, Mode::empty()) {
                Ok(fd) => {
                    cur = fd;
                    continue;
                }
                Err(rustix::io::Errno::NOENT) => {}
                Err(rustix::io::Errno::NOTDIR) => {
                    return Err(rejected(
                        rel,
                        "a parent path component that is not a directory",
                    ));
                }
                Err(e) => {
                    return Err(Error::Io {
                        op: "resolving a parent directory",
                        path: walked.display().to_string(),
                        source: e.into(),
                    });
                }
            }

            // Not there: create it, unless the name is already taken by
            // something that does not resolve inside the layer.
            match rustix::fs::mkdirat(&cur, name.as_c_str(), Mode::from_bits_truncate(0o755)) {
                Ok(()) => {}
                Err(rustix::io::Errno::EXIST) => {
                    return Err(rejected(
                        rel,
                        "a path that resolves through a symlink pointing outside the layer",
                    ));
                }
                Err(e) => {
                    return Err(Error::Io {
                        op: "creating a parent directory",
                        path: walked.display().to_string(),
                        source: e.into(),
                    });
                }
            }
            cur = self
                .openat(&walked, OFlags::DIRECTORY | OFlags::RDONLY, Mode::empty())
                .map_err(|e| Error::Io {
                    op: "opening a parent directory",
                    path: walked.display().to_string(),
                    source: e.into(),
                })?;
        }
        Ok(cur)
    }

    /// Remove whatever is at `rel`, following nothing.
    ///
    /// The spec's rule is remove-then-recreate, and the removal must never
    /// follow a symlink: that is exactly CVE-2021-32760's mechanism.
    fn remove(&self, rel: &Path) -> Result<(), Error> {
        let Some(name) = rel.file_name() else {
            return Ok(());
        };
        let dir = self.parent_dir(rel)?;
        let c = CString::new(name.as_bytes()).map_err(|_| rejected(rel, "an interior NUL byte"))?;
        match rustix::fs::unlinkat(&dir, c.as_c_str(), AtFlags::empty()) {
            Ok(()) | Err(rustix::io::Errno::NOENT) => Ok(()),
            Err(rustix::io::Errno::ISDIR) | Err(rustix::io::Errno::PERM) => {
                remove_dir_recursive(&dir, c.as_c_str())
            }
            Err(e) => Err(Error::Io {
                op: "removing an existing entry",
                path: rel.display().to_string(),
                source: e.into(),
            }),
        }
    }
}

fn remove_dir_recursive(parent: &rustix::fd::OwnedFd, name: &std::ffi::CStr) -> Result<(), Error> {
    // Rare: a whiteout over a directory a previous layer created. Walk it
    // through fds, never through a path that could be swapped.
    let dir = rustix::fs::openat(
        parent,
        name,
        OFlags::DIRECTORY | OFlags::RDONLY | OFlags::CLOEXEC | OFlags::NOFOLLOW,
        Mode::empty(),
    )
    .map_err(|e| Error::Io {
        op: "opening a directory to remove",
        path: name.to_string_lossy().into_owned(),
        source: e.into(),
    })?;
    let mut entries = Vec::new();
    {
        let mut rd = rustix::fs::Dir::read_from(&dir).map_err(|e| Error::Io {
            op: "reading a directory to remove",
            path: name.to_string_lossy().into_owned(),
            source: e.into(),
        })?;
        while let Some(Ok(e)) = rd.read() {
            let n = e.file_name().to_owned();
            if n.to_bytes() == b"." || n.to_bytes() == b".." {
                continue;
            }
            entries.push(n);
        }
    }
    for e in entries {
        if rustix::fs::unlinkat(&dir, e.as_c_str(), AtFlags::empty()).is_err() {
            remove_dir_recursive(&dir, e.as_c_str())?;
        }
    }
    drop(dir);
    rustix::fs::unlinkat(parent, name, AtFlags::REMOVEDIR).map_err(|e| Error::Io {
        op: "removing a directory",
        path: name.to_string_lossy().into_owned(),
        source: e.into(),
    })
}

fn cpath(p: &Path) -> Result<CString, Error> {
    CString::new(p.as_os_str().as_bytes()).map_err(|_| rejected(p, "an interior NUL byte"))
}

/// Lexical validation, run before any filesystem call.
///
/// This is **not** the containment guarantee — `SafeRoot` is. It exists so
/// a hostile path produces a good error message instead of a confusing one,
/// and because doing both costs nothing.
fn sanitise(raw: &Path) -> Result<PathBuf, Error> {
    let mut out = PathBuf::new();
    for comp in raw.components() {
        match comp {
            // A leading `/` is stripped, which is what every extractor
            // does; `..` is refused outright.
            Component::RootDir | Component::CurDir | Component::Prefix(_) => continue,
            Component::ParentDir => {
                return Err(rejected(raw, "a `..` component"));
            }
            Component::Normal(c) => {
                if c.as_bytes().contains(&0) {
                    return Err(rejected(raw, "an interior NUL byte"));
                }
                out.push(c);
            }
        }
    }
    if out.as_os_str().is_empty() {
        return Err(rejected(raw, "an empty path"));
    }
    Ok(out)
}

/// Extract one gzip-compressed layer into `root`, applying whiteouts.
pub fn apply_layer<R: Read>(
    root: &SafeRoot,
    reader: R,
    limits: Limits,
    report: &mut Report,
) -> Result<(), Error> {
    let mut archive = tar::Archive::new(reader);
    // Never `unpack`/`unpack_in`: the crate's own extraction has had this
    // exact class of bug (RUSTSEC-2026-0067). Header parsing only.
    archive.set_preserve_permissions(false);
    archive.set_unpack_xattrs(false);

    let entries = archive.entries().map_err(Error::Archive)?;
    for entry in entries {
        let mut entry = entry.map_err(Error::Archive)?;
        report.entries += 1;
        if report.entries > limits.max_entries {
            return Err(Error::TooManyEntries {
                limit: limits.max_entries,
            });
        }

        let raw = entry.path().map_err(Error::Archive)?.into_owned();
        let rel = sanitise(&raw)?;
        let header = entry.header().clone();
        let mode = header.mode().unwrap_or(0o644) & 0o7777;

        // --- whiteouts ---------------------------------------------------
        if let Some(name) = rel.file_name().and_then(OsStr::to_str) {
            if name == ".wh..wh..opq" {
                // Everything from lower layers under this directory is
                // hidden. With a flat extraction that means emptying it.
                report.opaque_dirs += 1;
                let dir = rel.parent().unwrap_or(Path::new("")).to_path_buf();
                empty_directory(root, &dir)?;
                continue;
            }
            if let Some(target) = name.strip_prefix(".wh.") {
                report.whiteouts += 1;
                let victim = rel.parent().unwrap_or(Path::new("")).join(target);
                root.remove(&victim)?;
                continue;
            }
        }

        match header.entry_type() {
            tar::EntryType::Directory => {
                let dir = root.parent_dir(&rel)?;
                let name = file_name_c(&rel)?;
                // The owner's search bit is forced on. A layer can declare
                // a directory 0644, and as root that is invisible - root
                // ignores permission bits - but as any other user nothing
                // can be created inside it and the rest of the layer fails
                // with EACCES. Recorded rather than silently applied,
                // because it is a deviation from what the layer asked for.
                let wanted = Mode::from_bits_truncate(mode);
                let effective = mode | 0o700;
                if effective != mode {
                    report.modes_relaxed += 1;
                }
                let _ = wanted;
                match rustix::fs::mkdirat(
                    &dir,
                    name.as_c_str(),
                    Mode::from_bits_truncate(effective),
                ) {
                    Ok(()) | Err(rustix::io::Errno::EXIST) => {}
                    Err(e) => {
                        return Err(Error::Io {
                            op: "creating a directory",
                            path: rel.display().to_string(),
                            source: e.into(),
                        });
                    }
                }
            }

            tar::EntryType::Regular | tar::EntryType::Continuous => {
                root.remove(&rel)?;
                let dir = root.parent_dir(&rel)?;
                let name = file_name_c(&rel)?;
                // Mode set at creation, never with a follow-up chmod: a
                // separate chmod is a second resolution and therefore a
                // second chance to be redirected.
                let fd = rustix::fs::openat(
                    &dir,
                    name.as_c_str(),
                    OFlags::CREATE
                        | OFlags::EXCL
                        | OFlags::WRONLY
                        | OFlags::CLOEXEC
                        | OFlags::NOFOLLOW,
                    Mode::from_bits_truncate(mode),
                )
                .map_err(|e| Error::Io {
                    op: "creating a file",
                    path: rel.display().to_string(),
                    source: e.into(),
                })?;
                let mut file = std::fs::File::from(fd);
                let written = std::io::copy(&mut entry, &mut file).map_err(Error::Archive)?;
                report.bytes += written;
                if report.bytes > limits.max_bytes {
                    return Err(Error::TooBig {
                        limit: limits.max_bytes,
                    });
                }
                // setuid and setgid bits are preserved deliberately: real
                // images ship `su` and `mount`. Containment is structural -
                // the container's own no_new_privs and dropped bounding set
                // are what make them harmless - not a matter of stripping
                // the bit here and breaking the image.
                let _ = file.set_permissions(std::fs::Permissions::from_mode(mode));
            }

            tar::EntryType::Symlink => {
                let target = entry
                    .link_name()
                    .map_err(Error::Archive)?
                    .ok_or_else(|| rejected(&rel, "a symlink with no target"))?
                    .into_owned();
                root.remove(&rel)?;
                let dir = root.parent_dir(&rel)?;
                let name = file_name_c(&rel)?;
                // The target is stored verbatim, including `../..` and
                // absolute paths. That is safe here and nowhere else:
                // anything that later resolves THROUGH this link goes
                // through RESOLVE_IN_ROOT, which reinterprets it against
                // the staging root. The symlink is not the danger; a later
                // entry resolving through it is.
                let t = CString::new(target.as_os_str().as_bytes())
                    .map_err(|_| rejected(&rel, "a symlink target with an interior NUL"))?;
                rustix::fs::symlinkat(t.as_c_str(), &dir, name.as_c_str()).map_err(|e| {
                    Error::Io {
                        op: "creating a symlink",
                        path: rel.display().to_string(),
                        source: e.into(),
                    }
                })?;
            }

            tar::EntryType::Link => {
                let target = entry
                    .link_name()
                    .map_err(Error::Archive)?
                    .ok_or_else(|| rejected(&rel, "a hardlink with no target"))?
                    .into_owned();
                // A hardlink has nothing to resolve out of, so symlink
                // defences do not apply: `linkat` on `/etc/shadow` creates
                // a second name for the HOST's inode inside the rootfs, and
                // the container can then write through it. An absolute
                // target is refused outright; a relative one is resolved
                // through the pinned root, which confines it to this
                // layer's staging directory.
                if target.is_absolute() {
                    return Err(rejected(&rel, "a hardlink to an absolute path"));
                }
                let tgt = sanitise(&target)?;
                let src = root
                    .openat(&tgt, OFlags::RDONLY | OFlags::NOFOLLOW, Mode::empty())
                    .map_err(|_| rejected(&rel, "a hardlink whose target is not in this layer"))?;
                root.remove(&rel)?;
                let dir = root.parent_dir(&rel)?;
                let name = file_name_c(&rel)?;
                // Link from the already-open descriptor, so nothing can be
                // swapped between resolving the target and linking it.
                let proc_path = CString::new(format!(
                    "/proc/self/fd/{}",
                    rustix::fd::AsRawFd::as_raw_fd(&src)
                ))
                .map_err(|_| rejected(&rel, "an interior NUL byte"))?;
                rustix::fs::linkat(
                    rustix::fs::CWD,
                    proc_path.as_c_str(),
                    &dir,
                    name.as_c_str(),
                    AtFlags::SYMLINK_FOLLOW,
                )
                .map_err(|e| Error::Io {
                    op: "creating a hardlink",
                    path: rel.display().to_string(),
                    source: e.into(),
                })?;
            }

            tar::EntryType::Char | tar::EntryType::Block => {
                // Refused, always. A layer containing `dev/mem` as char 1:1
                // or `dev/sda` as block 8:0, extracted as root, creates a
                // real device node in the container's rootfs - a direct
                // path to host memory or disk. This is a deliberate
                // deviation from the image spec, which lists devices among
                // the valid types, and it is also what a rootless extractor
                // is forced into anyway: mknod returns EPERM outside the
                // initial user namespace.
                //
                // Recorded rather than silently dropped, so a strict mode
                // can refuse the image outright.
                report.devices_skipped.push(rel.display().to_string());
            }

            tar::EntryType::Fifo => {
                root.remove(&rel)?;
                let dir = root.parent_dir(&rel)?;
                let name = file_name_c(&rel)?;
                let _ = rustix::fs::mknodat(
                    &dir,
                    name.as_c_str(),
                    rustix::fs::FileType::Fifo,
                    Mode::from_bits_truncate(mode),
                    0,
                );
            }

            // GNU long-name and PAX extension records are consumed by the
            // tar crate itself and never surface here.
            _ => continue,
        }

        // Extended attributes are dropped wholesale for now. The one that
        // matters is `security.capability`: a file carrying it gains
        // capabilities at execve, and written from inside a user namespace
        // the kernel silently REWRITES the value to a namespaced form -
        // so round-tripping an image through a rootless extraction changes
        // what the image means.
        if let Ok(Some(pax)) = entry.pax_extensions() {
            for ext in pax.flatten() {
                if let Ok(key) = ext.key()
                    && (key.starts_with("SCHILY.xattr.security.")
                        || key.starts_with("SCHILY.xattr.system."))
                {
                    report.xattrs_dropped += 1;
                }
            }
        }
    }
    Ok(())
}

fn file_name_c(rel: &Path) -> Result<CString, Error> {
    let name = rel
        .file_name()
        .ok_or_else(|| rejected(rel, "no final path component"))?;
    CString::new(name.as_bytes()).map_err(|_| rejected(rel, "an interior NUL byte"))
}

fn empty_directory(root: &SafeRoot, rel: &Path) -> Result<(), Error> {
    let dir = if rel.as_os_str().is_empty() {
        root.openat(
            Path::new("."),
            OFlags::DIRECTORY | OFlags::RDONLY,
            Mode::empty(),
        )
    } else {
        root.openat(rel, OFlags::DIRECTORY | OFlags::RDONLY, Mode::empty())
    };
    let Ok(dir) = dir else { return Ok(()) };
    let mut names = Vec::new();
    {
        let Ok(mut rd) = rustix::fs::Dir::read_from(&dir) else {
            return Ok(());
        };
        while let Some(Ok(e)) = rd.read() {
            let n = e.file_name().to_owned();
            if n.to_bytes() == b"." || n.to_bytes() == b".." {
                continue;
            }
            names.push(n);
        }
    }
    for n in names {
        if rustix::fs::unlinkat(&dir, n.as_c_str(), AtFlags::empty()).is_err() {
            let _ = remove_dir_recursive(&dir, n.as_c_str());
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn tar_with(entries: Vec<(tar::Header, Vec<u8>)>) -> Vec<u8> {
        let mut b = tar::Builder::new(Vec::new());
        for (mut h, data) in entries {
            h.set_cksum();
            b.append(&h, &data[..]).unwrap();
        }
        b.into_inner().unwrap()
    }

    /// Build a header with the name written straight into the GNU header
    /// bytes. `set_path` refuses `..` and absolute paths - which is a
    /// sensible thing for a tar *writer* to do and makes it useless for
    /// building the hostile archives this module exists to survive.
    fn header(path: &str, kind: tar::EntryType, size: u64) -> tar::Header {
        let mut h = tar::Header::new_gnu();
        let gnu = h.as_gnu_mut().unwrap();
        let bytes = path.as_bytes();
        gnu.name[..bytes.len()].copy_from_slice(bytes);
        h.set_entry_type(kind);
        h.set_size(size);
        h.set_mode(if kind == tar::EntryType::Directory {
            0o755
        } else {
            0o644
        });
        h
    }

    fn with_link(mut h: tar::Header, target: &str) -> tar::Header {
        let gnu = h.as_gnu_mut().unwrap();
        let b = target.as_bytes();
        gnu.linkname[..b.len()].copy_from_slice(b);
        h
    }

    fn staging(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("rustainer-extract-{tag}"));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn run(dir: &Path, data: Vec<u8>) -> Result<Report, Error> {
        let root = SafeRoot::open(dir)?;
        let mut r = Report::default();
        apply_layer(&root, &data[..], Limits::default(), &mut r)?;
        Ok(r)
    }

    #[test]
    fn a_dot_dot_path_is_refused() {
        let d = staging("dotdot");
        let t = tar_with(vec![(
            header("../escape", tar::EntryType::Regular, 2),
            b"hi".to_vec(),
        )]);
        assert!(matches!(run(&d, t), Err(Error::Rejected { .. })));
        assert!(!d.parent().unwrap().join("escape").exists());
        std::fs::remove_dir_all(&d).ok();
    }

    #[test]
    fn an_absolute_path_is_stripped_not_honoured() {
        let d = staging("abs");
        let t = tar_with(vec![(
            header("/etc/passwd", tar::EntryType::Regular, 2),
            b"xx".to_vec(),
        )]);
        run(&d, t).expect("extracts");
        assert!(
            d.join("etc/passwd").exists(),
            "landed inside the staging dir"
        );
        std::fs::remove_dir_all(&d).ok();
    }

    /// The hazard that lexical checking cannot close: the symlink is
    /// harmless, the entry resolving THROUGH it is not.
    #[test]
    fn a_symlink_escape_is_refused_rather_than_followed() {
        let d = staging("symlink");
        let outside = d.parent().unwrap().join("rustainer-extract-victim");
        std::fs::create_dir_all(&outside).unwrap();
        std::fs::write(outside.join("passwd"), b"original").unwrap();

        let link = with_link(
            header("escape", tar::EntryType::Symlink, 0),
            outside.to_str().unwrap(),
        );
        let t = tar_with(vec![
            (link, Vec::new()),
            (
                header("escape/passwd", tar::EntryType::Regular, 5),
                b"OWNED".to_vec(),
            ),
        ]);
        let err = run(&d, t).expect_err("must refuse rather than escape");
        assert!(
            matches!(&err, Error::Rejected { reason, .. } if reason.contains("outside the layer")),
            "{err}"
        );
        assert_eq!(
            std::fs::read_to_string(outside.join("passwd")).unwrap(),
            "original",
            "the write escaped the staging directory"
        );
        std::fs::remove_dir_all(&d).ok();
        std::fs::remove_dir_all(&outside).ok();
    }

    /// The case that must NOT be refused: every modern distribution image
    /// ships `/lib -> usr/lib`, and entries written through it are normal.
    #[test]
    fn a_symlink_pointing_inside_the_layer_is_followed_normally() {
        let d = staging("symlink-ok");
        let t = tar_with(vec![
            (header("usr/", tar::EntryType::Directory, 0), Vec::new()),
            (header("usr/lib/", tar::EntryType::Directory, 0), Vec::new()),
            (
                with_link(header("lib", tar::EntryType::Symlink, 0), "usr/lib"),
                Vec::new(),
            ),
            (
                header("lib/libc.so", tar::EntryType::Regular, 2),
                b"so".to_vec(),
            ),
        ]);
        run(&d, t).expect("a normal distribution layout must extract");
        assert!(
            d.join("usr/lib/libc.so").exists(),
            "written through the link"
        );
        std::fs::remove_dir_all(&d).ok();
    }

    /// A directory a layer declares unenterable must not stop the rest of
    /// the layer. Invisible when extracting as root, fatal otherwise.
    #[test]
    fn a_directory_with_no_search_bit_is_widened_and_reported() {
        let d = staging("narrow-dir");
        let mut dh = header("locked/", tar::EntryType::Directory, 0);
        dh.set_mode(0o644);
        let t = tar_with(vec![
            (dh, Vec::new()),
            (
                header("locked/inside", tar::EntryType::Regular, 2),
                b"ok".to_vec(),
            ),
        ]);
        let r = run(&d, t).expect("a 0644 directory must not stop extraction");
        assert_eq!(r.modes_relaxed, 1, "the widening must be reported");
        assert!(d.join("locked/inside").exists());
        std::fs::remove_dir_all(&d).ok();
    }

    #[test]
    fn an_absolute_hardlink_target_is_refused() {
        let d = staging("hardlink");
        let h = with_link(header("grab", tar::EntryType::Link, 0), "/etc/shadow");
        let t = tar_with(vec![(h, Vec::new())]);
        assert!(matches!(run(&d, t), Err(Error::Rejected { .. })));
        std::fs::remove_dir_all(&d).ok();
    }

    #[test]
    fn a_hardlink_outside_this_layer_is_refused() {
        let d = staging("hardlink2");
        let h = with_link(header("grab", tar::EntryType::Link, 0), "../../etc/shadow");
        let t = tar_with(vec![(h, Vec::new())]);
        assert!(matches!(run(&d, t), Err(Error::Rejected { .. })));
        std::fs::remove_dir_all(&d).ok();
    }

    #[test]
    fn device_nodes_are_never_created() {
        let d = staging("dev");
        let mut h = header("dev/mem", tar::EntryType::Char, 0);
        h.set_device_major(1).unwrap();
        h.set_device_minor(1).unwrap();
        h.set_cksum();
        let t = tar_with(vec![(h, Vec::new())]);
        let r = run(&d, t).expect("extracts");
        assert_eq!(r.devices_skipped, vec!["dev/mem".to_string()]);
        assert!(!d.join("dev/mem").exists());
        std::fs::remove_dir_all(&d).ok();
    }

    #[test]
    fn setuid_bits_survive_because_containment_is_structural() {
        let d = staging("setuid");
        let mut h = header("bin/su", tar::EntryType::Regular, 2);
        h.set_mode(0o4755);
        h.set_cksum();
        let t = tar_with(vec![(h, b"xx".to_vec())]);
        run(&d, t).expect("extracts");
        let m = std::fs::metadata(d.join("bin/su"))
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(m & 0o7777, 0o4755, "a real image ships setuid binaries");
        std::fs::remove_dir_all(&d).ok();
    }

    #[test]
    fn a_whiteout_removes_the_lower_entry() {
        let d = staging("whiteout");
        std::fs::create_dir_all(d.join("etc")).unwrap();
        std::fs::write(d.join("etc/keep"), b"k").unwrap();
        std::fs::write(d.join("etc/gone"), b"g").unwrap();
        let t = tar_with(vec![(
            header("etc/.wh.gone", tar::EntryType::Regular, 0),
            Vec::new(),
        )]);
        let r = run(&d, t).expect("extracts");
        assert_eq!(r.whiteouts, 1);
        assert!(d.join("etc/keep").exists());
        assert!(!d.join("etc/gone").exists());
        std::fs::remove_dir_all(&d).ok();
    }

    #[test]
    fn an_opaque_whiteout_empties_the_directory() {
        let d = staging("opaque");
        std::fs::create_dir_all(d.join("var/lib")).unwrap();
        std::fs::write(d.join("var/lib/old"), b"o").unwrap();
        let t = tar_with(vec![(
            header("var/.wh..wh..opq", tar::EntryType::Regular, 0),
            Vec::new(),
        )]);
        let r = run(&d, t).expect("extracts");
        assert_eq!(r.opaque_dirs, 1);
        assert!(!d.join("var/lib").exists());
        assert!(d.join("var").exists());
        std::fs::remove_dir_all(&d).ok();
    }

    #[test]
    fn a_decompression_bomb_hits_the_byte_limit() {
        let d = staging("bomb");
        let t = tar_with(vec![(
            header("big", tar::EntryType::Regular, 4096),
            vec![0u8; 4096],
        )]);
        let root = SafeRoot::open(&d).unwrap();
        let mut r = Report::default();
        let err = apply_layer(
            &root,
            &t[..],
            Limits {
                max_bytes: 100,
                max_entries: 10,
            },
            &mut r,
        )
        .unwrap_err();
        assert!(matches!(err, Error::TooBig { .. }));
        std::fs::remove_dir_all(&d).ok();
    }

    #[test]
    fn an_entry_flood_hits_the_entry_limit() {
        let d = staging("flood");
        let mut v = Vec::new();
        for i in 0..20 {
            v.push((
                header(&format!("f{i}"), tar::EntryType::Regular, 0),
                Vec::new(),
            ));
        }
        let t = tar_with(v);
        let root = SafeRoot::open(&d).unwrap();
        let mut r = Report::default();
        let err = apply_layer(
            &root,
            &t[..],
            Limits {
                max_bytes: 1 << 30,
                max_entries: 5,
            },
            &mut r,
        )
        .unwrap_err();
        assert!(matches!(err, Error::TooManyEntries { .. }));
        std::fs::remove_dir_all(&d).ok();
    }

    #[test]
    fn replacing_an_entry_never_follows_a_symlink() {
        // CVE-2021-32760's mechanism: the extractor writes through an
        // existing symlink instead of replacing it.
        let d = staging("replace");
        let outside = d.parent().unwrap().join("rustainer-extract-replace-victim");
        std::fs::write(&outside, b"original").unwrap();
        std::os::unix::fs::symlink(&outside, d.join("target")).unwrap();

        let t = tar_with(vec![(
            header("target", tar::EntryType::Regular, 5),
            b"OWNED".to_vec(),
        )]);
        run(&d, t).expect("extracts");
        assert_eq!(std::fs::read_to_string(&outside).unwrap(), "original");
        assert_eq!(std::fs::read_to_string(d.join("target")).unwrap(), "OWNED");
        std::fs::remove_file(&outside).ok();
        std::fs::remove_dir_all(&d).ok();
    }

    #[test]
    fn sanitise_accepts_ordinary_paths() {
        assert_eq!(
            sanitise(Path::new("a/b/c")).unwrap(),
            PathBuf::from("a/b/c")
        );
        assert_eq!(sanitise(Path::new("./a")).unwrap(), PathBuf::from("a"));
        assert!(sanitise(Path::new("")).is_err());
    }

    #[test]
    fn a_plain_file_round_trips() {
        let d = staging("plain");
        let t = tar_with(vec![(
            header("hello.txt", tar::EntryType::Regular, 5),
            b"world".to_vec(),
        )]);
        let r = run(&d, t).expect("extracts");
        assert_eq!(r.bytes, 5);
        assert_eq!(
            std::fs::read_to_string(d.join("hello.txt")).unwrap(),
            "world"
        );
        let mut f = std::fs::File::create(d.join("x")).unwrap();
        f.write_all(b"ok").unwrap();
        std::fs::remove_dir_all(&d).ok();
    }
}
