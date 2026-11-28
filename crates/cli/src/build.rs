//! Building an image from a Dockerfile, using this engine to do it.
//!
//! # The shape, which is the whole trick
//!
//! A build step that changes the filesystem is **a container run on the
//! layers so far**, and the container's own overlay upper directory is
//! the new layer. Nothing has to diff anything: overlayfs already wrote
//! down exactly what changed, including the deletions, which it records
//! as the same `0:0` character devices this engine's layer store already
//! uses (stage 15).
//!
//! So `RUN` is `rustainer run --layer … --layer …` with the step's
//! command, and afterwards the upper directory *is* the layer. The build
//! inherits the engine's isolation, its cgroups, its networking and its
//! security posture for free, because it is not a second implementation
//! of any of them.
//!
//! # What is deliberately not here
//!
//! No parallelism: stages are built in the order the file lists them,
//! every one up to the target, whether or not the target uses it.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};

use crate::dockerfile::Instruction;

/// What the build accumulates: layers on disk, and the config that will
/// describe them.
#[derive(Clone, Default)]
pub struct Built {
    pub layers: Vec<PathBuf>,
    pub diff_ids: Vec<String>,
    pub env: Vec<String>,
    pub workdir: Option<String>,
    pub user: Option<String>,
    pub cmd: Vec<String>,
    pub entrypoint: Vec<String>,
    pub labels: BTreeMap<String, String>,
    pub exposed: Vec<String>,
    /// `(argv, interval, retries)`; an empty argv is `HEALTHCHECK NONE`.
    pub healthcheck: Option<(Vec<String>, u64, u64)>,
    pub stop_signal: Option<String>,
    pub volumes: Vec<String>,
}

/// Substitute `$VAR` and `${VAR}` from the build arguments.
///
/// Only from ARG and ENV, never from the caller's environment: a build
/// that quietly picked up `$HOME` from whoever ran it would produce a
/// different image on every machine, which is the one thing a Dockerfile
/// exists to prevent.
pub fn expand(text: &str, vars: &BTreeMap<String, String>) -> String {
    let b = text.as_bytes();
    let mut out = String::with_capacity(text.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] != b'$' {
            out.push(b[i] as char);
            i += 1;
            continue;
        }
        let (name, next) = if b.get(i + 1) == Some(&b'{') {
            match b[i + 2..].iter().position(|&c| c == b'}') {
                Some(end) => (&text[i + 2..i + 2 + end], i + 3 + end),
                None => {
                    out.push('$');
                    i += 1;
                    continue;
                }
            }
        } else {
            let start = i + 1;
            let end = b[start..]
                .iter()
                .position(|c| !(c.is_ascii_alphanumeric() || *c == b'_'))
                .map_or(b.len(), |p| start + p);
            if end == start {
                out.push('$');
                i += 1;
                continue;
            }
            (&text[start..end], end)
        };
        // An unset variable expands to nothing, as a shell would, rather
        // than being left as `$NAME`: leaving it produces a path with a
        // dollar in it and a failure far from the cause.
        out.push_str(vars.get(name).map(String::as_str).unwrap_or(""));
        i = next;
    }
    out
}

/// Turn a finished overlay upper directory into a layer in the store.
///
/// Returns the `diff_id`, which is the sha256 of the layer's uncompressed
/// tar - the identifier the image config uses and the one a registry will
/// want if this image is ever pushed.
///
/// The tar is written in OCI's whiteout spelling, not overlayfs's: a
/// `0:0` character device becomes a `.wh.<name>` entry, and an opaque
/// directory becomes `.wh..wh..opq`. The two formats mean the same thing
/// and no registry has heard of the second one.
pub fn seal_layer(upper: &Path) -> Result<(String, Vec<u8>)> {
    let mut tar = tar::Builder::new(Vec::new());
    tar.mode(tar::HeaderMode::Deterministic);
    append_tree(&mut tar, upper, Path::new(""))?;
    let bytes = tar.into_inner().context("building the layer archive")?;
    let digest = rustainer_image::registry::sha256_hex(&bytes);
    Ok((digest, bytes))
}

fn append_tree<W: std::io::Write>(
    tar: &mut tar::Builder<W>,
    root: &Path,
    rel: &Path,
) -> Result<()> {
    let dir = root.join(rel);
    let mut entries: Vec<_> = std::fs::read_dir(&dir)
        .with_context(|| format!("reading {}", dir.display()))?
        .filter_map(Result::ok)
        .collect();
    // Sorted, so the same filesystem always produces the same bytes and
    // therefore the same diff_id. Readdir order is not stable.
    entries.sort_by_key(std::fs::DirEntry::file_name);

    for e in entries {
        let name = e.file_name();
        let child = rel.join(&name);
        let path = e.path();
        let md = std::fs::symlink_metadata(&path)?;
        let ft = md.file_type();

        if is_whiteout_device(&md) {
            // Overlay's deletion marker, written in OCI's spelling.
            let mut h = tar::Header::new_gnu();
            h.set_size(0);
            h.set_mode(0o644);
            h.set_entry_type(tar::EntryType::Regular);
            let wh = rel.join(format!(".wh.{}", name.to_string_lossy()));
            tar.append_data(&mut h, wh, std::io::empty())?;
            continue;
        }

        if ft.is_dir() {
            let mut h = tar::Header::new_gnu();
            h.set_metadata(&md);
            h.set_entry_type(tar::EntryType::Directory);
            h.set_size(0);
            tar.append_data(&mut h, &child, std::io::empty())?;
            if is_opaque(&path) {
                let mut oh = tar::Header::new_gnu();
                oh.set_size(0);
                oh.set_mode(0o644);
                oh.set_entry_type(tar::EntryType::Regular);
                tar.append_data(&mut oh, child.join(".wh..wh..opq"), std::io::empty())?;
            }
            append_tree(tar, root, &child)?;
        } else if ft.is_symlink() {
            let target = std::fs::read_link(&path)?;
            let mut h = tar::Header::new_gnu();
            h.set_metadata(&md);
            h.set_entry_type(tar::EntryType::Symlink);
            h.set_size(0);
            tar.append_link(&mut h, &child, &target)?;
        } else if ft.is_file() {
            let mut f = std::fs::File::open(&path)?;
            let mut h = tar::Header::new_gnu();
            h.set_metadata(&md);
            tar.append_data(&mut h, &child, &mut f)?;
        }
        // Anything else - sockets, fifos, real devices - is dropped. A
        // layer is not a place for them, and the extractor on the other
        // side refuses device nodes anyway.
    }
    Ok(())
}

fn is_whiteout_device(md: &std::fs::Metadata) -> bool {
    use std::os::unix::fs::{FileTypeExt, MetadataExt};
    md.file_type().is_char_device() && md.rdev() == 0
}

fn is_opaque(p: &Path) -> bool {
    let mut buf = [0u8; 8];
    let c = match std::ffi::CString::new(p.as_os_str().as_encoded_bytes()) {
        Ok(c) => c,
        Err(_) => return false,
    };
    // Either spelling: which one was written depends on whether the
    // overlay was mounted `userxattr`, which depends on whether we are in
    // the initial user namespace.
    for name in [c"trusted.overlay.opaque", c"user.overlay.opaque"] {
        if rustix::fs::getxattr(c.as_c_str(), name, &mut buf).is_ok_and(|n| n > 0) {
            return true;
        }
    }
    false
}

/// Everything a COPY has to decide, with the host side resolved safely.
///
/// The context directory is the boundary: a `COPY ../../etc/shadow .`
/// must not reach outside it, and the check is on the CANONICAL path
/// because `a/../../etc` is only visibly an escape after resolution.
pub fn resolve_source(context: &Path, source: &str) -> Result<PathBuf> {
    let joined = context.join(source.trim_start_matches('/'));
    let canon = joined
        .canonicalize()
        .with_context(|| format!("{source:?} is not in the build context"))?;
    let root = context
        .canonicalize()
        .with_context(|| format!("resolving the build context {}", context.display()))?;
    if !canon.starts_with(&root) {
        bail!(
            "{source:?} resolves to {}, which is outside the build context. A \
             Dockerfile may only copy what is in the directory it was given.",
            canon.display()
        );
    }
    Ok(canon)
}

/// `*` and `?` inside a name, `**` across names. What `COPY` sources
/// and `.dockerignore` lines are written in.
pub fn glob(pattern: &str, path: &str) -> bool {
    fn name(p: &[u8], s: &[u8]) -> bool {
        match (p.first(), s.first()) {
            (None, None) => true,
            (Some(b'*'), _) => name(&p[1..], s) || (!s.is_empty() && name(p, &s[1..])),
            (Some(b'?'), Some(_)) => name(&p[1..], &s[1..]),
            (Some(a), Some(b)) if a == b => name(&p[1..], &s[1..]),
            _ => false,
        }
    }
    fn parts(p: &[&str], s: &[&str]) -> bool {
        match (p.first(), s.first()) {
            (None, None) => true,
            (Some(&"**"), _) => parts(&p[1..], s) || (!s.is_empty() && parts(p, &s[1..])),
            (Some(a), Some(b)) => name(a.as_bytes(), b.as_bytes()) && parts(&p[1..], &s[1..]),
            _ => false,
        }
    }
    let split = |x: &'_ str| -> Vec<String> {
        x.split('/')
            .filter(|c| !c.is_empty() && *c != ".")
            .map(str::to_owned)
            .collect()
    };
    let (p, s) = (split(pattern), split(path));
    parts(
        &p.iter().map(String::as_str).collect::<Vec<_>>(),
        &s.iter().map(String::as_str).collect::<Vec<_>>(),
    )
}

/// The context's `.dockerignore`: what a `COPY` leaves behind.
///
/// A line names a path or anything beneath it, the last line that
/// matches decides, and `!` turns a line into "keep this after all".
pub struct Ignore {
    root: PathBuf,
    rules: Vec<(bool, String)>,
}

impl Ignore {
    pub fn load(context: &Path) -> Self {
        let text = std::fs::read_to_string(context.join(".dockerignore")).unwrap_or_default();
        let rules = text
            .lines()
            .map(str::trim)
            .filter(|l| !l.is_empty() && !l.starts_with('#'))
            .map(|l| match l.strip_prefix('!') {
                Some(keep) => (true, keep.trim().to_owned()),
                None => (false, l.to_owned()),
            })
            .collect();
        Self {
            root: context.to_path_buf(),
            rules,
        }
    }

    fn keeps_something(&self) -> bool {
        self.rules.iter().any(|(keep, _)| *keep)
    }

    pub fn skips(&self, path: &Path) -> bool {
        let Ok(rel) = path.strip_prefix(&self.root) else {
            return false;
        };
        let rel = rel.to_string_lossy();
        let mut skipped = false;
        for (keep, pattern) in &self.rules {
            // The path itself or any directory above it.
            let hit = rel
                .match_indices('/')
                .map(|(i, _)| &rel[..i])
                .chain([&rel[..]])
                .any(|prefix| glob(pattern, prefix));
            if hit {
                skipped = !keep;
            }
        }
        skipped
    }
}

/// The context paths a `COPY` source names: one, or every match of a
/// wildcard such as `package*.json`.
pub fn sources(context: &Path, source: &str) -> Result<Vec<PathBuf>> {
    if !source.contains(['*', '?']) {
        return Ok(vec![resolve_source(context, source)?]);
    }
    let mut found = vec![context.to_path_buf()];
    for part in source.split('/').filter(|c| !c.is_empty() && *c != ".") {
        let mut next = Vec::new();
        for dir in &found {
            if !part.contains(['*', '?']) {
                next.push(dir.join(part));
                continue;
            }
            for e in std::fs::read_dir(dir).into_iter().flatten().flatten() {
                if glob(part, &e.file_name().to_string_lossy()) {
                    next.push(e.path());
                }
            }
        }
        found = next;
    }
    found.retain(|p| p.symlink_metadata().is_ok());
    found.sort();
    if found.is_empty() {
        bail!("{source:?} matches nothing in the build context");
    }
    found
        .iter()
        .map(|p| resolve_source(context, &p.strip_prefix(context).unwrap().to_string_lossy()))
        .collect()
}

/// What a stack of layers (bottom first) shows at `rel`: the
/// directories that merge there, top first, or the one file that wins.
/// Empty when nothing is there, which includes "deleted by a layer
/// above".
fn visible(layers: &[PathBuf], rel: &Path) -> Vec<PathBuf> {
    let parts: Vec<_> = rel.components().collect();
    let mut out: Vec<PathBuf> = Vec::new();
    'layers: for layer in layers.iter().rev() {
        let mut p = layer.clone();
        let mut opaque = false;
        let mut missing = false;
        let mut is_dir = true;
        for (i, c) in parts.iter().enumerate() {
            p.push(c);
            let Ok(md) = std::fs::symlink_metadata(&p) else {
                missing = true;
                break;
            };
            if is_whiteout_device(&md) {
                break 'layers;
            }
            is_dir = md.is_dir();
            if !is_dir && i + 1 != parts.len() {
                // A file where the path needs a directory hides
                // whatever the layers below had under that name.
                break 'layers;
            }
            opaque |= is_dir && is_opaque(&p);
        }
        if !missing {
            if !is_dir && !out.is_empty() {
                break;
            }
            out.push(p);
            if !is_dir {
                break;
            }
        }
        if opaque {
            break;
        }
    }
    out
}

/// `rel` with every symlink among its directories followed **inside
/// the layers**: `/lib/x` where `/lib -> usr/lib`, as most distributions
/// have it. Done by hand because letting the kernel follow one would
/// resolve an absolute link against the host's root, and copy the
/// host's file into the image.
fn follow(layers: &[PathBuf], rel: &Path) -> PathBuf {
    let names = |p: &Path| -> Vec<std::ffi::OsString> {
        p.components()
            .rev()
            .map(|c| c.as_os_str().to_owned())
            .collect()
    };
    let mut todo = names(rel);
    let mut done = PathBuf::new();
    let mut hops = 0;
    while let Some(c) = todo.pop() {
        if c == "/" || c == "." {
            continue;
        }
        if c == ".." {
            done.pop();
            continue;
        }
        done.push(&c);
        // The last name is the thing being copied, link or not.
        if todo.is_empty() || hops == 40 {
            continue;
        }
        if let Some(top) = visible(layers, &done).first()
            && let Ok(target) = std::fs::read_link(top)
        {
            hops += 1;
            if target.is_absolute() {
                done = PathBuf::new();
            } else {
                done.pop();
            }
            todo.extend(names(&target));
        }
    }
    done
}

/// The paths in a stack of layers that a `COPY --from` source names.
// ponytail: a wildcard is honoured in the last name only, which is where
// people put one (`/lib/ld-musl-*`); walk every level if that changes.
pub fn sources_in(layers: &[PathBuf], source: &str) -> Result<Vec<PathBuf>> {
    let rel = follow(layers, Path::new(source));
    let pattern = rel.file_name().map(|n| n.to_string_lossy().into_owned());
    let Some(pattern) = pattern.filter(|p| p.contains(['*', '?'])) else {
        return Ok(vec![rel]);
    };
    let parent = rel.parent().unwrap_or(Path::new(""));
    let mut names = std::collections::BTreeSet::new();
    for dir in visible(layers, parent) {
        for e in std::fs::read_dir(dir).into_iter().flatten().flatten() {
            names.insert(e.file_name());
        }
    }
    let found: Vec<PathBuf> = names
        .iter()
        .filter(|n| glob(&pattern, &n.to_string_lossy()))
        .map(|n| parent.join(n))
        .filter(|p| exists_in(layers, p))
        .collect();
    if found.is_empty() {
        bail!("{source:?} matches nothing in what it is copied from");
    }
    Ok(found)
}

/// Whether a container over these layers would find anything at `rel`.
pub fn exists_in(layers: &[PathBuf], rel: &Path) -> bool {
    !visible(layers, rel).is_empty()
}

/// Whether what it would find there is a directory.
pub fn is_dir_in(layers: &[PathBuf], rel: &Path) -> bool {
    visible(layers, rel)
        .first()
        .is_some_and(|p| std::fs::symlink_metadata(p).is_ok_and(|m| m.is_dir()))
}

/// Copy `rel` out of a stack of layers as a container over them would
/// see it: `COPY --from`.
///
/// Read in user space rather than by mounting the overlay, because the
/// build's own process may be an unprivileged user who cannot mount one.
// ponytail: one walk of the layer stack per entry, and a symlink in the
// middle of the source path is not followed. Mount the overlay in a
// helper container if a `COPY --from` of a huge tree is ever slow.
pub fn copy_merged(layers: &[PathBuf], rel: &Path, dest: &Path) -> Result<()> {
    use std::os::unix::fs::{MetadataExt, PermissionsExt};
    let found = visible(layers, rel);
    let Some(top) = found.first() else {
        bail!("/{} is not in what it is copied from", rel.display());
    };
    let md = std::fs::symlink_metadata(top)?;
    if md.is_dir() {
        std::fs::create_dir_all(dest)?;
        let _ = std::fs::set_permissions(dest, std::fs::Permissions::from_mode(md.mode()));
        let mut names = std::collections::BTreeSet::new();
        for dir in &found {
            for e in std::fs::read_dir(dir)? {
                names.insert(e?.file_name());
            }
        }
        for n in names {
            let child = rel.join(&n);
            if !visible(layers, &child).is_empty() {
                copy_merged(layers, &child, &dest.join(&n))?;
            }
        }
    } else {
        copy_into(top, dest, None)?;
    }
    // The owner comes along, as Docker's does; without the privilege to
    // set it, it is whoever is building.
    let _ = std::os::unix::fs::lchown(dest, Some(md.uid()), Some(md.gid()));
    Ok(())
}

/// `--chown=<user>[:<group>]` as numbers, the names looked up in the
/// image being built, which is the only place they mean anything.
pub fn owner(layers: &[PathBuf], spec: &str) -> Result<(u32, u32)> {
    let id = |name: &str, file: &str| -> Result<u32> {
        if let Ok(n) = name.parse() {
            return Ok(n);
        }
        let text = visible(layers, Path::new(file))
            .first()
            // Not through a link: that would read the host's file.
            .filter(|p| std::fs::symlink_metadata(p).is_ok_and(|m| m.is_file()))
            .and_then(|p| std::fs::read_to_string(p).ok())
            .unwrap_or_default();
        text.lines()
            .map(|l| l.split(':').collect::<Vec<_>>())
            .find(|f| f.first() == Some(&name))
            .and_then(|f| f.get(2)?.parse().ok())
            .with_context(|| format!("--chown: the image's /{file} has no {name:?}"))
    };
    let (user, group) = spec.split_once(':').unwrap_or((spec, ""));
    let uid = id(user, "etc/passwd")?;
    // No group means the same number as the user, as Docker has it.
    let gid = if group.is_empty() {
        uid
    } else {
        id(group, "etc/group")?
    };
    Ok((uid, gid))
}

/// Apply `--chown` and `--chmod` to everything a COPY just wrote.
pub fn retouch(path: &Path, owner: Option<(u32, u32)>, mode: Option<u32>) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    let md = std::fs::symlink_metadata(path)?;
    if let Some((uid, gid)) = owner {
        std::os::unix::fs::lchown(path, Some(uid), Some(gid))
            .with_context(|| format!("--chown on {} (it needs root)", path.display()))?;
    }
    if let Some(mode) = mode
        && !md.file_type().is_symlink()
    {
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode))?;
    }
    if md.is_dir() {
        for e in std::fs::read_dir(path)? {
            retouch(&e?.path(), owner, mode)?;
        }
    }
    Ok(())
}

/// Whether `ADD` unpacks this rather than copying it.
// ponytail: decided by the name and limited to tar and tar.gz, where
// Docker sniffs the content and also takes bzip2 and xz.
pub fn is_archive(path: &Path) -> bool {
    let name = path.to_string_lossy();
    path.is_file()
        && [".tar", ".tar.gz", ".tgz"]
            .iter()
            .any(|e| name.ends_with(e))
}

/// Unpack a tar archive into `dest`: what `ADD` does with one.
pub fn unpack(archive: &Path, dest: &Path) -> Result<()> {
    std::fs::create_dir_all(dest)?;
    let file = std::fs::File::open(archive)?;
    let reader: Box<dyn std::io::Read> = if archive.extension().is_some_and(|e| e == "tar") {
        Box::new(file)
    } else {
        Box::new(flate2::read::GzDecoder::new(file))
    };
    let mut tar = tar::Archive::new(reader);
    tar.set_preserve_permissions(true);
    // `unpack` refuses an entry that would land outside `dest`.
    tar.unpack(dest)?;
    Ok(())
}

/// Copy a file or directory tree into `dest`, preserving modes and
/// leaving out what `ignore` names.
pub fn copy_into(src: &Path, dest: &Path, ignore: Option<&Ignore>) -> Result<()> {
    let md = std::fs::symlink_metadata(src)?;
    let skipped = ignore.is_some_and(|i| i.skips(src));
    // A skipped directory is still walked when some line says `!keep`:
    // the thing to keep may be inside it.
    if skipped && !(md.is_dir() && ignore.is_some_and(Ignore::keeps_something)) {
        return Ok(());
    }
    if md.is_dir() {
        if !skipped {
            std::fs::create_dir_all(dest)?;
        }
        for e in std::fs::read_dir(src)? {
            let e = e?;
            copy_into(&e.path(), &dest.join(e.file_name()), ignore)?;
        }
        return Ok(());
    }
    if md.file_type().is_symlink() {
        let target = std::fs::read_link(src)?;
        let _ = std::fs::remove_file(dest);
        std::os::unix::fs::symlink(target, dest)?;
        return Ok(());
    }
    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::copy(src, dest)?;
    // The source's modification time, not the moment of the copy. A
    // layer's identity is a hash of its tar, the tar records mtimes, and
    // "when the build happened to run" is not part of what was copied:
    // with it in, the same context built twice a second apart made two
    // different layers, and every cached step after a COPY was a miss.
    if let Ok(t) = md.modified() {
        let _ = std::fs::File::options()
            .write(true)
            .open(dest)
            .and_then(|f| f.set_modified(t));
    }
    Ok(())
}

/// Give every directory in a COPY layer one fixed modification time.
///
/// The directories are made by the copy - the destination's parents, and
/// a copied tree's own - so their times are the time of the build, for
/// the reason above. The epoch rather than the source directory's time,
/// because half of them have no source directory.
pub fn settle_directory_times(dir: &Path) {
    for e in std::fs::read_dir(dir).into_iter().flatten().flatten() {
        if e.file_type().is_ok_and(|t| t.is_dir()) {
            settle_directory_times(&e.path());
            let _ =
                std::fs::File::open(e.path()).and_then(|f| f.set_modified(std::time::UNIX_EPOCH));
        }
    }
}

/// The instructions that produce a layer, in order, for reporting.
pub fn describe(i: &Instruction) -> String {
    match i {
        Instruction::From { image, .. } => format!("FROM {image}"),
        Instruction::Run { argv, .. } => format!("RUN {}", argv.join(" ")),
        Instruction::Copy {
            sources,
            dest,
            from,
            ..
        } => {
            let from = from
                .as_ref()
                .map(|f| format!("--from={f} "))
                .unwrap_or_default();
            format!("COPY {from}{} {dest}", sources.join(" "))
        }
        Instruction::Env(k, v) => format!("ENV {k}={v}"),
        Instruction::Workdir(w) => format!("WORKDIR {w}"),
        Instruction::Cmd(c) => format!("CMD {}", c.join(" ")),
        Instruction::Entrypoint(c) => format!("ENTRYPOINT {}", c.join(" ")),
        Instruction::User(u) => format!("USER {u}"),
        Instruction::Expose(p) => format!("EXPOSE {p}"),
        Instruction::Label(k, v) => format!("LABEL {k}={v}"),
        Instruction::Arg(k, _) => format!("ARG {k}"),
        Instruction::Healthcheck { test, .. } if test.is_empty() => "HEALTHCHECK NONE".into(),
        Instruction::Healthcheck { test, .. } => format!("HEALTHCHECK CMD {}", test.join(" ")),
        Instruction::StopSignal(s) => format!("STOPSIGNAL {s}"),
        Instruction::Volume(v) => format!("VOLUME {}", v.join(" ")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn expansion_uses_build_args_and_never_the_callers_environment() {
        let mut vars = BTreeMap::new();
        vars.insert("V".to_owned(), "3.21".to_owned());
        assert_eq!(expand("alpine:${V}", &vars), "alpine:3.21");
        assert_eq!(expand("alpine:$V", &vars), "alpine:3.21");
        // A build that picked up $HOME from whoever ran it would produce
        // a different image on every machine.
        unsafe { std::env::set_var("RUSTAINER_BUILD_TEST", "leaked") };
        assert_eq!(expand("$RUSTAINER_BUILD_TEST", &vars), "");
        assert_eq!(expand("100$ and ${UNSET}x", &vars), "100$ and x");
    }

    #[test]
    fn globs_and_dockerignore() {
        assert!(glob("package*.json", "package-lock.json"));
        assert!(!glob("package*.json", "sub/package.json"));
        assert!(glob("**/*.log", "a/b/c.log"));
        assert!(glob("**/*.log", "c.log"));
        assert!(!glob("a?", "a"));

        let dir = std::env::temp_dir().join(format!("rustainer-ign-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("node_modules/keep")).unwrap();
        std::fs::create_dir_all(dir.join("src")).unwrap();
        std::fs::write(
            dir.join(".dockerignore"),
            "# c\nnode_modules\n!node_modules/keep\n*.log\n",
        )
        .unwrap();
        for f in [
            "node_modules/x.js",
            "node_modules/keep/k.js",
            "src/a.rs",
            "b.log",
        ] {
            std::fs::write(dir.join(f), "x").unwrap();
        }
        let out = dir.join("out");
        let ig = Ignore::load(&dir);
        for name in ["node_modules", "src", "b.log"] {
            copy_into(&dir.join(name), &out.join(name), Some(&ig)).unwrap();
        }
        assert!(out.join("src/a.rs").exists());
        assert!(out.join("node_modules/keep/k.js").exists());
        assert!(!out.join("node_modules/x.js").exists());
        assert!(!out.join("b.log").exists());
        assert_eq!(sources(&dir, "s*").unwrap().len(), 1);
        assert!(sources(&dir, "nothing*").is_err());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_copy_from_layers_sees_what_a_container_would() {
        let dir = std::env::temp_dir().join(format!("rustainer-merged-{}", std::process::id()));
        let (low, high) = (dir.join("low"), dir.join("high"));
        std::fs::create_dir_all(low.join("app/sub")).unwrap();
        std::fs::create_dir_all(high.join("app")).unwrap();
        std::fs::write(low.join("app/old"), "old").unwrap();
        std::fs::write(low.join("app/both"), "low").unwrap();
        std::fs::write(low.join("app/sub/deep"), "deep").unwrap();
        std::fs::write(high.join("app/both"), "high").unwrap();
        std::fs::write(high.join("app/new"), "new").unwrap();
        let out = dir.join("out");
        copy_merged(&[low, high], Path::new("app"), &out).unwrap();
        assert_eq!(std::fs::read_to_string(out.join("both")).unwrap(), "high");
        assert_eq!(std::fs::read_to_string(out.join("old")).unwrap(), "old");
        assert_eq!(std::fs::read_to_string(out.join("new")).unwrap(), "new");
        assert_eq!(
            std::fs::read_to_string(out.join("sub/deep")).unwrap(),
            "deep"
        );
        assert!(copy_merged(&[dir.join("low")], Path::new("nope"), &out).is_err());
        // A link among the directories is followed inside the layers,
        // absolute or not, and never out to the host.
        let low = dir.join("low");
        std::os::unix::fs::symlink("/app", low.join("abs")).unwrap();
        std::os::unix::fs::symlink("app/sub", low.join("rel")).unwrap();
        std::os::unix::fs::symlink("/etc", low.join("host")).unwrap();
        let layers = [low];
        assert_eq!(
            sources_in(&layers, "/abs/sub/deep").unwrap(),
            [PathBuf::from("app/sub/deep")]
        );
        assert_eq!(
            sources_in(&layers, "/rel/d*").unwrap(),
            [PathBuf::from("app/sub/deep")]
        );
        assert!(!exists_in(
            &layers,
            &follow(&layers, Path::new("/host/passwd"))
        ));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_source_outside_the_context_is_refused() {
        let ctx = std::env::temp_dir().join("rustainer-build-ctx");
        std::fs::create_dir_all(ctx.join("sub")).unwrap();
        std::fs::write(ctx.join("sub/f"), b"x").unwrap();
        assert!(resolve_source(&ctx, "sub/f").is_ok());
        // Only visibly an escape after canonicalisation, which is why the
        // check is on the resolved path.
        let e = resolve_source(&ctx, "sub/../../etc")
            .unwrap_err()
            .to_string();
        assert!(
            e.contains("outside the build context") || e.contains("not in the build context"),
            "{e}"
        );
        std::fs::remove_dir_all(&ctx).ok();
    }

    #[test]
    fn a_layer_archive_is_byte_identical_for_the_same_tree() {
        // The diff_id is the hash of these bytes, so an unstable readdir
        // order would give the same filesystem two different identities.
        let d = std::env::temp_dir().join("rustainer-build-seal");
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(d.join("etc")).unwrap();
        std::fs::write(d.join("etc/b"), b"bbb").unwrap();
        std::fs::write(d.join("etc/a"), b"aaa").unwrap();
        std::fs::write(d.join("z"), b"z").unwrap();
        let (one, bytes) = seal_layer(&d).unwrap();
        let (two, _) = seal_layer(&d).unwrap();
        assert_eq!(one, two);
        assert!(!bytes.is_empty());
        assert!(one.starts_with("sha256:"));
        std::fs::remove_dir_all(&d).ok();
    }
}
