//! The Container Device Interface: a device, as its vendor describes it.
//!
//! A GPU is not a device node. It is a handful of nodes, a set of
//! userspace driver libraries that must match the kernel module on
//! this exact host, and some environment - and only the vendor's own
//! tooling knows which. CDI is the file format they write that down in
//! (`nvidia-ctk cdi generate` writes `/etc/cdi/nvidia.yaml`), and
//! Docker and Podman both read it. So does this: `--device
//! nvidia.com/gpu=all`, and `--gpus all` when such a file exists.
//!
//! # What is and is not done with a spec
//!
//! Device nodes and mounts are binds; environment is environment. A
//! spec also lists *hooks*: programs to run against the container
//! before it starts. They are not run - a file that names a program for
//! a container engine to execute as itself is not something to do on
//! its say-so. The two that every GPU spec has are instead done here,
//! by what they are for: `create-symlinks` is a list of links, and
//! `update-ldcache` exists so the loader can find each library under
//! its soname, which is one more link per library. Any other hook is
//! reported and skipped.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use rustainer_core::mount::VolumeMount;

#[derive(Default)]
pub struct Edits {
    pub mounts: Vec<VolumeMount>,
    pub env: Vec<String>,
    /// `(path in the container, what it points at)`.
    pub links: Vec<(PathBuf, String)>,
    pub notes: Vec<String>,
}

/// `vendor.com/class=name`, as opposed to a path under `/dev`.
pub fn is_name(s: &str) -> bool {
    !s.starts_with('/')
        && s.split_once('=')
            .is_some_and(|(kind, _)| kind.contains('/'))
}

fn specs() -> Vec<serde_json::Value> {
    let dirs = match std::env::var("RUSTAINER_CDI_DIRS") {
        Ok(d) => d.split(':').map(PathBuf::from).collect(),
        Err(_) => vec![PathBuf::from("/etc/cdi"), PathBuf::from("/var/run/cdi")],
    };
    let mut out = Vec::new();
    for dir in dirs {
        let mut files: Vec<PathBuf> = std::fs::read_dir(&dir)
            .into_iter()
            .flatten()
            .flatten()
            .map(|e| e.path())
            .collect();
        files.sort();
        for f in files {
            let Ok(text) = std::fs::read_to_string(&f) else {
                continue;
            };
            let parsed = match f.extension().and_then(|e| e.to_str()) {
                Some("json") => serde_json::from_str(&text).ok(),
                Some("yaml" | "yml") => serde_yaml_ng::from_str(&text).ok(),
                _ => None,
            };
            out.extend(parsed);
        }
    }
    out
}

/// Whether any spec on this machine describes devices of this kind.
pub fn has_kind(kind: &str) -> bool {
    specs().iter().any(|s| s["kind"] == kind)
}

/// Everything the named devices ask of a container.
pub fn resolve(names: &[String]) -> Result<Edits> {
    let mut edits = Edits::default();
    if names.is_empty() {
        return Ok(edits);
    }
    let specs = specs();
    let mut folders: Vec<String> = Vec::new();
    for full in names {
        let (kind, name) = full.split_once('=').expect("checked by is_name");
        let spec = specs.iter().find(|s| s["kind"] == kind).with_context(|| {
            format!(
                "--device {full}: no CDI spec on this machine describes {kind}. For an \
                 NVIDIA GPU, `nvidia-ctk cdi generate --output=/etc/cdi/nvidia.yaml` writes one."
            )
        })?;
        let device = spec["devices"]
            .as_array()
            .into_iter()
            .flatten()
            .find(|d| d["name"] == name)
            .with_context(|| {
                let have: Vec<&str> = spec["devices"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(|d| d["name"].as_str())
                    .collect();
                format!(
                    "--device {full}: {kind} has no device {name:?}; it has {}",
                    have.join(", ")
                )
            })?;
        // What every device of the spec needs, then what this one does.
        for part in [&spec["containerEdits"], &device["containerEdits"]] {
            apply(part, &mut edits, &mut folders).with_context(|| format!("--device {full}"))?;
        }
    }
    link_sonames(&mut edits, &folders);
    Ok(edits)
}

/// `update-ldcache`, by its effect: each library mounted into one of
/// `folders` reachable under the name programs were linked against.
fn link_sonames(edits: &mut Edits, folders: &[String]) {
    let wanted: Vec<(PathBuf, String)> = edits
        .mounts
        .iter()
        .filter(|m| {
            folders
                .iter()
                .any(|f| Path::new(&m.target).parent() == Some(Path::new(f)))
        })
        .filter_map(|m| {
            let file = Path::new(&m.target)
                .file_name()?
                .to_string_lossy()
                .into_owned();
            let so = soname(&m.source)?;
            (so != file).then(|| (Path::new(&m.target).with_file_name(so), file))
        })
        .collect();
    for link in wanted {
        if !edits.links.iter().any(|(p, _)| *p == link.0) {
            edits.links.push(link);
        }
    }
}

/// What an NVIDIA GPU needs, from the driver's own tool, on a host
/// that has the driver and no CDI spec for it.
///
/// `nvidia-container-cli list` prints the device nodes, driver
/// libraries, programs and firmware a container must be given, one
/// path a line, for the driver actually loaded. It is the same
/// information `nvidia-ctk` writes into a CDI spec, without needing
/// that tool or root to write the file.
pub fn nvidia_from_host() -> Option<Edits> {
    // As root it changes privilege to do its work, which a root that
    // exists only inside a user namespace is not allowed to; `--user`
    // is its own mode for a caller like that, and for a plain user.
    for flags in [&[][..], &["--user"][..]] {
        let out = std::process::Command::new("nvidia-container-cli")
            .args(flags)
            .arg("list")
            .stderr(std::process::Stdio::null())
            .output()
            .ok()?;
        let edits = from_paths(&String::from_utf8_lossy(&out.stdout));
        if out.status.success() && !edits.mounts.is_empty() {
            return Some(edits);
        }
    }
    None
}

fn from_paths(listing: &str) -> Edits {
    let mut edits = Edits::default();
    let mut folders: Vec<String> = Vec::new();
    for path in listing
        .lines()
        .map(str::trim)
        .filter(|l| l.starts_with('/'))
    {
        let source = PathBuf::from(path);
        let Ok(md) = std::fs::metadata(&source) else {
            continue;
        };
        let device = path.starts_with("/dev/");
        if path.contains(".so")
            && let Some(dir) = source.parent().and_then(|d| d.to_str())
            && !folders.iter().any(|f| f == dir)
        {
            folders.push(dir.to_owned());
        }
        edits.mounts.push(VolumeMount {
            source,
            target: path.to_owned(),
            // The driver's files are the host's; a container reads them.
            read_only: !device,
            is_file: !md.is_dir(),
            tmpfs: None,
        });
    }
    link_sonames(&mut edits, &folders);
    edits
}

fn apply(part: &serde_json::Value, edits: &mut Edits, folders: &mut Vec<String>) -> Result<()> {
    let list = |key: &str| part[key].as_array().cloned().unwrap_or_default();
    for e in list("env") {
        edits.env.extend(e.as_str().map(str::to_owned));
    }
    let mut bind = |host: &str, target: &str, read_only: bool| -> Result<()> {
        let source = PathBuf::from(host);
        if !target.starts_with('/') || target.split('/').any(|c| c == "..") {
            bail!("the spec names {target:?}, which is not a path a container can have");
        }
        let md = std::fs::metadata(&source)
            .with_context(|| format!("the spec names {host}, which this machine does not have"))?;
        if !edits.mounts.iter().any(|m| m.target == target) {
            edits.mounts.push(VolumeMount {
                source,
                target: target.to_owned(),
                read_only,
                is_file: !md.is_dir(),
                tmpfs: None,
            });
        }
        Ok(())
    };
    for d in list("deviceNodes") {
        let path = d["path"].as_str().context("a device node with no path")?;
        bind(d["hostPath"].as_str().unwrap_or(path), path, false)?;
    }
    for m in list("mounts") {
        let host = m["hostPath"].as_str().context("a mount with no hostPath")?;
        let target = m["containerPath"].as_str().unwrap_or(host);
        let ro = m["options"]
            .as_array()
            .is_some_and(|o| o.iter().any(|x| x == "ro"));
        bind(host, target, ro)?;
    }
    for h in list("hooks") {
        let args: Vec<&str> = h["args"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|a| a.as_str())
            .collect();
        let after = |flag: &str| -> Vec<String> {
            args.windows(2)
                .filter(|w| w[0] == flag)
                .map(|w| w[1].to_owned())
                .collect()
        };
        if args.contains(&"update-ldcache") {
            folders.extend(after("--folder"));
        } else if args.contains(&"create-symlinks") {
            for link in after("--link") {
                if let Some((target, path)) = link.split_once("::") {
                    edits.links.push((PathBuf::from(path), target.to_owned()));
                }
            }
        } else {
            let what = args.get(1).or(args.first()).copied().unwrap_or("?");
            edits
                .notes
                .push(format!("the device's hook {what:?} is not run"));
        }
    }
    Ok(())
}

/// The name a shared library asks to be found under: `DT_SONAME`, read
/// from the file. `None` for anything that is not a 64-bit
/// little-endian ELF file with one.
pub fn soname(path: &Path) -> Option<String> {
    use std::os::unix::fs::FileExt;
    let f = std::fs::File::open(path).ok()?;
    let read = |off: u64, len: usize| -> Option<Vec<u8>> {
        let mut b = vec![0u8; len];
        f.read_exact_at(&mut b, off).ok()?;
        Some(b)
    };
    let u64_at = |b: &[u8], i: usize| u64::from_le_bytes(b[i..i + 8].try_into().unwrap());
    let head = read(0, 64)?;
    if &head[..4] != b"\x7fELF" || head[4] != 2 || head[5] != 1 {
        return None;
    }
    let phoff = u64_at(&head, 0x20);
    let phentsize = u16::from_le_bytes([head[0x36], head[0x37]]) as u64;
    let phnum = u16::from_le_bytes([head[0x38], head[0x39]]) as u64;
    if phentsize < 56 || phnum > 512 {
        return None;
    }
    let (mut loads, mut dynamic) = (Vec::new(), None);
    for i in 0..phnum {
        let ph = read(phoff + i * phentsize, 56)?;
        let kind = u32::from_le_bytes(ph[0..4].try_into().unwrap());
        let (offset, vaddr, filesz) = (u64_at(&ph, 8), u64_at(&ph, 16), u64_at(&ph, 32));
        match kind {
            1 => loads.push((vaddr, offset, filesz)),
            2 => dynamic = Some((offset, filesz)),
            _ => {}
        }
    }
    let (dyn_off, dyn_len) = dynamic?;
    let table = read(dyn_off, dyn_len.min(1 << 16) as usize)?;
    let (mut name_at, mut strtab) = (None, None);
    for entry in table.as_chunks::<16>().0 {
        match u64_at(entry, 0) {
            14 => name_at = Some(u64_at(entry, 8)),
            5 => strtab = Some(u64_at(entry, 8)),
            0 => break,
            _ => {}
        }
    }
    let (name_at, strtab) = (name_at?, strtab?);
    // The string table is given as an address; find the file offset.
    let (vaddr, offset, _) = loads
        .into_iter()
        .find(|(v, _, size)| strtab >= *v && strtab < v + size)?;
    let mut name = Vec::new();
    for i in 0..255u64 {
        let c = read(offset + (strtab - vaddr) + name_at + i, 1)?[0];
        if c == 0 {
            break;
        }
        name.push(c);
    }
    String::from_utf8(name)
        .ok()
        .filter(|n| !n.is_empty() && !n.contains('/'))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_listing_of_paths_becomes_devices_and_read_only_files() {
        let dir = std::env::temp_dir().join(format!("rustainer-nv-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let lib = dir.join("libnvidia-ml.so.580.1");
        std::fs::write(&lib, "x").unwrap();
        let edits = from_paths(&format!(
            "/dev/null\n{}\n/no/such/file\nnot a path\n",
            lib.display()
        ));
        assert_eq!(edits.mounts.len(), 2);
        assert!(!edits.mounts[0].read_only && edits.mounts[0].target == "/dev/null");
        assert!(edits.mounts[1].read_only && edits.mounts[1].is_file);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_cdi_name_is_not_a_path() {
        assert!(is_name("nvidia.com/gpu=all"));
        assert!(is_name("example.com/fpga=0"));
        assert!(!is_name("/dev/fuse"));
        assert!(!is_name("/dev/sda:/dev/xvda"));
        assert!(!is_name("gpu=all"));
    }

    #[test]
    fn a_spec_becomes_mounts_environment_and_links() {
        let dir = std::env::temp_dir().join(format!("rustainer-cdi-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let lib = dir.join("libx.so.1.2.3");
        std::fs::write(&lib, "not a library").unwrap();
        let spec = serde_json::json!({
            "kind": "example.com/gpu",
            "containerEdits": {
                "env": ["A=1"],
                "mounts": [{"hostPath": lib, "containerPath": "/usr/lib/libx.so.1.2.3", "options": ["ro"]}],
                "hooks": [
                    {"args": ["h", "create-symlinks", "--link", "libx.so.1.2.3::/usr/lib/libx.so"]},
                    {"args": ["h", "update-ldcache", "--folder", "/usr/lib"]},
                    {"args": ["h", "chmod", "--path", "/dev/dri"]}
                ]
            },
            "devices": [{"name": "0", "containerEdits": {"deviceNodes": [{"path": "/dev/gpu0", "hostPath": "/dev/null"}]}}]
        });
        let (mut edits, mut folders) = (Edits::default(), Vec::new());
        apply(&spec["containerEdits"], &mut edits, &mut folders).unwrap();
        apply(
            &spec["devices"][0]["containerEdits"],
            &mut edits,
            &mut folders,
        )
        .unwrap();
        assert_eq!(edits.env, ["A=1"]);
        assert_eq!(folders, ["/usr/lib"]);
        assert_eq!(edits.mounts.len(), 2);
        assert!(edits.mounts[0].read_only && edits.mounts[0].is_file);
        assert_eq!(edits.mounts[1].target, "/dev/gpu0");
        assert_eq!(
            edits.links,
            [(
                PathBuf::from("/usr/lib/libx.so"),
                "libx.so.1.2.3".to_owned()
            )]
        );
        assert_eq!(edits.notes.len(), 1);
        // Not ELF: no soname, and no panic on a short file.
        assert_eq!(soname(&lib), None);
        // A path that leaves the container is the spec's mistake to own.
        let bad =
            serde_json::json!({"mounts": [{"hostPath": lib, "containerPath": "/usr/../../etc/x"}]});
        assert!(apply(&bad, &mut Edits::default(), &mut Vec::new()).is_err());
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
