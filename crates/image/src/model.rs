//! Models: a GGUF file in a registry, and a server that loads it.
//!
//! Docker publishes models as OCI artifacts (`ai/smollm2` and the rest
//! of the `ai` namespace): an ordinary manifest whose layer is the GGUF
//! file itself, media type `application/vnd.docker.ai.gguf.*`. Pulling
//! one needs nothing the image pull does not already have, except a
//! download that goes to disk rather than through memory.

use std::io::Read;
use std::path::{Path, PathBuf};

use crate::reference::Reference;
use crate::registry::{Client, Fetched};

/// Where a model's file lives: `<root>/models/<name>/<tag>.gguf`. The
/// path is the record - there is no index to fall out of step with it.
pub fn path(root: &Path, r: &Reference) -> PathBuf {
    root.join("models")
        .join(&r.name)
        .join(format!("{}.gguf", r.tag))
}

/// Every model on disk, as `(reference, bytes)`.
pub fn list(root: &Path) -> Vec<(String, u64)> {
    fn walk(dir: &Path, name: &str, out: &mut Vec<(String, u64)>) {
        for e in std::fs::read_dir(dir).into_iter().flatten().flatten() {
            let file = e.file_name().to_string_lossy().into_owned();
            let p = e.path();
            if p.is_dir() {
                let sep = if name.is_empty() { "" } else { "/" };
                walk(&p, &format!("{name}{sep}{file}"), out);
            } else if let Some(tag) = file.strip_suffix(".gguf") {
                out.push((format!("{name}:{tag}"), e.metadata().map_or(0, |m| m.len())));
            }
        }
    }
    let mut out = Vec::new();
    walk(&root.join("models"), "", &mut out);
    out.sort();
    out
}

/// Download the GGUF layer of `r` into the store. Returns its path and
/// whether anything was downloaded.
pub fn pull(root: &Path, r: &Reference) -> Result<(PathBuf, bool), String> {
    let dest = path(root, r);
    if dest.exists() {
        return Ok((dest, false));
    }
    let mut client = Client::for_reading(r);
    let manifest = match client.manifest(r).map_err(|e| e.to_string())? {
        (Fetched::Manifest(m), _, _) => m,
        (Fetched::Index(_), _, _) => return Err(format!("{} is an image, not a model", r.name)),
    };
    let layer = manifest
        .layers
        .iter()
        .find(|l| l.media_type.contains("gguf"))
        .ok_or_else(|| {
            format!(
                "{} has no GGUF layer; it is an image, or a model in a format \
                 (safetensors, MLX) the llama.cpp server cannot load",
                r.name
            )
        })?;
    std::fs::create_dir_all(dest.parent().unwrap()).map_err(|e| e.to_string())?;
    client
        .blob_to_file(r, &layer.digest, &dest)
        .map_err(|e| e.to_string())?;
    Ok((dest, true))
}

fn agent(seconds: u64) -> ureq::Agent {
    ureq::Agent::config_builder()
        .http_status_as_error(false)
        .timeout_global(Some(std::time::Duration::from_secs(seconds)))
        .build()
        .new_agent()
}

/// Has the server at `base` finished loading its model?
pub fn ready(base: &str) -> bool {
    agent(3)
        .get(&format!("{base}/health"))
        .call()
        .is_ok_and(|r| r.status().as_u16() == 200)
}

/// One question, one answer, over the OpenAI-compatible endpoint.
pub fn ask(base: &str, prompt: &str) -> Result<String, String> {
    let url = format!("{base}/v1/chat/completions");
    let body = serde_json::json!({ "messages": [{ "role": "user", "content": prompt }] });
    // ponytail: ten minutes and no streaming; stream the tokens when
    // someone runs a model slow enough to want to watch.
    let resp = agent(600)
        .post(&url)
        .header("Content-Type", "application/json")
        .send(body.to_string().as_bytes())
        .map_err(|e| format!("{url}: {e}"))?;
    let status = resp.status().as_u16();
    let mut raw = Vec::new();
    resp.into_body()
        .into_reader()
        .take(8 * 1024 * 1024)
        .read_to_end(&mut raw)
        .map_err(|e| e.to_string())?;
    let doc: serde_json::Value = serde_json::from_slice(&raw).unwrap_or_default();
    match doc["choices"][0]["message"]["content"].as_str() {
        Some(text) if status == 200 => Ok(text.trim().to_owned()),
        _ => Err(format!(
            "the model server answered HTTP {status}: {}",
            String::from_utf8_lossy(&raw)
                .chars()
                .take(200)
                .collect::<String>()
        )),
    }
}

#[test]
fn a_model_is_listed_by_the_reference_that_pulled_it() {
    let root = std::env::temp_dir().join(format!("rustainer-model-test-{}", std::process::id()));
    let r = Reference::parse("ai/smollm2:360M-Q4_K_M").unwrap();
    let p = path(&root, &r);
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(&p, b"gguf").unwrap();
    assert_eq!(list(&root), [("ai/smollm2:360M-Q4_K_M".to_owned(), 4)]);
    std::fs::remove_dir_all(&root).unwrap();
}
