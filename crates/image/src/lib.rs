//! Image pulling, verification and extraction.
//!
//! A separate subsystem from container execution, with one narrow
//! interface: this produces a content-addressed store and a materialised
//! rootfs; the runtime consumes a directory path and a config. Neither
//! calls into the other (RND.md §18.4).
//!
//! Synchronous throughout. Nothing here may introduce an executor, because
//! this code runs in the same process that later calls `clone3`.

pub mod extract;
pub mod reference;
pub mod registry;

use std::io::Write;
use std::path::{Path, PathBuf};

pub use reference::Reference;
use registry::{Client, Fetched, ImageConfig, sha256_hex};

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error(transparent)]
    Registry(#[from] registry::Error),
    #[error(transparent)]
    Extract(#[from] extract::Error),
    #[error("{0}")]
    Reference(String),
    #[error(
        "layer {index} does not match the image config: the uncompressed bytes hash to \
         {actual}, but the config declares diff_id {expected}. The compressed blob matched \
         its own digest, so this is a layer that is internally consistent and does not \
         belong to this image."
    )]
    DiffIdMismatch {
        index: usize,
        expected: String,
        actual: String,
    },
    #[error(
        "no image {reference:?} in the local store. `rustainer images` lists what is \
         there, by the same name it accepts here."
    )]
    NoSuchImage { reference: String },
    #[error("{op} {path}: {source}")]
    Io {
        op: &'static str,
        path: String,
        #[source]
        source: std::io::Error,
    },
}

fn io(op: &'static str, path: impl AsRef<Path>) -> impl FnOnce(std::io::Error) -> Error {
    let path = path.as_ref().display().to_string();
    move |source| Error::Io { op, path, source }
}

/// Where images live.
///
/// ```text
/// <root>/blobs/sha256/<hex>             verified bytes, by digest
/// <root>/images/<manifest-digest>/      rootfs/ and config.json
/// <root>/tags/<key>                     reference -> manifest digest
/// ```
///
/// Flat extraction per image, not shared layers: stage 7 introduces
/// OverlayFS and keys extracted layers by ChainID. Keeping the store
/// content-addressed from the start is what makes that an addition rather
/// than a migration.
#[derive(Clone, Debug)]
pub struct Store {
    root: PathBuf,
}

/// What the runtime needs to run an image.
#[derive(Clone, Debug)]
pub struct Materialised {
    /// Extracted layer directories, lowest first.
    ///
    /// Kept separate rather than flattened into one tree: that is what
    /// lets two containers from one image share the read-only layers and
    /// each get a private writable upper, and what makes a second image
    /// sharing a base layer cost nothing.
    pub layers: Vec<PathBuf>,
    pub digest: String,
    pub env: Vec<String>,
    pub entrypoint: Vec<String>,
    pub cmd: Vec<String>,
    pub working_dir: Option<String>,
    pub user: Option<String>,
    pub report: extract::Report,
}

impl Materialised {
    /// `entrypoint + cmd`, the way the spec composes them.
    pub fn command(&self) -> Vec<String> {
        let mut v = self.entrypoint.clone();
        v.extend(self.cmd.iter().cloned());
        v
    }
}

impl Store {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    pub fn default_root() -> PathBuf {
        if let Some(v) = std::env::var_os("RUSTAINER_IMAGE_ROOT") {
            return PathBuf::from(v);
        }
        if rustix::process::geteuid().is_root() {
            return PathBuf::from("/var/lib/rustainer");
        }
        match std::env::var_os("HOME") {
            Some(h) => PathBuf::from(h).join(".local/share/rustainer"),
            None => PathBuf::from("/tmp/rustainer-images"),
        }
    }

    fn blob_path(&self, digest: &str) -> PathBuf {
        self.root
            .join("blobs/sha256")
            .join(digest.trim_start_matches("sha256:"))
    }
    fn image_dir(&self, digest: &str) -> PathBuf {
        self.root
            .join("images")
            .join(digest.trim_start_matches("sha256:"))
    }
    /// Extracted layers, keyed by their uncompressed digest.
    ///
    /// Partitioned by extended-attribute mode, because a layer written
    /// with `trusted.overlay.*` whiteouts is silently unreadable by a
    /// `userxattr` mount and vice versa: deleted files quietly reappear.
    /// Sharing one directory between the two modes corrupts both.
    fn layer_dir(&self, diff_id: &str, userxattr: bool) -> PathBuf {
        self.root
            .join(if userxattr { "layers-user" } else { "layers" })
            .join(diff_id.trim_start_matches("sha256:"))
    }

    /// Are we outside the initial user namespace?
    fn userxattr() -> bool {
        std::fs::read_to_string("/proc/self/uid_map")
            .map(|m| m.split_whitespace().collect::<Vec<_>>() != ["0", "0", "4294967295"])
            .unwrap_or(false)
    }
    fn tag_path(&self, key: &str) -> PathBuf {
        self.root.join("tags").join(key)
    }

    pub fn prepare(&self) -> Result<(), Error> {
        for d in ["blobs/sha256", "images", "tags", "layers", "layers-user"] {
            let p = self.root.join(d);
            std::fs::create_dir_all(&p).map_err(io("creating", &p))?;
        }
        Ok(())
    }

    /// Images in the store, newest directory first is not tracked; sorted.
    pub fn list(&self) -> Result<Vec<(String, String)>, Error> {
        let dir = self.root.join("tags");
        let mut out = Vec::new();
        let Ok(rd) = std::fs::read_dir(&dir) else {
            return Ok(out);
        };
        for e in rd.flatten() {
            if let Ok(d) = std::fs::read_to_string(e.path()) {
                out.push((
                    e.file_name().to_string_lossy().into_owned(),
                    d.trim().to_owned(),
                ));
            }
        }
        out.sort();
        Ok(out)
    }

    pub fn remove(&self, reference: &str) -> Result<(), Error> {
        // A reference, or the key `list` printed for it.
        //
        // Those are not the same string: a store key has `/` and `:`
        // replaced by `_`, so `mirror.gcr.io/library/alpine:3.21` is
        // listed as `mirror.gcr.io_library_alpine_3.21` - and feeding
        // that back produced "no such file" naming a path nobody typed.
        // A tool has to accept what it prints; the UI's remove button is
        // simply the first caller that had nothing else to pass.
        let by_reference = Reference::parse(reference)
            .ok()
            .map(|r| self.tag_path(&r.store_key()))
            .filter(|p| p.exists());
        let tag = match by_reference {
            Some(p) => p,
            None => {
                let direct = self.tag_path(reference);
                if direct.exists() {
                    direct
                } else {
                    // Report the reference form, which is what a person
                    // most likely meant, rather than whichever of the two
                    // we happened to try second.
                    self.tag_path(
                        &Reference::parse(reference)
                            .map_err(Error::Reference)?
                            .store_key(),
                    )
                }
            }
        };
        if !tag.exists() {
            // Said as the thing the person did, not as the path we
            // happened to build out of it.
            return Err(Error::NoSuchImage {
                reference: reference.to_owned(),
            });
        }
        let digest = std::fs::read_to_string(&tag).ok();
        std::fs::remove_file(&tag).map_err(io("removing the tag", &tag))?;
        if let Some(d) = digest {
            let dir = self.image_dir(d.trim());
            // Only this image's record goes. Blobs and extracted layers
            // stay: they are content-addressed, another image may share
            // them, and re-fetching is far more expensive than the disk.
            let _ = std::fs::remove_dir_all(&dir);
        }
        Ok(())
    }

    /// Everything the runtime needs, from the store alone, with no network.
    ///
    /// `run <image>` used to call [`Self::pull`] unconditionally, and
    /// `pull` begins by fetching the manifest - so starting a container
    /// from an image already on disk cost four TLS round-trips to the
    /// registry before anything was cloned. That was 870 of the 890
    /// milliseconds a container start took, and, worse than slow, it made
    /// a cached image **unrunnable offline**: the whole command failed at
    /// a DNS lookup for an image whose every byte was local.
    ///
    /// Returns `None` - never an error - when anything is missing or
    /// unreadable, so the caller simply falls back to pulling. A store
    /// that has been partly garbage-collected, or written by a build with
    /// a different `userxattr` setting, is not a failure; it is a cache
    /// miss.
    ///
    /// This is Docker's `--pull=missing`, which is Docker's default.
    pub fn local(&self, reference: &str) -> Option<Materialised> {
        let r = Reference::parse(reference).ok()?;
        let manifest_digest = std::fs::read_to_string(self.tag_path(&r.store_key())).ok()?;
        let manifest_digest = manifest_digest.trim();
        let config_bytes =
            std::fs::read(self.image_dir(manifest_digest).join("config.json")).ok()?;
        let config: ImageConfig = serde_json::from_slice(&config_bytes).ok()?;

        // Every layer, in order, and all of them. A missing one means the
        // extracted tree is incomplete, and half an image is not a cache
        // hit - overlaying what is left would produce a filesystem that
        // looks plausible and is wrong.
        let userxattr = Self::userxattr();
        let mut layers = Vec::new();
        for diff_id in &config.rootfs.diff_ids {
            let dir = self.layer_dir(diff_id, userxattr);
            if !dir.is_dir() {
                return None;
            }
            layers.push(dir);
        }
        if layers.is_empty() {
            return None;
        }

        // An empty report, because this call extracted nothing. The
        // degradations a report carries are about an extraction that has
        // already happened and was already reported when it did.
        self.materialise(manifest_digest, &config, layers, extract::Report::default())
            .ok()
    }

    /// Pull if needed, then return what the runtime needs.
    pub fn pull(&self, reference: &str, force: bool) -> Result<Materialised, Error> {
        let r = Reference::parse(reference).map_err(Error::Reference)?;
        self.prepare()?;

        let mut client = Client::new();

        // Step 4: the manifest or index, classified by what arrived.
        let (fetched, digest, _body) = client.manifest(&r)?;

        // Step 5: platform selection, if this was an index.
        let (manifest, manifest_digest) = match fetched {
            Fetched::Manifest(m) => (m, digest),
            Fetched::Index(idx) => {
                let d = registry::select_platform(&idx, "linux", std::env::consts::ARCH)?;
                let m = client.manifest_by_digest(&r, &d.digest)?;
                (m, d.digest.clone())
            }
        };

        let image_dir = self.image_dir(&manifest_digest);
        let config_path = image_dir.join("config.json");

        // Step 7: the config blob. Its digest is the image ID.
        let config_bytes = self.blob(&mut client, &r, &manifest.config.digest)?;
        let config: ImageConfig =
            serde_json::from_slice(&config_bytes).map_err(|source| registry::Error::Parse {
                what: "the image config".into(),
                source,
            })?;

        let userxattr = Self::userxattr();
        let mut layer_dirs = Vec::new();
        let mut report = extract::Report::default();

        for (i, layer) in manifest.layers.iter().enumerate() {
            let diff_id = config
                .rootfs
                .diff_ids
                .get(i)
                .cloned()
                .unwrap_or_else(|| layer.digest.clone());
            let dir = self.layer_dir(&diff_id, userxattr);
            if dir.exists() && !force {
                layer_dirs.push(dir);
                continue;
            }
            if dir.exists() {
                std::fs::remove_dir_all(&dir).map_err(io("removing a stale layer", &dir))?;
            }

            // A fresh, empty, per-attempt staging directory, committed by
            // rename only once the whole layer verified. A half-extracted
            // layer must never be visible under its content address.
            let staging = dir.with_extension("staging");
            let _ = std::fs::remove_dir_all(&staging);
            std::fs::create_dir_all(&staging)
                .map_err(io("creating a staging directory", &staging))?;

            // Both digests: the compressed one says these are the bytes we
            // asked for, the uncompressed one says they belong to this image.
            let bytes = self.blob(&mut client, &r, &layer.digest)?;
            let gz = flate2::read::GzDecoder::new(&bytes[..]);
            let mut hashing = registry::HashingReader::new(gz);
            {
                let root = extract::SafeRoot::open(&staging)?;
                extract::apply_layer(
                    &root,
                    &mut hashing,
                    extract::Limits::default(),
                    userxattr,
                    &mut report,
                )?;
            }
            // The tar reader stops at the end-of-archive marker and leaves
            // the trailing padding unread, so drain before hashing.
            std::io::copy(&mut hashing, &mut std::io::sink())
                .map_err(io("draining the layer stream", "layer"))?;
            let actual = hashing.digest();
            if let Some(expected) = config.rootfs.diff_ids.get(i)
                && *expected != actual
            {
                let _ = std::fs::remove_dir_all(&staging);
                return Err(Error::DiffIdMismatch {
                    index: i,
                    expected: expected.clone(),
                    actual,
                });
            }
            if let Some(p) = dir.parent() {
                std::fs::create_dir_all(p).map_err(io("creating the layer directory", p))?;
            }
            std::fs::rename(&staging, &dir).map_err(io("committing a layer", &dir))?;
            layer_dirs.push(dir);
        }

        std::fs::create_dir_all(&image_dir)
            .map_err(io("creating the image directory", &image_dir))?;
        let mut f =
            std::fs::File::create(&config_path).map_err(io("writing the config", &config_path))?;
        f.write_all(&config_bytes)
            .map_err(io("writing the config", &config_path))?;

        let tag = self.tag_path(&r.store_key());
        std::fs::write(&tag, &manifest_digest).map_err(io("recording the tag", &tag))?;

        self.materialise(&manifest_digest, &config, layer_dirs, report)
    }

    fn materialise(
        &self,
        digest: &str,
        config: &ImageConfig,
        layers: Vec<PathBuf>,
        report: extract::Report,
    ) -> Result<Materialised, Error> {
        let c = config.config.clone().unwrap_or_default();
        Ok(Materialised {
            layers,
            digest: digest.to_owned(),
            env: c.env.unwrap_or_default(),
            entrypoint: c.entrypoint.unwrap_or_default(),
            cmd: c.cmd.unwrap_or_default(),
            // An absent field and a present-but-empty one mean the same
            // thing — "not set" — and a surprising number of real images
            // write the empty string. `quay.io/podman/hello` is one, and
            // taking it literally produces `--user: "" is not a numeric
            // uid`, which blames the user for a field they never typed.
            working_dir: c.working_dir.filter(|s| !s.is_empty()),
            user: c.user.filter(|s| !s.is_empty()),
            report,
        })
    }

    /// A blob, from the store if we already verified it, else fetched.
    fn blob(&self, client: &mut Client, r: &Reference, digest: &str) -> Result<Vec<u8>, Error> {
        let path = self.blob_path(digest);
        if let Ok(bytes) = std::fs::read(&path) {
            // Re-verify on read. A store is a cache, and a cache that is
            // trusted without checking is an attacker's favourite place to
            // put something.
            if sha256_hex(&bytes) == digest {
                return Ok(bytes);
            }
            let _ = std::fs::remove_file(&path);
        }
        let bytes = client.blob(r, digest)?;
        if let Some(p) = path.parent() {
            std::fs::create_dir_all(p).map_err(io("creating the blob directory", p))?;
        }
        let tmp = path.with_extension("tmp");
        std::fs::write(&tmp, &bytes).map_err(io("writing a blob", &tmp))?;
        std::fs::rename(&tmp, &path).map_err(io("committing a blob", &path))?;
        Ok(bytes)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_store_lays_itself_out_predictably() {
        let root = std::env::temp_dir().join("rustainer-store-test");
        let _ = std::fs::remove_dir_all(&root);
        let s = Store::new(&root);
        s.prepare().unwrap();
        assert!(root.join("blobs/sha256").is_dir());
        assert!(root.join("images").is_dir());
        assert!(root.join("tags").is_dir());
        assert_eq!(s.list().unwrap().len(), 0);
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn a_tag_key_is_filesystem_safe() {
        let r = Reference::parse("ghcr.io/owner/name:v1").unwrap();
        let k = r.store_key();
        assert!(!k.contains('/'), "{k}");
        assert!(!k.contains(':'), "{k}");
    }

    #[test]
    fn entrypoint_and_cmd_compose_the_way_the_spec_says() {
        let m = Materialised {
            layers: vec![],
            digest: "sha256:x".into(),
            env: vec![],
            entrypoint: vec!["/bin/sh".into(), "-c".into()],
            cmd: vec!["echo hi".into()],
            working_dir: None,
            user: None,
            report: extract::Report::default(),
        };
        assert_eq!(m.command(), vec!["/bin/sh", "-c", "echo hi"]);
    }

    #[test]
    fn an_image_with_only_a_cmd_still_has_a_command() {
        let m = Materialised {
            layers: vec![],
            digest: "sha256:x".into(),
            env: vec![],
            entrypoint: vec![],
            cmd: vec!["/bin/sh".into()],
            working_dir: None,
            user: None,
            report: extract::Report::default(),
        };
        assert_eq!(m.command(), vec!["/bin/sh"]);
    }

    #[test]
    fn the_diff_id_error_explains_what_it_means() {
        let e = Error::DiffIdMismatch {
            index: 0,
            expected: "sha256:a".into(),
            actual: "sha256:b".into(),
        };
        let s = e.to_string();
        assert!(s.contains("does not belong to this image"), "{s}");
    }
}
