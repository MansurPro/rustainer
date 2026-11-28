//! Reading a Docker Compose file as a stack.
//!
//! A translation, not a second engine: the YAML becomes the same table
//! a `rustainer.toml` would have parsed to, and everything after that -
//! validation, ordering, `up` - is the stack code, unchanged.
//!
//! A key this cannot honour is an error that names it, as it is in a
//! stack file. The exception is a short list of keys whose absence
//! does not change whether the application works (`container_name`,
//! `labels`, `networks` ...): those are dropped, and reported.

use std::collections::BTreeMap;
use std::path::Path;

use anyhow::{Context, Result, bail};
use serde_json::Value as J;
use toml::Value as T;

/// Dropped with a note rather than refused.
const IGNORED: &[&str] = &[
    "container_name",
    "hostname",
    "networks",
    "labels",
    "tty",
    "stdin_open",
    "logging",
    "expose",
    "pull_policy",
    "stop_grace_period",
    "stop_signal",
    "init",
    "attach",
    "annotations",
    "memswap_limit",
    "oom_score_adj",
    "cpu_shares",
];

type Table = toml::map::Map<String, T>;

/// A Compose document as a stack document, and what was left out.
///
/// `dir` is the Compose file's directory: relative host paths are
/// resolved against it, and its name is the stack's when the file
/// gives none - both as Compose does.
pub fn to_stack(
    yaml: &str,
    dir: &Path,
    vars: &BTreeMap<String, String>,
) -> Result<(T, Vec<String>)> {
    let mut doc = load(yaml)?;
    let names: Vec<String> = doc
        .get("services")
        .and_then(J::as_object)
        .context("a Compose file needs a `services` mapping")?
        .keys()
        .cloned()
        .collect();
    for n in names {
        let svc = doc["services"][&n].take();
        doc["services"][&n] =
            extend_from_files(svc, dir, vars, 0).with_context(|| format!("service {n:?}"))?;
    }
    let top = doc
        .as_object()
        .context("a Compose file is a mapping with a `services` key")?;
    let mut notes = Vec::new();
    let mut out = Table::new();

    let name = match top.get("name").and_then(J::as_str) {
        Some(n) => n.to_owned(),
        None => {
            let d = dir.canonicalize().unwrap_or_else(|_| dir.to_path_buf());
            d.file_name()
                .map(|n| n.to_string_lossy().to_lowercase())
                .unwrap_or_default()
                .chars()
                .filter(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_')
                .collect()
        }
    };
    out.insert("name".into(), T::String(name));

    let mut services = Table::new();
    for (name, svc) in top
        .get("services")
        .and_then(J::as_object)
        .context("a Compose file needs a `services` mapping")?
    {
        let t = service(svc, dir, &mut notes).with_context(|| format!("service {name:?}"))?;
        services.insert(name.clone(), T::Table(t));
    }
    out.insert("services".into(), T::Table(services));

    for key in ["configs", "secrets"] {
        let Some(m) = top.get(key).and_then(J::as_object) else {
            continue;
        };
        let mut t = Table::new();
        for (n, v) in m {
            let file = v
                .get("file")
                .and_then(J::as_str)
                .with_context(|| format!("{key}.{n}: only `file:` sources are supported"))?;
            let mut f = Table::new();
            f.insert("file".into(), T::String(file.into()));
            t.insert(n.clone(), T::Table(f));
        }
        out.insert(key.into(), T::Table(t));
    }
    for k in top.keys() {
        match k.as_str() {
            "name" | "services" | "configs" | "secrets" | "version" => {}
            // Named volumes are created on first use here, so declaring
            // one says nothing new - unless it asks for something.
            "volumes" => {
                for (n, v) in top[k].as_object().into_iter().flatten() {
                    if v.as_object().is_some_and(|o| !o.is_empty()) {
                        notes.push(format!("volumes.{n}: its options are ignored"));
                    }
                }
            }
            "networks" => notes.push("networks: ignored; a stack has one network".into()),
            x if x.starts_with("x-") => {}
            other => bail!("the top-level key {other:?} is not supported"),
        }
    }
    Ok((T::Table(out), notes))
}

/// YAML to a plain tree: merge keys applied, tags dropped.
fn load(yaml: &str) -> Result<J> {
    let mut doc: serde_yaml_ng::Value = serde_yaml_ng::from_str(yaml).context("not valid YAML")?;
    doc.apply_merge().context("resolving `<<` merge keys")?;
    let doc = untag(doc);
    let doc: J = serde_json::to_value(doc).context("a YAML key that is not a string")?;
    if !doc.is_object() {
        bail!("a Compose file is a mapping with a `services` key");
    }
    Ok(doc)
}

/// `!override` asks for a list to replace the one it is merged over,
/// which is what merging stack files does to every list already, so
/// the tag has nothing left to say. `!reset` asks for the key to go.
fn untag(v: serde_yaml_ng::Value) -> serde_yaml_ng::Value {
    use serde_yaml_ng::Value as Y;
    match v {
        Y::Tagged(t) if t.tag == "!reset" => Y::Null,
        Y::Tagged(t) => untag(t.value),
        Y::Sequence(s) => Y::Sequence(s.into_iter().map(untag).collect()),
        Y::Mapping(m) => Y::Mapping(m.into_iter().map(|(k, v)| (k, untag(v))).collect()),
        other => other,
    }
}

/// `extends: {file, service}`: that service, from that file, with this
/// one laid over it. A service of the same file is left for the stack
/// to resolve, which already knows how.
fn extend_from_files(
    mut svc: J,
    dir: &Path,
    vars: &BTreeMap<String, String>,
    depth: usize,
) -> Result<J> {
    let Some(file) = svc.pointer("/extends/file").and_then(J::as_str) else {
        return Ok(svc);
    };
    if depth > 16 {
        bail!("extends: more than 16 files deep, which is a loop");
    }
    let path = dir.join(file);
    let name = svc
        .pointer("/extends/service")
        .and_then(J::as_str)
        .context("extends: `file` needs a `service`")?
        .to_owned();
    let text = std::fs::read_to_string(&path)
        .with_context(|| format!("extends: reading {}", path.display()))?;
    let text = crate::stack::Stack::interpolate_as(&text, vars, true)
        .with_context(|| format!("in {}", path.display()))?;
    let mut other = load(&text).with_context(|| format!("in {}", path.display()))?;
    let mut base = other
        .pointer_mut(&format!("/services/{name}"))
        .map(J::take)
        .with_context(|| format!("extends: {} has no service {name:?}", path.display()))?;
    if base.pointer("/extends").is_some_and(J::is_string) {
        bail!(
            "extends: {name:?} in {} extends in turn, by name only",
            path.display()
        );
    }
    let there = path.parent().unwrap_or(dir);
    base = extend_from_files(base, there, vars, depth + 1)?;
    svc.as_object_mut()
        .context("is not a mapping")?
        .remove("extends");
    overlay(&mut base, svc);
    Ok(base)
}

/// Compose's rule for `extends`: mappings merge, lists append, and
/// anything else is replaced.
fn overlay(base: &mut J, over: J) {
    match (base, over) {
        (J::Object(b), J::Object(o)) => {
            for (k, v) in o {
                overlay(b.entry(k).or_insert(J::Null), v);
            }
        }
        (J::Array(b), J::Array(o)) => b.extend(o),
        (b, o) => *b = o,
    }
}

fn service(svc: &J, dir: &Path, notes: &mut Vec<String>) -> Result<Table> {
    let svc = svc.as_object().context("is not a mapping")?;
    let mut t = Table::new();
    let mut command: Option<Vec<String>> = None;
    let mut entry_rest: Vec<String> = Vec::new();
    // `environment` wins over `env_file`, whichever is written first.
    let mut env_file: Vec<(String, String)> = Vec::new();
    let mut env: Vec<String> = Vec::new();

    for (k, v) in svc {
        let k = k.as_str();
        let put_str = |t: &mut Table, key: &str| -> Result<()> {
            t.insert(
                key.into(),
                T::String(scalar(v).with_context(|| k.to_owned())?),
            );
            Ok(())
        };
        match k {
            "image" | "restart" | "user" | "platform" | "pid" | "ipc" | "shm_size" => {
                put_str(&mut t, k)?
            }
            "sysctls" => {
                t.insert(k.into(), pairs(v).context("sysctls")?);
            }
            // `nofile: 1024`, or `nofile: {soft: 1024, hard: 2048}`.
            "ulimits" => {
                let J::Object(o) = v else {
                    bail!("ulimits is a mapping of a limit to a number, or to soft and hard");
                };
                let list = o
                    .iter()
                    .map(|(name, lim)| {
                        let value = match lim {
                            J::Object(l) => {
                                let part = |k: &str| {
                                    l.get(k).map(scalar).transpose()?.with_context(|| {
                                        format!("ulimits.{name} needs both soft and hard")
                                    })
                                };
                                format!("{}:{}", part("soft")?, part("hard")?)
                            }
                            other => scalar(other)?,
                        };
                        Ok(T::String(format!("{name}={value}")))
                    })
                    .collect::<Result<_>>()?;
                t.insert(k.into(), T::Array(list));
            }
            "working_dir" => put_str(&mut t, "workdir")?,
            "mem_limit" => put_str(&mut t, "memory")?,
            "network_mode" => put_str(&mut t, "network")?,
            "cpus" => {
                t.insert("cpus".into(), T::Float(number(v).context("cpus")?));
            }
            "pids_limit" => {
                t.insert(
                    "pids_limit".into(),
                    T::Integer(number(v).context(k.to_owned())? as i64),
                );
            }
            "privileged" | "read_only" => {
                t.insert(
                    k.into(),
                    T::Boolean(v.as_bool().with_context(|| k.to_owned())?),
                );
            }
            "profiles" | "devices" | "cap_add" | "cap_drop" | "security_opt" => {
                t.insert(k.into(), strings(v).with_context(|| k.to_owned())?);
            }
            "build" => {
                let context = match v {
                    J::String(s) => s.clone(),
                    J::Object(o) => {
                        for (bk, bv) in o {
                            match bk.as_str() {
                                "context" => {}
                                "dockerfile" => {
                                    t.insert("dockerfile".into(), T::String(scalar(bv)?));
                                }
                                "target" => {
                                    t.insert("target".into(), T::String(scalar(bv)?));
                                }
                                "args" => {
                                    t.insert("build_args".into(), pairs(bv).context("build.args")?);
                                }
                                other => bail!("build.{other} is not supported"),
                            }
                        }
                        o.get("context")
                            .and_then(J::as_str)
                            .unwrap_or(".")
                            .to_owned()
                    }
                    _ => bail!("build is neither a path nor a mapping"),
                };
                t.insert("build".into(), T::String(context));
            }
            "command" => command = Some(words_or_list(v).context("command")?),
            "entrypoint" => {
                let mut e = words_or_list(v).context("entrypoint")?;
                if e.is_empty() {
                    t.insert("entrypoint".into(), T::String(String::new()));
                } else {
                    t.insert("entrypoint".into(), T::String(e.remove(0)));
                    entry_rest = e;
                }
            }
            "environment" => match pairs(v).context("environment")? {
                T::Array(a) => {
                    env.extend(a.into_iter().filter_map(|x| x.as_str().map(str::to_owned)))
                }
                _ => unreachable!("pairs returns an array"),
            },
            "env_file" => {
                let files = match v {
                    J::String(s) => vec![s.clone()],
                    J::Array(a) => a.iter().map(scalar).collect::<Result<_>>()?,
                    _ => bail!("env_file is neither a path nor a list of paths"),
                };
                for f in files {
                    let p = dir.join(&f);
                    let text = std::fs::read_to_string(&p)
                        .with_context(|| format!("env_file {}", p.display()))?;
                    env_file.extend(
                        crate::stack::Stack::parse_dotenv(&text)
                            .with_context(|| format!("env_file {}", p.display()))?,
                    );
                }
            }
            "ports" => {
                let mut out = Vec::new();
                for p in v.as_array().context("ports is not a list")? {
                    out.push(T::String(match p {
                        J::Object(o) => {
                            let target = o.get("target").map(scalar).transpose()?;
                            let target = target.context("a port needs a `target`")?;
                            let proto = match o.get("protocol").and_then(J::as_str) {
                                Some(p) if p != "tcp" => format!("/{p}"),
                                _ => String::new(),
                            };
                            let host = match o.get("host_ip").and_then(J::as_str) {
                                Some(ip) => format!("{ip}:"),
                                None => String::new(),
                            };
                            let published = match o.get("published") {
                                Some(p) => scalar(p)?,
                                None => target.clone(),
                            };
                            format!("{host}{published}:{target}{proto}")
                        }
                        other => scalar(other).context("ports")?,
                    }));
                }
                t.insert("ports".into(), T::Array(out));
            }
            "volumes" => {
                let mut out = Vec::new();
                for m in v.as_array().context("volumes is not a list")? {
                    out.push(T::String(volume(m, dir).context("volumes")?));
                }
                t.insert("volumes".into(), T::Array(out));
            }
            "depends_on" => match v {
                J::Array(_) => {
                    t.insert(k.into(), strings(v)?);
                }
                J::Object(o) => {
                    let mut d = Table::new();
                    for (dep, c) in o {
                        let cond = c.get("condition").and_then(J::as_str);
                        d.insert(
                            dep.clone(),
                            T::String(
                                match cond.unwrap_or("service_started") {
                                    "service_started" => "started",
                                    "service_healthy" => "healthy",
                                    "service_completed_successfully" => "completed",
                                    other => bail!(
                                        "depends_on.{dep}: the condition {other:?} is not supported"
                                    ),
                                }
                                .into(),
                            ),
                        );
                    }
                    t.insert(k.into(), T::Table(d));
                }
                _ => bail!("depends_on is neither a list nor a mapping"),
            },
            "healthcheck" => {
                let o = v.as_object().context("healthcheck is not a mapping")?;
                if o.get("disable") == Some(&J::Bool(true)) {
                    continue;
                }
                let mut h = Table::new();
                for (hk, hv) in o {
                    match hk.as_str() {
                        "test" => {
                            let test = match hv {
                                J::String(s) => shell(s),
                                J::Array(_) => {
                                    let mut a = match strings(hv)? {
                                        T::Array(a) => a,
                                        _ => unreachable!("strings returns an array"),
                                    };
                                    let kind = if a.is_empty() {
                                        None
                                    } else {
                                        Some(a.remove(0))
                                    };
                                    match kind.as_ref().and_then(T::as_str) {
                                        Some("CMD") => T::Array(a),
                                        Some("CMD-SHELL") => {
                                            shell(a.first().and_then(T::as_str).unwrap_or(""))
                                        }
                                        Some("NONE") => continue,
                                        _ => bail!(
                                            "healthcheck.test must start with CMD, CMD-SHELL or NONE"
                                        ),
                                    }
                                }
                                _ => bail!("healthcheck.test is neither a string nor a list"),
                            };
                            h.insert("test".into(), test);
                        }
                        "interval" | "start_period" => {
                            let s = seconds(&scalar(hv)?).with_context(|| hk.clone())?;
                            // A stack's interval is at least a second.
                            let s = if hk == "interval" { s.max(1) } else { s };
                            h.insert(hk.clone(), T::Integer(s));
                        }
                        "retries" => {
                            h.insert(hk.clone(), T::Integer(number(hv)? as i64));
                        }
                        // One attempt here is one `exec`, which has its
                        // own bound; there is no separate knob.
                        "timeout" | "start_interval" | "disable" => {}
                        other => bail!("healthcheck.{other} is not supported"),
                    }
                }
                if h.contains_key("test") {
                    t.insert(k.into(), T::Table(h));
                }
            }
            "tmpfs" => {
                let list = match v {
                    J::String(s) => T::Array(vec![T::String(s.clone())]),
                    other => strings(other).context("tmpfs")?,
                };
                t.insert(k.into(), list);
            }
            // `name:address`, `name=address`, or a mapping of one to the
            // other.
            "extra_hosts" => {
                let hosts: Vec<String> = match v {
                    J::Object(o) => o
                        .iter()
                        .map(|(h, ip)| Ok(format!("{h}:{}", scalar(ip)?)))
                        .collect::<Result<_>>()?,
                    J::Array(a) => a
                        .iter()
                        .map(|e| scalar(e).map(|e| e.replacen('=', ":", 1)))
                        .collect::<Result<_>>()?,
                    _ => bail!("extra_hosts is neither a list nor a mapping"),
                };
                t.insert(
                    k.into(),
                    T::Array(hosts.into_iter().map(T::String).collect()),
                );
            }
            "configs" | "secrets" => {
                t.insert(
                    k.into(),
                    strings(v).with_context(|| format!("{k}: only the short form (a name)"))?,
                );
            }
            "extends" => {
                let s = match v {
                    J::String(s) => s.clone(),
                    J::Object(o) if o.len() == 1 && o.contains_key("service") => {
                        scalar(&o["service"])?
                    }
                    _ => bail!("extends: only a service of this file, not `file:`"),
                };
                t.insert(k.into(), T::String(s));
            }
            "deploy" => {
                for (dk, dv) in v.as_object().context("deploy is not a mapping")? {
                    match dk.as_str() {
                        "replicas" => {
                            t.insert("replicas".into(), T::Integer(number(dv)? as i64));
                        }
                        "resources" => {
                            if let Some(l) = dv.get("limits") {
                                if let Some(c) = l.get("cpus") {
                                    t.insert("cpus".into(), T::Float(number(c)?));
                                }
                                if let Some(m) = l.get("memory") {
                                    t.insert("memory".into(), T::String(scalar(m)?));
                                }
                                if let Some(p) = l.get("pids") {
                                    t.insert("pids_limit".into(), T::Integer(number(p)? as i64));
                                }
                            }
                        }
                        "placement" => {
                            if let Some(c) = dv.get("constraints") {
                                t.insert("placement".into(), strings(c)?);
                            }
                        }
                        other => notes.push(format!("deploy.{other}: ignored")),
                    }
                }
            }
            "develop" => {
                let mut out = Vec::new();
                for w in v.get("watch").and_then(J::as_array).into_iter().flatten() {
                    let mut wt = Table::new();
                    for key in ["path", "action", "target"] {
                        if let Some(x) = w.get(key) {
                            wt.insert(key.into(), T::String(scalar(x)?));
                        }
                    }
                    out.push(T::Table(wt));
                }
                t.insert("watch".into(), T::Array(out));
            }
            x if IGNORED.contains(&x) => notes.push(format!("{x}: ignored")),
            x if x.starts_with("x-") => {}
            other => bail!(
                "the key {other:?} is not supported. Remove it, or write the stack \
                 as a rustainer.toml."
            ),
        }
    }

    let mut merged: Vec<String> = Vec::new();
    for e in env_file
        .into_iter()
        .map(|(k, v)| format!("{k}={v}"))
        .chain(env)
    {
        let key = e.split('=').next().unwrap_or_default().to_owned();
        merged.retain(|m| m.split('=').next() != Some(key.as_str()));
        merged.push(e);
    }
    if !merged.is_empty() {
        t.insert(
            "env".into(),
            T::Array(merged.into_iter().map(T::String).collect()),
        );
    }
    // Compose's entrypoint is a list and `run --entrypoint` takes one
    // word: the rest of the list is the front of the command. And as
    // in Docker, a new entrypoint without a command means no command.
    if !entry_rest.is_empty() || command.is_some() {
        entry_rest.extend(command.unwrap_or_default());
        t.insert(
            "command".into(),
            T::Array(entry_rest.into_iter().map(T::String).collect()),
        );
    }
    Ok(t)
}

/// `src:dst[:opts]` or the long form, as `-v` takes it.
fn volume(m: &J, dir: &Path) -> Result<String> {
    let (src, dst, ro) = match m {
        J::String(s) => {
            let p: Vec<&str> = s.split(':').collect();
            match p.as_slice() {
                // An anonymous volume; `-v` says what it thinks of those.
                [_] => return Ok(s.clone()),
                [a, b] => (a.to_string(), b.to_string(), false),
                // `z`, `cached` and the rest mean nothing here; `ro` does.
                [a, b, o] => (
                    a.to_string(),
                    b.to_string(),
                    o.split(',').any(|x| x == "ro"),
                ),
                _ => bail!("{s:?} is not `source:target[:options]`"),
            }
        }
        J::Object(o) => {
            match o.get("type").and_then(J::as_str) {
                Some("bind" | "volume") => {}
                other => bail!("a volume of type {other:?} is not supported"),
            }
            (
                o.get("source")
                    .map(scalar)
                    .transpose()?
                    .context("a volume needs a `source`")?,
                o.get("target")
                    .map(scalar)
                    .transpose()?
                    .context("a volume needs a `target`")?,
                o.get("read_only") == Some(&J::Bool(true)),
            )
        }
        _ => bail!("a volume is neither a string nor a mapping"),
    };
    // Relative to the Compose file, not to wherever `up` was typed.
    let src = if src.starts_with('.') {
        dir.join(&src).display().to_string()
    } else {
        src
    };
    Ok(format!("{src}:{dst}{}", if ro { ":ro" } else { "" }))
}

fn scalar(v: &J) -> Result<String> {
    Ok(match v {
        J::String(s) => s.clone(),
        J::Number(n) => n.to_string(),
        J::Bool(b) => b.to_string(),
        other => bail!("expected a plain value, found {other}"),
    })
}

fn number(v: &J) -> Result<f64> {
    match v {
        J::Number(n) => n.as_f64(),
        J::String(s) => s.parse().ok(),
        _ => None,
    }
    .with_context(|| format!("{v} is not a number"))
}

fn strings(v: &J) -> Result<T> {
    Ok(T::Array(
        v.as_array()
            .context("expected a list")?
            .iter()
            .map(|x| scalar(x).map(T::String))
            .collect::<Result<_>>()?,
    ))
}

/// `KEY=value` strings from either shape Compose allows: a list of
/// them, or a mapping. A key with no value is passed on bare, which
/// means "the value I have".
fn pairs(v: &J) -> Result<T> {
    Ok(T::Array(match v {
        J::Array(a) => a
            .iter()
            .map(|e| scalar(e).map(T::String))
            .collect::<Result<_>>()?,
        J::Object(o) => o
            .iter()
            .map(|(k, val)| {
                Ok(T::String(if val.is_null() {
                    k.clone()
                } else {
                    format!("{k}={}", scalar(val)?)
                }))
            })
            .collect::<Result<_>>()?,
        _ => bail!("neither a list nor a mapping"),
    }))
}

fn shell(s: &str) -> T {
    T::Array(vec!["/bin/sh".into(), "-c".into(), T::String(s.to_owned())])
}

/// A command written as a string is split the way a shell would split
/// it, quotes and all, and run without one - as Compose does.
fn words_or_list(v: &J) -> Result<Vec<String>> {
    match v {
        J::Null => Ok(Vec::new()),
        J::Array(a) => a.iter().map(scalar).collect(),
        J::String(s) => words(s),
        _ => bail!("neither a string nor a list"),
    }
}

pub(crate) fn words(s: &str) -> Result<Vec<String>> {
    let mut out = Vec::new();
    let mut cur: Option<String> = None;
    let mut quote: Option<char> = None;
    let mut chars = s.chars();
    while let Some(c) = chars.next() {
        match (quote, c) {
            (Some(q), c) if c == q => quote = None,
            (Some('"') | None, '\\') => cur
                .get_or_insert_default()
                .push(chars.next().context("a trailing backslash")?),
            (Some(_), c) => cur.get_or_insert_default().push(c),
            (None, '\'' | '"') => {
                quote = Some(c);
                cur.get_or_insert_default();
            }
            (None, c) if c.is_whitespace() => out.extend(cur.take()),
            (None, c) => cur.get_or_insert_default().push(c),
        }
    }
    if quote.is_some() {
        bail!("{s:?} has a quote that is never closed");
    }
    out.extend(cur);
    Ok(out)
}

/// Compose durations: `30s`, `1m30s`, `500ms`, or a bare number of
/// seconds. Rounded up, because a stack counts whole seconds.
fn seconds(s: &str) -> Result<i64> {
    if let Ok(n) = s.parse::<f64>() {
        return Ok(n.ceil() as i64);
    }
    let mut total = 0f64;
    let mut num = String::new();
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c.is_ascii_digit() || c == '.' {
            num.push(c);
            continue;
        }
        let n: f64 = num
            .parse()
            .with_context(|| format!("{s:?} is not a duration"))?;
        num.clear();
        total += n * match c {
            'h' => 3600.0,
            'm' if chars.peek() == Some(&'s') => {
                chars.next();
                0.001
            }
            'm' => 60.0,
            's' => 1.0,
            _ => bail!("{s:?} is not a duration"),
        };
    }
    if !num.is_empty() {
        bail!("{s:?} is not a duration");
    }
    Ok(total.ceil() as i64)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_compose_file_reads_as_the_stack_it_describes() {
        let yaml = r#"
name: shop
x-common: &common
  restart: unless-stopped
services:
  db:
    <<: *common
    image: postgres:16-alpine
    container_name: shop-db
    environment:
      POSTGRES_PASSWORD: secret
      PORT: 5432
    healthcheck:
      test: ["CMD-SHELL", "pg_isready -U postgres"]
      interval: 1m30s
      timeout: 5s
      retries: 5
    volumes:
      - pgdata:/var/lib/postgresql/data
      - ./init:/docker-entrypoint-initdb.d:ro,z
  web:
    image: nginx
    command: sh -c "echo 'a b' && nginx"
    entrypoint: ["/tini", "--"]
    ports:
      - "8080:80"
      - target: 443
        published: 8443
    tmpfs: /run
    shm_size: 256m
    ulimits:
      nproc: 512
      nofile:
        soft: 1024
        hard: 2048
    sysctls:
      net.core.somaxconn: 1024
    extra_hosts:
      - "host.docker.internal=10.0.0.1"
    depends_on:
      db:
        condition: service_healthy
    deploy:
      replicas: 2
      resources:
        limits:
          memory: 256M
volumes:
  pgdata:
"#;
        let (doc, notes) = to_stack(yaml, Path::new("/srv/shop"), &BTreeMap::new()).unwrap();
        assert_eq!(notes, ["container_name: ignored"]);
        let s = &doc["services"];
        assert_eq!(doc["name"].as_str(), Some("shop"));
        assert_eq!(s["db"]["restart"].as_str(), Some("unless-stopped"));
        assert_eq!(
            s["db"]["env"].as_array().unwrap(),
            &[T::from("PORT=5432"), T::from("POSTGRES_PASSWORD=secret")]
        );
        assert_eq!(s["db"]["healthcheck"]["interval"].as_integer(), Some(90));
        assert_eq!(
            s["db"]["healthcheck"]["test"][2].as_str(),
            Some("pg_isready -U postgres")
        );
        assert_eq!(
            s["db"]["volumes"][1].as_str(),
            Some("/srv/shop/./init:/docker-entrypoint-initdb.d:ro")
        );
        assert_eq!(s["web"]["entrypoint"].as_str(), Some("/tini"));
        assert_eq!(
            s["web"]["command"].as_array().unwrap(),
            &["--", "sh", "-c", "echo 'a b' && nginx"].map(T::from)
        );
        assert_eq!(s["web"]["ports"][1].as_str(), Some("8443:443"));
        assert_eq!(s["web"]["depends_on"]["db"].as_str(), Some("healthy"));
        assert_eq!(s["web"]["replicas"].as_integer(), Some(2));
        assert_eq!(s["web"]["tmpfs"][0].as_str(), Some("/run"));
        assert_eq!(s["web"]["shm_size"].as_str(), Some("256m"));
        assert_eq!(
            s["web"]["ulimits"].as_array().unwrap(),
            &[T::from("nofile=1024:2048"), T::from("nproc=512")]
        );
        assert_eq!(
            s["web"]["sysctls"][0].as_str(),
            Some("net.core.somaxconn=1024")
        );
        assert_eq!(
            s["web"]["extra_hosts"][0].as_str(),
            Some("host.docker.internal:10.0.0.1")
        );
        assert_eq!(s["web"]["memory"].as_str(), Some("256M"));
    }

    #[test]
    fn a_key_that_cannot_be_honoured_is_named() {
        let e = to_stack(
            "services:\n  a:\n    image: x\n    userns_mode: host\n",
            Path::new("/x"),
            &BTreeMap::new(),
        )
        .unwrap_err();
        assert!(format!("{e:#}").contains("\"userns_mode\""), "{e:#}");
    }

    #[test]
    fn a_service_extends_one_in_another_file_and_tags_are_read_through() {
        let dir = std::env::temp_dir().join(format!("rustainer-compose-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("base.yml"),
            "services:\n  db:\n    image: postgres:${TAG:?say which}\n    ports: [\"5432:5432\"]\n    environment: {A: 1}\n",
        )
        .unwrap();
        let vars = BTreeMap::from([("TAG".to_owned(), "16".to_owned())]);
        let yaml = "services:\n  db:\n    extends: {file: base.yml, service: db}\n    environment: {B: 2}\n    ports: !override\n      - \"5433:5432\"\n";
        let (doc, _) = to_stack(yaml, &dir, &vars).unwrap();
        std::fs::remove_dir_all(&dir).unwrap();
        let db = &doc["services"]["db"];
        assert_eq!(db["image"].as_str(), Some("postgres:16"));
        assert_eq!(db["env"].as_array().unwrap(), &["A=1", "B=2"].map(T::from));
        // Lists append under `extends`, tag or no tag: the tag speaks to
        // merging `-f` files, where a list here replaces anyway.
        assert_eq!(db["ports"].as_array().unwrap().len(), 2);
        assert!(db.get("extends").is_none());
    }

    #[test]
    fn the_shells_substitution_forms() {
        let vars = BTreeMap::from([
            ("SET".to_owned(), "v".to_owned()),
            ("EMPTY".to_owned(), String::new()),
        ]);
        let i = |t: &str| crate::stack::Stack::interpolate(t, &vars).map_err(|e| e.to_string());
        assert_eq!(
            i("${SET-d} ${NOPE-d} ${EMPTY-d} ${EMPTY:-d}").unwrap(),
            "v d  d"
        );
        assert_eq!(i("${SET:+yes}${NOPE:+yes}${SET?msg}").unwrap(), "yesv");
        assert!(i("${NOPE:?give it}").unwrap_err().contains("give it"));
        assert!(i("${EMPTY:?give it}").is_err());
        assert_eq!(i("${NOPE-http://x:9090}").unwrap(), "http://x:9090");
        let c = |t: &str| crate::stack::Stack::interpolate_as(t, &vars, true).unwrap();
        assert_eq!(c("$SET/$NOPE. $$SET $1 ${SET}"), "v/. $SET $1 v");
        assert_eq!(i("$SET").unwrap(), "$SET");
    }
}
