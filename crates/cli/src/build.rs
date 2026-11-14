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
//! No parallelism, no content-addressed build cache keyed on
//! instructions, no multi-stage `--from`. BuildKit is those three things
//! and they are each a project. A cache that is wrong is worse than no
//! cache, and `--from` needs a graph where this has a list.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};

use crate::dockerfile::Instruction;

/// What the build accumulates: layers on disk, and the config that will
/// describe them.
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

/// Copy a file or directory tree into `dest`, preserving modes.
pub fn copy_into(src: &Path, dest: &Path) -> Result<()> {
    let md = std::fs::symlink_metadata(src)?;
    if md.is_dir() {
        std::fs::create_dir_all(dest)?;
        for e in std::fs::read_dir(src)? {
            let e = e?;
            copy_into(&e.path(), &dest.join(e.file_name()))?;
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
    Ok(())
}

/// The instructions that produce a layer, in order, for reporting.
pub fn describe(i: &Instruction) -> String {
    match i {
        Instruction::From { image, .. } => format!("FROM {image}"),
        Instruction::Run(argv) => format!("RUN {}", argv.join(" ")),
        Instruction::Copy { sources, dest, .. } => {
            format!("COPY {} {dest}", sources.join(" "))
        }
        Instruction::Env(k, v) => format!("ENV {k}={v}"),
        Instruction::Workdir(w) => format!("WORKDIR {w}"),
        Instruction::Cmd(c) => format!("CMD {}", c.join(" ")),
        Instruction::Entrypoint(c) => format!("ENTRYPOINT {}", c.join(" ")),
        Instruction::User(u) => format!("USER {u}"),
        Instruction::Expose(p) => format!("EXPOSE {p}"),
        Instruction::Label(k, v) => format!("LABEL {k}={v}"),
        Instruction::Arg(k, _) => format!("ARG {k}"),
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
