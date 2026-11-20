//! A local HTTP API over the engine, and the page that uses it.
//!
//! # Why this process cannot start a container
//!
//! It is multi-threaded — one thread per connection — and **a container is
//! created by a process that has never had a second thread**. That is not a
//! style rule: after a raw `clone`, the child inherits every lock in the
//! address space in whatever state it was in, and glibc has had no chance
//! to quiesce any of them (RND.md §15.4.1). [`rustainer_core::spawn`]
//! checks the thread count and refuses.
//!
//! So this server never calls into the runtime to *change* anything. It
//! executes the CLI — `/proc/self/exe run …` — and lets a fresh,
//! single-threaded process do it, exactly as the CLI itself does for the
//! supervisor. The rule that made `clone3` safe at stage 1 is what decides
//! the shape of the API at stage 17, which is the kind of thing a
//! "just add an HTTP server" plan does not survive contact with.
//!
//! Reads are different and are done in-process: listing containers,
//! tailing logs and sampling cgroup counters are all file reads, and none
//! of them clones.
//!
//! # What this is not
//!
//! Not a daemon that owns containers. Containers outlive it, because they
//! are owned by their own supervisors; stop this server and everything
//! keeps running. It is a *view*, and a thin one — which is also why
//! there is no state in it to corrupt, and why two of them can run at once.
//!
//! # Security posture, stated rather than implied
//!
//! There is **no authentication**. Anything that can reach this port can
//! start a container, and a container is a path to root on the host. It
//! binds `127.0.0.1` by default for that reason, and an address that is
//! not loopback produces a warning naming exactly what is being exposed.

pub mod http;

use std::io::Read;
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};

use rustainer_core::state::{Paths, State, Status};
use rustainer_core::supervise::Config;
use rustainer_image::Store;
use serde_json::{Value, json};

/// The page, compiled in. One file, no build step, no asset pipeline: a
/// container engine that needs `npm install` to show a list of containers
/// has acquired a dependency it cannot explain.
const INDEX_HTML: &str = include_str!("../ui/index.html");

pub struct Server {
    paths: Paths,
    /// The binary to re-execute for anything that mutates. `/proc/self/exe`
    /// rather than `current_exe()`: the CLI seals itself into a memfd at
    /// startup, so `current_exe()` resolves to `/memfd:rustainer (deleted)`
    /// — a path that cannot be spawned — while the magic symlink still
    /// re-executes the sealed image.
    exe: PathBuf,
    root_arg: Option<String>,
    /// Live terminal sessions, by id. The only state this server has,
    /// and it is deliberately not persisted: a session is a live pty
    /// and a live process, and neither survives the server anyway.
    terms: std::sync::Mutex<std::collections::HashMap<String, std::sync::Arc<Term>>>,
}

impl Server {
    pub fn new(paths: Paths, root_arg: Option<String>) -> Self {
        Self {
            paths,
            exe: PathBuf::from("/proc/self/exe"),
            root_arg,
            terms: std::sync::Mutex::new(std::collections::HashMap::new()),
        }
    }

    /// Serve until killed. One thread per connection.
    pub fn serve(self, listener: TcpListener) -> std::io::Result<()> {
        let me = std::sync::Arc::new(self);
        for conn in listener.incoming() {
            let Ok(stream) = conn else { continue };
            let me = std::sync::Arc::clone(&me);
            // Detached: a connection that hangs must not hold up the
            // accept loop, and there is no per-connection state to join.
            std::thread::spawn(move || {
                let mut stream = stream;
                if let Err(e) = me.handle(&mut stream) {
                    // The peer is usually just gone. Nothing to report to.
                    let _ = e;
                }
            });
        }
        Ok(())
    }

    fn handle(&self, stream: &mut TcpStream) -> std::io::Result<()> {
        let req = match http::read_request(stream) {
            Ok(Some(r)) => r,
            Ok(None) => return Ok(()),
            Err(http::Error::BodyTooLarge) => return http::error(stream, 413, "body too large"),
            Err(e) => return http::error(stream, 400, &e.to_string()),
        };
        self.route(stream, &req)
    }

    fn route(&self, stream: &mut TcpStream, req: &http::Request) -> std::io::Result<()> {
        let path = req.path.as_str();
        let method = req.method.as_str();

        if path == "/" || path == "/index.html" {
            return http::respond(
                stream,
                200,
                "text/html; charset=utf-8",
                INDEX_HTML.as_bytes(),
            );
        }

        match (method, path) {
            ("GET", "/api/system") => return http::json(stream, 200, &self.system()),
            ("GET", "/api/containers") => return http::json(stream, 200, &self.containers()),
            ("POST", "/api/containers") => return self.create(stream, req),
            ("GET", "/api/images") => return http::json(stream, 200, &self.images()),
            ("POST", "/api/images/pull") => return self.pull(stream, req),
            // Nodes and pods together, as kubectl prints them: the page
            // reads the cluster through the same tool a person would.
            ("GET", "/api/kube") => {
                return self.run_cli(
                    stream,
                    &["kube", "kubectl", "get", "nodes,pods", "-A", "-o", "json"],
                );
            }
            ("POST", "/api/kube/up") => return self.run_cli(stream, &["kube", "up"]),
            ("POST", "/api/kube/down") => return self.run_cli(stream, &["kube", "down"]),
            ("GET", "/api/models") => return self.run_cli(stream, &["model", "ls"]),
            (
                "POST",
                "/api/models/pull" | "/api/models/run" | "/api/models/stop" | "/api/models/rm"
                | "/api/models/ask",
            ) => {
                let spec: Value = serde_json::from_slice(&req.body).unwrap_or(Value::Null);
                // A reference, not a flag: it is placed on a command line.
                let Some(model) = spec["model"].as_str().filter(|m| {
                    !m.is_empty()
                        && !m.starts_with('-')
                        && m.chars()
                            .all(|c| c.is_ascii_alphanumeric() || "._-/:".contains(c))
                }) else {
                    return http::error(stream, 400, "a model reference is required");
                };
                let verb = req.path.rsplit('/').next().unwrap_or_default();
                return match (verb, spec["prompt"].as_str()) {
                    // `--` so a prompt that starts with a dash stays a prompt.
                    ("ask", Some(prompt)) => {
                        self.run_cli(stream, &["model", "ask", model, "--", prompt])
                    }
                    ("ask", None) => http::error(stream, 400, "a prompt is required"),
                    _ => self.run_cli(stream, &["model", verb, model]),
                };
            }
            ("GET", "/api/extensions") => {
                return self.run_cli(stream, &["extension", "ls", "--json"]);
            }
            ("GET", "/api/mcp") => return self.run_cli(stream, &["mcp", "ls"]),
            ("GET", "/api/mcp/config") => {
                let server = req.param("server").unwrap_or_default();
                let network = req.param("network").unwrap_or_else(|| "bridge".into());
                // Names, not flags: both are placed on a command line.
                let plain = |s: &str| {
                    !s.is_empty()
                        && !s.starts_with('-')
                        && s.chars()
                            .all(|c| c.is_ascii_alphanumeric() || "._-/:".contains(c))
                };
                if !plain(&server) || !plain(&network) {
                    return http::error(stream, 400, "not a server name");
                }
                return self.run_cli(stream, &["mcp", "config", &server, "--network", &network]);
            }
            ("GET", "/api/registry/repos") => {
                // No namespace means "mine": the CLI takes it from the
                // credentials this server was started with.
                let ns = req.param("namespace").unwrap_or_default();
                if ns.is_empty() {
                    return self.run_cli(stream, &["repos"]);
                }
                if !is_safe_id(&ns) {
                    return http::error(stream, 400, "not a namespace");
                }
                return self.run_cli(stream, &["repos", &ns]);
            }
            ("GET", "/api/registry/tags") => {
                let Some(repo) = req.param("repository").filter(|s| !s.is_empty()) else {
                    return http::error(stream, 400, "a repository is required");
                };
                // A leading `-` would be read by the CLI as a flag.
                if repo.starts_with('-') {
                    return http::error(stream, 400, "not a repository");
                }
                return self.run_cli(stream, &["tags", &repo]);
            }
            ("GET", "/api/volumes") => return http::json(stream, 200, &self.volumes()),
            ("POST", "/api/volumes") => {
                let spec: Value = serde_json::from_slice(&req.body).unwrap_or(Value::Null);
                let name = spec["name"].as_str().unwrap_or("");
                if !is_safe_id(name) {
                    return http::error(stream, 400, "not a usable volume name");
                }
                return self.run_cli(stream, &["volume", "create", name]);
            }
            ("POST", "/api/images/scan") => {
                let spec: Value = serde_json::from_slice(&req.body).unwrap_or(Value::Null);
                let Some(reference) = spec["reference"]
                    .as_str()
                    .filter(|s| !s.is_empty() && !s.starts_with('-'))
                else {
                    return http::error(stream, 400, "a reference is required");
                };
                return self.run_cli(stream, &["scan", "--json", reference]);
            }
            ("POST", "/api/images/push") => {
                let spec: Value = serde_json::from_slice(&req.body).unwrap_or(Value::Null);
                let Some(reference) = spec["reference"].as_str().filter(|s| !s.is_empty()) else {
                    return http::error(stream, 400, "a reference is required");
                };
                // Credentials are whatever RUSTAINER_REGISTRY_AUTH this
                // server was started with. None ever arrive in a request:
                // a password in a JSON body is a password in a browser.
                return match spec["to"].as_str().filter(|s| !s.is_empty()) {
                    Some(to) => self.run_cli(stream, &["push", reference, "--to", to]),
                    None => self.run_cli(stream, &["push", reference]),
                };
            }
            ("GET", "/api/networks") => return http::json(stream, 200, &self.networks()),
            ("POST", "/api/build") => return self.build(stream, req),
            ("GET", "/api/events") => {
                let since = req
                    .param("since")
                    .and_then(|v| v.parse::<u64>().ok())
                    .unwrap_or(0);
                let limit = req
                    .param("limit")
                    .and_then(|v| v.parse::<usize>().ok())
                    .unwrap_or(200)
                    .min(2000);
                return http::json(
                    stream,
                    200,
                    &serde_json::Value::Array(rustainer_core::events::read(
                        &self.paths,
                        since,
                        limit,
                    )),
                );
            }
            _ => {}
        }

        // /api/containers/{id}/{action}
        if let Some(rest) = path.strip_prefix("/api/containers/") {
            let (id, action) = match rest.split_once('/') {
                Some((i, a)) => (i, a),
                None => (rest, ""),
            };
            let id = http::percent_decode(id);
            if !is_safe_id(&id) {
                return http::error(stream, 400, "not a usable container id");
            }
            return match (method, action) {
                ("GET", "logs") => self.logs(stream, &id, req),
                ("GET", "stats") => http::json(stream, 200, &self.stats(&id)),
                ("GET", "files") => self.container_files(stream, &id, req),
                ("PUT", "files") => self.put_file(stream, &[&id], req),
                ("POST", "stop") => self.run_cli(stream, &["stop", "-t", "5", &id]),
                ("POST", "kill") => self.run_cli(stream, &["stop", "-t", "0", &id]),
                ("POST", "start") => self.run_cli(stream, &["start", &id]),
                ("POST", "restart") => self.run_cli(stream, &["restart", "-t", "5", &id]),
                ("POST", "exec") => self.exec(stream, &id, req),
                ("POST", "term") => self.term_open(stream, &id, req),
                ("DELETE", "") => self.run_cli(stream, &["rm", "--force", &id]),
                ("GET", "") => http::json(stream, 200, &self.one(&id)),
                _ => http::error(stream, 405, "no such container action"),
            };
        }

        if let Some(rest) = path.strip_prefix("/api/extensions/") {
            let (name, action) = rest.split_once('/').unwrap_or((rest, ""));
            let name = http::percent_decode(name);
            if !is_safe_id(&name) {
                return http::error(stream, 400, "not an extension name");
            }
            return match (method, action) {
                ("POST", "enable") => self.run_cli(stream, &["extension", "enable", &name]),
                ("POST", "disable") => self.run_cli(stream, &["extension", "disable", &name]),
                ("DELETE", "") => self.run_cli(stream, &["extension", "rm", &name]),
                _ => http::error(stream, 405, "no such extension action"),
            };
        }

        if let Some(rest) = path.strip_prefix("/api/term/") {
            let (session, action) = match rest.split_once('/') {
                Some((s, a)) => (s, a),
                None => (rest, ""),
            };
            if !is_safe_id(session) {
                return http::error(stream, 400, "not a session id");
            }
            return match (method, action) {
                ("GET", "out") => self.term_read(stream, session, req),
                ("POST", "in") => self.term_write(stream, session, req),
                ("DELETE", "") => self.term_close(stream, session),
                _ => http::error(stream, 405, "no such terminal action"),
            };
        }

        if let Some(name) = path.strip_prefix("/api/networks/") {
            let name = http::percent_decode(name);
            if method != "DELETE" {
                return http::error(stream, 405, "only DELETE");
            }
            return self.run_cli(stream, &["network", "rm", &name]);
        }

        if let Some(name) = path.strip_prefix("/api/volumes/") {
            if let Some((name, action)) = name.split_once('/') {
                let name = http::percent_decode(name);
                if !is_safe_id(&name) {
                    return http::error(stream, 400, "not a usable volume name");
                }
                let root = Store::default_root().join("volumes").join(&name);
                return match (method, action) {
                    ("GET", "files") => browse_reply(stream, &root, req),
                    ("PUT", "files") => self.put_file(stream, &["--volume", &name], req),
                    ("GET", "export") if root.is_dir() => self.export_volume(stream, &name),
                    ("POST", "empty") => self.run_cli(stream, &["volume", "empty", &name]),
                    ("POST", "import") => {
                        self.run_cli_with(stream, &["volume", "import", &name], &req.body)
                    }
                    ("POST", "clone") => {
                        let spec: Value = serde_json::from_slice(&req.body).unwrap_or(Value::Null);
                        let to = spec["to"].as_str().unwrap_or("");
                        if !is_safe_id(to) {
                            return http::error(stream, 400, "not a usable volume name");
                        }
                        self.run_cli(stream, &["volume", "clone", &name, to])
                    }
                    _ => http::error(stream, 404, "no such volume action"),
                };
            }
            let name = http::percent_decode(name);
            if method != "DELETE" {
                return http::error(stream, 405, "only DELETE");
            }
            return self.run_cli(stream, &["volume", "rm", &name]);
        }

        if let Some(reference) = path.strip_prefix("/api/images/") {
            let reference = http::percent_decode(reference);
            if method != "DELETE" {
                return http::error(stream, 405, "only DELETE");
            }
            return self.run_cli(stream, &["rmi", &reference]);
        }

        http::error(stream, 404, "no such endpoint")
    }

    // --- reads ----------------------------------------------------------

    fn system(&self) -> Value {
        let uid = rustix::process::geteuid().as_raw();
        json!({
            "version": env!("CARGO_PKG_VERSION"),
            "kernel": read_trimmed("/proc/sys/kernel/osrelease"),
            "arch": std::env::consts::ARCH,
            "uid": uid,
            "rootless": uid != 0,
            "cgroup": format!("{:?}", rustainer_core::cgroup::detect()),
            "state_root": self.paths.containers().display().to_string(),
            "image_root": Store::default_root().display().to_string(),
        })
    }

    fn containers(&self) -> Value {
        let mut ids = self.paths.list().unwrap_or_default();
        ids.sort();
        Value::Array(ids.iter().map(|id| self.describe(id)).collect())
    }

    fn one(&self, id: &str) -> Value {
        self.describe(id)
    }

    fn describe(&self, id: &str) -> Value {
        let Ok(st) = State::load(&self.paths, id) else {
            return json!({ "id": id, "status": "unknown" });
        };
        let cfg = Config::load(&self.paths, id).ok();
        let observed = st.observed_status();
        json!({
            "id": id,
            "status": observed.to_string(),
            "pid": match (observed, st.init_pid) {
                (Status::Running | Status::Created, Some(p)) => Value::from(p),
                _ => Value::Null,
            },
            "restarts": st.restarts,
            "restart_policy": cfg.as_ref().map(|c| format!("{:?}", c.restart).to_lowercase()),
            "stack": cfg.as_ref().and_then(|c| c.stack.clone()),
            "service": cfg.as_ref().and_then(|c| c.service.clone()),
            "command": st.command.join(" "),
            "hostname": st.hostname,
            "created_at": st.created_at,
            "started_at": st.started_at,
            "exited_at": st.exited_at,
            "exit_code": st.exit_code,
            "signal": st.signal,
            "oom_killed": st.oom_killed,
            // Written by a watcher process, not by the supervisor, so
            // it is simply absent when nothing is watching - which is
            // the honest answer for a container with no healthcheck.
            "health": read_trimmed(self.paths.dir(id).join("health")),
            "error": st.error,
            "ip": cfg.as_ref().and_then(|c| c.ip.clone()),
            "ports": cfg.as_ref().map(|c| {
                c.ports.iter()
                    .map(|p| json!({ "host": p.host_port, "container": p.container_port }))
                    .collect::<Vec<_>>()
            }),
            "volumes": cfg.as_ref().map(|c| {
                c.volumes.iter()
                    .map(|v| json!({
                        "source": v.source.display().to_string(),
                        "target": v.target,
                        "read_only": v.read_only,
                    }))
                    .collect::<Vec<_>>()
            }),
            "memory": cfg.as_ref().and_then(|c| c.memory),
            "cpus": cfg.as_ref().and_then(|c| c.cpus),
        })
    }

    /// cgroup counters, sampled now. `null` where the host gives us no
    /// cgroup, which is a real and common state and not an error.
    fn stats(&self, id: &str) -> Value {
        let Ok(st) = State::load(&self.paths, id) else {
            return json!({ "error": "no such container" });
        };
        let Some(pid) = st
            .init_pid
            .filter(|_| st.observed_status() == Status::Running)
        else {
            return json!({ "running": false });
        };
        let Some(dir) = cgroup_dir_of(pid) else {
            return json!({ "running": true, "cgroup": false });
        };
        let read = |f: &str| read_trimmed(dir.join(f));
        // usage_usec from cpu.stat, which is cumulative; the page turns
        // consecutive samples into a percentage, because a rate is the
        // only form of this number anybody wants.
        let cpu_usec = read("cpu.stat").and_then(|s| {
            s.lines()
                .find_map(|l| l.strip_prefix("usage_usec "))
                .and_then(|v| v.trim().parse::<u64>().ok())
        });
        // `memory.current` exists only where the memory controller is
        // enabled in the parent's subtree_control, and this engine does
        // not enable a controller it was not asked for - so on a host
        // with no `--memory` anywhere, every container's memory read as
        // "-" while Docker Desktop showed a number for the same
        // container. Summing the RSS of the processes in the cgroup needs
        // no controller and no accounting overhead: `cgroup.procs` is
        // always there, and /proc/<pid>/status always has VmRSS.
        //
        // It is not the same number. It double-counts shared pages
        // between processes and leaves out page cache the cgroup is
        // charged for, so it is reported under its own name and the page
        // says which one it is showing.
        let memory_rss = read("cgroup.procs").map(|procs| {
            procs
                .lines()
                .filter_map(|l| l.trim().parse::<i32>().ok())
                .filter_map(|p| read_trimmed(format!("/proc/{p}/status")))
                .filter_map(|s| {
                    s.lines()
                        .find_map(|l| l.strip_prefix("VmRSS:"))
                        .and_then(|v| v.split_whitespace().next()?.parse::<u64>().ok())
                })
                .sum::<u64>()
                * 1024
        });
        let net = read_trimmed(format!("/proc/{pid}/net/dev")).map(|s| {
            s.lines()
                .filter_map(|l| l.split_once(':'))
                .filter(|(name, _)| name.trim() != "lo")
                .fold((0u64, 0u64), |acc, (_, rest)| {
                    let f: Vec<u64> = rest
                        .split_whitespace()
                        .filter_map(|v| v.parse().ok())
                        .collect();
                    (
                        acc.0 + f.first().copied().unwrap_or(0),
                        acc.1 + f.get(8).copied().unwrap_or(0),
                    )
                })
        });
        // Needs the io controller, which is not always enabled; absent
        // then, and the page shows nothing rather than a zero.
        let io = read("io.stat").map(|s| {
            s.split_whitespace()
                .fold((0u64, 0u64), |acc, kv| match kv.split_once('=') {
                    Some(("rbytes", v)) => (acc.0 + v.parse::<u64>().unwrap_or(0), acc.1),
                    Some(("wbytes", v)) => (acc.0, acc.1 + v.parse::<u64>().unwrap_or(0)),
                    _ => acc,
                })
        });
        json!({
            "running": true,
            "cgroup": true,
            "memory_rss": memory_rss,
            "memory_current": read("memory.current").and_then(|s| s.parse::<u64>().ok()),
            "memory_max": read("memory.max"),
            "memory_peak": read("memory.peak").and_then(|s| s.parse::<u64>().ok()),
            "pids_current": read("pids.current").and_then(|s| s.parse::<u64>().ok()),
            "pids_max": read("pids.max"),
            "cpu_usec": cpu_usec,
            // Cumulative since the container started, like the CPU
            // counter. `/proc/<pid>/net/dev` is that process's network
            // namespace, so this is the container's traffic and not the
            // host's - except with `--network host`, where they are the
            // same thing and the number says so by being large.
            "net_rx": net.map(|n| n.0),
            "net_tx": net.map(|n| n.1),
            "io_read": io.map(|n| n.0),
            "io_write": io.map(|n| n.1),
            "sampled_at_usec": now_usec(),
        })
    }

    fn logs(&self, stream: &mut TcpStream, id: &str, req: &http::Request) -> std::io::Result<()> {
        let tail: usize = req
            .param("tail")
            .and_then(|v| v.parse().ok())
            .unwrap_or(400)
            .min(5000);
        let (out, out_times) = tail_file(&self.paths.stdout_log(id), tail);
        let (err, err_times) = tail_file(&self.paths.stderr_log(id), tail);
        http::json(
            stream,
            200,
            // One time per line, in microseconds, or null where the log
            // predates the index. Separate from the text so a reader
            // that does not want them never has to parse them out.
            &json!({
                "stdout": out, "stderr": err,
                "stdout_times": out_times, "stderr_times": err_times,
            }),
        )
    }

    /// Every container's config, for the two "who is using this" answers.
    fn configs(&self) -> Vec<Config> {
        self.paths
            .list()
            .unwrap_or_default()
            .iter()
            .filter_map(|id| Config::load(&self.paths, id).ok())
            .collect()
    }

    fn images(&self) -> Value {
        let store = Store::new(Store::default_root());
        let list = store.list().unwrap_or_default();
        // A container does not record the reference it was started from,
        // only the layer directories it mounted - and those are named by
        // diff_id, so the set of names identifies the image exactly as
        // well as a reference would, and survives a retag.
        let stacks: Vec<(String, std::collections::BTreeSet<String>)> = self
            .configs()
            .into_iter()
            .map(|c| {
                let names = c
                    .layers
                    .iter()
                    .filter_map(|l| Some(Path::new(l).file_name()?.to_string_lossy().into_owned()))
                    .collect();
                (c.id, names)
            })
            .collect();
        Value::Array(
            list.into_iter()
                .map(|(reference, digest)| {
                    // `reference` is the store's key and is what removal
                    // takes; `name` is what a person typed, where known.
                    let mut row = json!({
                        "reference": reference,
                        "name": store.name_of(&reference),
                        "digest": digest,
                    });
                    let Some((config, manifest)) = store.inspect(&reference) else {
                        return row;
                    };
                    let ids: std::collections::BTreeSet<String> = config
                        .rootfs
                        .diff_ids
                        .iter()
                        .map(|d| d.trim_start_matches("sha256:").to_owned())
                        .collect();
                    let section = config.config.unwrap_or_default();
                    row["created"] = json!(config.created);
                    row["layers"] = json!(ids.len());
                    row["history"] = json!(
                        config
                            .history
                            .iter()
                            .filter_map(|h| h.created_by.clone())
                            .collect::<Vec<_>>()
                    );
                    row["layer_sizes"] = json!(
                        manifest.as_ref().map(|m| m
                            .layers
                            .iter()
                            .map(|l| l.size)
                            .collect::<Vec<_>>())
                    );
                    // What the registry stores and what a pull transfers:
                    // the COMPRESSED layers. Not the size on disk, which
                    // would take a walk of every layer on every refresh,
                    // and the page labels it as what it is.
                    row["compressed_size"] =
                        json!(manifest.map(|m| m.layers.iter().map(|l| l.size).sum::<u64>()));
                    row["entrypoint"] = json!(section.entrypoint);
                    row["cmd"] = json!(section.cmd);
                    row["env"] = json!(section.env);
                    row["working_dir"] = json!(section.working_dir);
                    row["user"] = json!(section.user);
                    row["used_by"] = json!(
                        stacks
                            .iter()
                            .filter(|(_, names)| !ids.is_empty() && *names == ids)
                            .map(|(id, _)| id)
                            .collect::<Vec<_>>()
                    );
                    row
                })
                .collect(),
        )
    }

    fn volumes(&self) -> Value {
        let root = Store::default_root().join("volumes");
        let configs = self.configs();
        let mut rows = Vec::new();
        if let Ok(rd) = std::fs::read_dir(&root) {
            for e in rd.flatten() {
                if !e.path().is_dir() {
                    continue;
                }
                let mut budget = DU_BUDGET;
                let size = du(&e.path(), &mut budget);
                rows.push(json!({
                    "name": e.file_name().to_string_lossy(),
                    "path": e.path().display().to_string(),
                    "entries": std::fs::read_dir(e.path()).map(|d| d.count()).unwrap_or(0),
                    // Birth time where the filesystem records one; the
                    // directory's own mtime where it does not.
                    "created": e.metadata().ok().and_then(|m| m.created().or(m.modified()).ok())
                        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                        .map(|d| d.as_secs()),
                    "size": size,
                    // Out of budget means the number is a floor, and the
                    // page writes it as one.
                    "size_partial": budget == 0,
                    "used_by": configs
                        .iter()
                        .filter(|c| c.volumes.iter().any(|v| v.source == e.path()))
                        .map(|c| c.id.as_str())
                        .collect::<Vec<_>>(),
                }));
            }
        }
        rows.sort_by_key(|v| v["name"].as_str().unwrap_or("").to_owned());
        Value::Array(rows)
    }

    /// A container's filesystem as its own processes see it: through
    /// `/proc/<pid>/root`, which is the overlay, the volumes and the
    /// tmpfs mounts already assembled.
    ///
    /// A stopped container has no mount namespace left to look into, so
    /// what is shown instead is its writable layer: exactly the files it
    /// created or changed, without the image beneath them. That is less
    /// than the whole tree and it is the part that is the container's
    /// own - and the page says which of the two it is showing.
    fn container_files(
        &self,
        stream: &mut TcpStream,
        id: &str,
        req: &http::Request,
    ) -> std::io::Result<()> {
        let pid = State::load(&self.paths, id)
            .ok()
            .filter(|st| st.observed_status() == Status::Running)
            .and_then(|st| st.init_pid);
        let Some(pid) = pid else {
            let upper = Config::load(&self.paths, id)
                .ok()
                .and_then(|c| c.overlay_dir)
                .map(|d| Path::new(&d).join("upper"))
                .filter(|u| u.is_dir());
            return match upper {
                Some(u) => browse_reply(stream, &u, req),
                None => http::error(
                    stream,
                    409,
                    "this container has no files of its own to show",
                ),
            };
        };
        let root = PathBuf::from(format!("/proc/{pid}/root"));
        let rel = req.param("path").unwrap_or_default();
        let layered = Config::load(&self.paths, id)
            .ok()
            .and_then(|c| Some((Path::new(&c.overlay_dir?).join("upper"), c.layers)));
        let (Some((upper, layers)), None) = (layered, req.param("raw")) else {
            return browse_reply(stream, &root, req);
        };
        // What this container changed, against its image: an entry that
        // is in the writable layer is `added` if no image layer has it
        // and `modified` if one does. A directory is modified when
        // anything beneath it is, which is how overlayfs records it and
        // also what somebody looking for a change wants to follow.
        match browse(&root, &rel) {
            Ok(mut v) => {
                for e in v["entries"].as_array_mut().into_iter().flatten() {
                    let child = format!(
                        "{}/{}",
                        rel.trim_matches('/'),
                        e["name"].as_str().unwrap_or("")
                    );
                    if exists_in(&upper, &child) {
                        let below = layers.iter().any(|l| exists_in(Path::new(l), &child));
                        e["note"] = json!(if below { "modified" } else { "added" });
                    }
                }
                // What it deleted: gone from the merged view, so listed
                // from the writable layer, where overlayfs leaves a
                // marker in place of each removed name.
                if let Some(entries) = v["entries"].as_array_mut() {
                    for name in whiteouts_in(&upper, &rel) {
                        entries.push(json!({
                            "name": name, "kind": "other", "size": 0, "mode": "",
                            "uid": "", "gid": "", "mtime": 0, "note": "deleted",
                        }));
                    }
                }
                http::json(stream, 200, &v)
            }
            Err(e) => http::error(stream, 404, &e),
        }
    }

    /// A volume as a tar archive, streamed from the CLI's stdout so a
    /// large volume is never held in this process's memory.
    fn export_volume(&self, stream: &mut TcpStream, name: &str) -> std::io::Result<()> {
        let mut cmd = std::process::Command::new(&self.exe);
        if let Some(root) = &self.root_arg {
            cmd.arg("--root").arg(root);
        }
        let mut child = cmd
            .args(["volume", "export", name])
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null())
            .spawn()?;
        let out = child.stdout.take().expect("piped above");
        let r = download(stream, &format!("{name}.tar"), out);
        let _ = child.wait();
        r
    }

    // --- writes, all of which execute the CLI ---------------------------

    fn create(&self, stream: &mut TcpStream, req: &http::Request) -> std::io::Result<()> {
        let spec: Value = match serde_json::from_slice(&req.body) {
            Ok(v) => v,
            Err(e) => return http::error(stream, 400, &format!("bad JSON: {e}")),
        };
        let Some(image) = spec["image"].as_str().filter(|s| !s.is_empty()) else {
            return http::error(stream, 400, "an image is required");
        };

        let mut args: Vec<String> = vec!["run".into(), "-d".into()];
        if let Some(name) = spec["name"].as_str().filter(|s| !s.is_empty()) {
            args.push("--name".into());
            args.push(name.into());
        }
        if let Some(net) = spec["network"].as_str().filter(|s| !s.is_empty()) {
            args.push("--network".into());
            args.push(net.into());
        }
        for (key, field) in [("-p", "ports"), ("-v", "volumes"), ("-e", "env")] {
            if let Some(items) = spec[field].as_array() {
                for i in items.iter().filter_map(|v| v.as_str()) {
                    if i.is_empty() {
                        continue;
                    }
                    args.push(key.into());
                    args.push(i.into());
                }
            }
        }
        for (flag, field) in [
            ("-m", "memory"),
            ("--cpus", "cpus"),
            ("--pids-limit", "pids"),
        ] {
            if let Some(v) = spec[field].as_str().filter(|s| !s.is_empty()) {
                args.push(flag.into());
                args.push(v.into());
            }
        }
        args.push(image.into());
        // Split here rather than in the page: the page would have to
        // reimplement shell quoting to do it, and would get it wrong.
        if let Some(cmd) = spec["command"].as_str().filter(|s| !s.trim().is_empty()) {
            args.extend(cmd.split_whitespace().map(str::to_owned));
        }

        let refs: Vec<&str> = args.iter().map(String::as_str).collect();
        self.run_cli(stream, &refs)
    }

    fn build(&self, stream: &mut TcpStream, req: &http::Request) -> std::io::Result<()> {
        let spec: Value = serde_json::from_slice(&req.body).unwrap_or(Value::Null);
        let Some(tag) = spec["tag"].as_str().filter(|s| !s.is_empty()) else {
            return http::error(stream, 400, "a tag is required");
        };
        let context = spec["context"].as_str().unwrap_or(".");
        let mut args: Vec<&str> = vec!["build", "-t", tag];
        if let Some(f) = spec["file"].as_str().filter(|s| !s.is_empty()) {
            args.push("-f");
            args.push(f);
        }
        if let Some(n) = spec["network"]
            .as_str()
            .filter(|s| !s.is_empty() && !s.starts_with('-'))
        {
            args.push("--network");
            args.push(n);
        }
        if spec["no_cache"] == true {
            args.push("--no-cache");
        }
        if context.starts_with('-') {
            return http::error(stream, 400, "not a build context");
        }
        args.push(context);
        self.run_cli(stream, &args)
    }

    fn pull(&self, stream: &mut TcpStream, req: &http::Request) -> std::io::Result<()> {
        let spec: Value = serde_json::from_slice(&req.body).unwrap_or(Value::Null);
        let Some(reference) = spec["reference"].as_str().filter(|s| !s.is_empty()) else {
            return http::error(stream, 400, "a reference is required");
        };
        self.run_cli(stream, &["pull", reference])
    }

    fn exec(&self, stream: &mut TcpStream, id: &str, req: &http::Request) -> std::io::Result<()> {
        let spec: Value = serde_json::from_slice(&req.body).unwrap_or(Value::Null);
        let cmd = spec["command"].as_str().unwrap_or("").trim();
        if cmd.is_empty() {
            return http::error(stream, 400, "a command is required");
        }
        // One shot, output collected. Not a terminal: a pty would need a
        // websocket and a second protocol, and this page is a dashboard
        // rather than a shell. `exec` on the command line is the shell.
        let mut args: Vec<&str> = vec!["exec", id];
        let parts: Vec<&str> = cmd.split_whitespace().collect();
        args.extend(parts);
        self.run_cli(stream, &args)
    }

    /// Execute the CLI and report what it said.
    ///
    /// Every mutation goes through here, and the reason is at the top of
    /// this file: a container is created by a process that has never had a
    /// second thread, and this one has many.
    /// Save the request body as a file. Through the CLI like every
    /// other write, with the body on its stdin; the size is already
    /// bounded by the request limit, which is what makes this an editor
    /// for config files and not a way to ship a database.
    fn put_file(
        &self,
        stream: &mut TcpStream,
        target: &[&str],
        req: &http::Request,
    ) -> std::io::Result<()> {
        let Some(path) = req
            .param("path")
            .filter(|p| !p.trim_matches('/').is_empty())
        else {
            return http::error(stream, 400, "a path is required");
        };
        let mut args = vec!["put"];
        args.extend_from_slice(target);
        args.push(&path);
        self.run_cli_with(stream, &args, &req.body)
    }

    fn run_cli(&self, stream: &mut TcpStream, args: &[&str]) -> std::io::Result<()> {
        self.run_cli_with(stream, args, &[])
    }

    fn run_cli_with(
        &self,
        stream: &mut TcpStream,
        args: &[&str],
        stdin: &[u8],
    ) -> std::io::Result<()> {
        use std::io::Write;
        use std::process::Stdio;
        let mut cmd = std::process::Command::new(&self.exe);
        if let Some(root) = &self.root_arg {
            cmd.arg("--root").arg(root);
        }
        cmd.args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let out = cmd.spawn().and_then(|mut child| {
            // Written and closed before waiting: the child reads to EOF.
            // At most the request limit, so it cannot fill both pipes.
            if let Some(mut input) = child.stdin.take() {
                let _ = input.write_all(stdin);
            }
            child.wait_with_output()
        });
        let out = match out {
            Ok(o) => o,
            Err(e) => return http::error(stream, 500, &format!("running the engine: {e}")),
        };
        let stdout = String::from_utf8_lossy(&out.stdout).trim().to_owned();
        let stderr = String::from_utf8_lossy(&out.stderr).trim().to_owned();
        let code = out.status.code().unwrap_or(-1);
        http::json(
            stream,
            if code == 0 { 200 } else { 400 },
            &json!({ "ok": code == 0, "exit": code, "stdout": stdout, "stderr": stderr }),
        )
    }
}

/// How many directory entries a volume's size may cost to compute. The
/// list is polled, and a volume holding a million files must not turn
/// every poll into a million `lstat`s.
const DU_BUDGET: usize = 20_000;
/// How much of a file the browser shows.
const FILE_HEAD: u64 = 64 * 1024;
/// How many entries of one directory the browser lists.
const DIR_MAX: usize = 2000;

/// Bytes under `dir`, not following symlinks, until `budget` runs out.
fn du(dir: &Path, budget: &mut usize) -> u64 {
    let mut total = 0;
    let Ok(rd) = std::fs::read_dir(dir) else {
        return 0;
    };
    for e in rd.flatten() {
        if *budget == 0 {
            break;
        }
        *budget -= 1;
        let Ok(m) = e.metadata() else { continue };
        total += if m.is_dir() {
            du(&e.path(), budget)
        } else {
            m.len()
        };
    }
    total
}

/// Send a body of unknown length as a file to save. No Content-Length:
/// the connection closing is the end, which HTTP/1.1 allows and which
/// is what lets the body be streamed instead of buffered.
fn download(stream: &mut TcpStream, name: &str, mut body: impl Read) -> std::io::Result<()> {
    use std::io::Write;
    let name = if is_safe_id(name) { name } else { "download" };
    write!(
        stream,
        "HTTP/1.1 200 OK\r\nContent-Type: application/octet-stream\r\n\
         Content-Disposition: attachment; filename=\"{name}\"\r\n\
         Connection: close\r\nX-Content-Type-Options: nosniff\r\nCache-Control: no-store\r\n\r\n"
    )?;
    std::io::copy(&mut body, stream)?;
    stream.flush()
}

fn browse_reply(stream: &mut TcpStream, root: &Path, req: &http::Request) -> std::io::Result<()> {
    let rel = req.param("path").unwrap_or_default();
    // `raw=1`: the whole file, as a download, through the same confined
    // lookup as everything else here.
    if req.param("raw").is_some() {
        return match resolve(root, &rel) {
            Ok((via, meta, _fd)) if meta.is_file() => match std::fs::File::open(&via) {
                Ok(f) => {
                    let name = rel.rsplit('/').next().unwrap_or("");
                    download(stream, name, f)
                }
                Err(e) => http::error(stream, 404, &e.to_string()),
            },
            Ok(_) => http::error(stream, 400, "only a regular file can be downloaded"),
            Err(e) => http::error(stream, 404, &e),
        };
    }
    match browse(root, &rel) {
        Ok(v) => http::json(stream, 200, &v),
        Err(e) => http::error(stream, 404, &e),
    }
}

/// Is there anything at `rel` under `root`? Confined like every other
/// lookup here, and it does not follow a link at the final name: the
/// question is whether the LAYER has an entry there, not where a link
/// in it points.
fn exists_in(root: &Path, rel: &str) -> bool {
    use rustix::fs::{Mode, OFlags, ResolveFlags};
    let Ok(rootfd) = rustix::fs::open(
        root,
        OFlags::DIRECTORY | OFlags::RDONLY | OFlags::CLOEXEC,
        Mode::empty(),
    ) else {
        return false;
    };
    let rel = rel.trim_start_matches('/');
    !rel.is_empty()
        && rustix::fs::openat2(
            &rootfd,
            rel,
            OFlags::PATH | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::empty(),
            ResolveFlags::IN_ROOT | ResolveFlags::NO_MAGICLINKS,
        )
        .is_ok()
}

/// The names a container deleted from the directory `rel`: overlayfs
/// records each as a character device numbered 0,0 in the upper layer.
// ponytail: a directory deleted and recreated ("opaque") hides what was
// under it without one marker per name, and those are not listed.
fn whiteouts_in(upper: &Path, rel: &str) -> Vec<String> {
    use std::os::unix::fs::{FileTypeExt, MetadataExt};
    let dir = if rel.trim_matches('/').is_empty() {
        upper.to_path_buf()
    } else {
        match resolve(upper, rel) {
            Ok((name, m, _fd)) if m.is_dir() => {
                return whiteouts_at(Path::new(&name));
            }
            _ => return Vec::new(),
        }
    };
    return whiteouts_at(&dir);

    fn whiteouts_at(dir: &Path) -> Vec<String> {
        let mut out: Vec<String> = std::fs::read_dir(dir)
            .into_iter()
            .flatten()
            .flatten()
            .filter(|e| {
                e.metadata()
                    .is_ok_and(|m| m.file_type().is_char_device() && m.rdev() == 0)
            })
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .collect();
        out.sort();
        out
    }
}

/// Find `rel` under `root` without leaving it, and say what it is.
/// Returns the descriptor's `/proc/self/fd` name - the one path that
/// cannot be re-resolved to something else between the check and the
/// use - and the descriptor, which has to outlive that name.
fn resolve(
    root: &Path,
    rel: &str,
) -> Result<(String, std::fs::Metadata, rustix::fd::OwnedFd), String> {
    use rustix::fs::{Mode, OFlags, ResolveFlags};
    use std::os::fd::AsRawFd;
    let rootfd = rustix::fs::open(
        root,
        OFlags::DIRECTORY | OFlags::RDONLY | OFlags::CLOEXEC,
        Mode::empty(),
    )
    .map_err(|e| format!("opening the root: {e}"))?;
    let rel = rel.trim_start_matches('/');
    let fd = rustix::fs::openat2(
        &rootfd,
        if rel.is_empty() { "." } else { rel },
        OFlags::PATH | OFlags::CLOEXEC,
        Mode::empty(),
        ResolveFlags::IN_ROOT | ResolveFlags::NO_MAGICLINKS,
    )
    .map_err(|e| format!("/{rel}: {e}"))?;
    // The descriptor, by its /proc name: the one path that cannot be
    // re-resolved to something else between the check and the use.
    let via = format!("/proc/self/fd/{}", fd.as_raw_fd());
    let meta = std::fs::metadata(&via).map_err(|e| e.to_string())?;
    Ok((via, meta, fd))
}

/// List a directory, or show the head of a file, somewhere under `root`.
///
/// The path comes from a URL and the tree it names belongs to a
/// container, so neither is trusted. `openat2(RESOLVE_IN_ROOT)` makes
/// the kernel treat `root` as `/` for the whole walk: `..` stops there,
/// and an absolute symlink - `/etc -> /`, or a link a container planted
/// at `/var/log/x -> /etc/shadow` hoping the host would read its own -
/// resolves inside the tree instead of on the host. Checking the string
/// for `..` would not have covered the symlink.
///
/// Opened `O_PATH` first and reopened only once it is known to be a
/// directory or a regular file: opening a FIFO blocks and opening a
/// device node can have effects, and a container gets to choose what is
/// at the path.
fn browse(root: &Path, rel: &str) -> Result<Value, String> {
    use std::os::unix::fs::{MetadataExt, PermissionsExt};
    let (via, meta, _fd) = resolve(root, rel)?;

    if meta.is_dir() {
        let mut entries = Vec::new();
        let mut truncated = false;
        for e in std::fs::read_dir(&via)
            .map_err(|e| e.to_string())?
            .flatten()
        {
            if entries.len() == DIR_MAX {
                truncated = true;
                break;
            }
            let Ok(m) = e.metadata() else { continue };
            let ft = m.file_type();
            entries.push(json!({
                "name": e.file_name().to_string_lossy(),
                "kind": if ft.is_dir() { "dir" } else if ft.is_symlink() { "link" }
                        else if ft.is_file() { "file" } else { "other" },
                "size": m.len(),
                "mode": format!("{:04o}", m.permissions().mode() & 0o7777),
                "uid": m.uid(),
                "gid": m.gid(),
                "mtime": m.mtime(),
                "target": ft.is_symlink().then(|| std::fs::read_link(e.path()).ok()
                    .map(|t| t.display().to_string())).flatten(),
            }));
        }
        entries.sort_by_key(|e| {
            (
                e["kind"] != "dir",
                e["name"].as_str().unwrap_or("").to_owned(),
            )
        });
        return Ok(json!({ "kind": "dir", "entries": entries, "truncated": truncated }));
    }
    if meta.is_file() {
        let mut head = Vec::new();
        std::fs::File::open(&via)
            .and_then(|f| f.take(FILE_HEAD).read_to_end(&mut head))
            .map_err(|e| e.to_string())?;
        let binary = head.contains(&0);
        return Ok(json!({
            "kind": "file",
            "size": meta.len(),
            "truncated": meta.len() > FILE_HEAD,
            "binary": binary,
            "text": (!binary).then(|| String::from_utf8_lossy(&head).into_owned()),
        }));
    }
    Ok(json!({ "kind": "other" }))
}

/// Ids come back to us in a URL and become a path. Anything that is not
/// plainly a name is refused rather than sanitised, because sanitising is
/// where traversal bugs live.
fn is_safe_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 128
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.')
        && !id.starts_with('.')
}

fn read_trimmed(p: impl AsRef<Path>) -> Option<String> {
    std::fs::read_to_string(p).ok().map(|s| s.trim().to_owned())
}

/// The cgroup directory of a running container.
///
/// Read from `/proc/<pid>/cgroup`, which reports relative to the READER's
/// cgroup namespace — ours, the host's — so this is the full path. Read
/// from inside the container it would be `/`.
fn cgroup_dir_of(pid: i32) -> Option<PathBuf> {
    let s = std::fs::read_to_string(format!("/proc/{pid}/cgroup")).ok()?;
    let rel = s.lines().find_map(|l| l.strip_prefix("0::"))?.trim();
    let dir = PathBuf::from("/sys/fs/cgroup").join(rel.trim_start_matches('/'));
    dir.is_dir().then_some(dir)
}

/// The last `n` lines of a log file, without reading the whole thing.
///
/// A container that has been running for a week has a log measured in
/// gigabytes, and a dashboard that reads all of it to show the last
/// screenful is a dashboard that takes the machine down.
fn tail_file(path: &Path, n: usize) -> (String, Vec<Option<u64>>) {
    const WINDOW: u64 = 256 * 1024;
    let Ok(mut f) = std::fs::File::open(path) else {
        return (String::new(), Vec::new());
    };
    let len = f.metadata().map(|m| m.len()).unwrap_or(0);
    let start = len.saturating_sub(WINDOW);
    if start > 0 {
        use std::io::Seek;
        let _ = f.seek(std::io::SeekFrom::Start(start));
    }
    let mut buf = Vec::new();
    let _ = f.take(WINDOW).read_to_end(&mut buf);
    // A window boundary usually lands mid-line; drop that fragment rather
    // than show half a line as if it were whole.
    let skip = if start > 0 {
        buf.iter()
            .position(|&b| b == b'\n')
            .map_or(buf.len(), |i| i + 1)
    } else {
        0
    };
    // Each line with the byte offset it starts at in the file, which is
    // what the time index is keyed by.
    let mut lines: Vec<(u64, &[u8])> = Vec::new();
    let mut at = skip;
    for line in buf[skip..].split_inclusive(|&b| b == b'\n') {
        lines.push((start + at as u64, line.strip_suffix(b"\n").unwrap_or(line)));
        at += line.len();
    }
    let lines = &lines[lines.len().saturating_sub(n)..];

    // `offset usec` pairs, ascending. The time of a line is the last
    // entry at or before where it starts.
    let mut idx_path = path.as_os_str().to_owned();
    idx_path.push(".idx");
    // Only its tail: the index grows for as long as the container
    // talks, and the lines being shown are at the end of the log.
    let mut raw = Vec::new();
    if let Ok(mut f) = std::fs::File::open(idx_path) {
        use std::io::Seek;
        let _ = f.seek(std::io::SeekFrom::End(-(WINDOW as i64)));
        let _ = f.read_to_end(&mut raw);
    }
    let index: Vec<(u64, u64)> = String::from_utf8_lossy(&raw)
        .lines()
        .filter_map(|l| {
            let (o, t) = l.split_once(' ')?;
            Some((o.parse().ok()?, t.parse().ok()?))
        })
        .collect();
    let times = lines
        .iter()
        .map(|(off, _)| {
            let i = index.partition_point(|(o, _)| o <= off);
            (i > 0).then(|| index[i - 1].1)
        })
        .collect();
    let text = lines
        .iter()
        .map(|(_, l)| String::from_utf8_lossy(l))
        .collect::<Vec<_>>()
        .join("\n");
    (text, times)
}

fn now_usec() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_micros() as u64)
        .unwrap_or(0)
}

// ---------------------------------------------------------------------------
// terminal sessions
// ---------------------------------------------------------------------------

/// One live `exec -t` session, as seen from the page.
///
/// The server allocates the pty and spawns the CLI with the slave as its
/// stdio - it does NOT create the container process itself, for the same
/// reason as every other mutation here: this process is multi-threaded
/// and `clone3` refuses one. The CLI it spawns is single-threaded and
/// does the cloning.
///
/// Output accumulates in a bounded buffer that the page polls by offset.
/// Polling rather than a websocket because a websocket is a second
/// protocol, a second framing, and a second set of failure modes, for a
/// page that already polls everything else twice a second.
pub struct Term {
    master: std::os::fd::OwnedFd,
    child: std::process::Child,
    /// Everything the session has produced, oldest first.
    out: std::sync::Mutex<Vec<u8>>,
    /// How many bytes have been dropped off the front, so a page that
    /// polls by offset can tell it missed some rather than silently
    /// resyncing to the wrong place.
    dropped: std::sync::atomic::AtomicU64,
    done: std::sync::atomic::AtomicBool,
}

/// The most output one session keeps. A terminal that has been open all
/// day is a terminal whose first screen nobody wants.
const TERM_BUFFER: usize = 256 * 1024;

impl Term {
    fn read_loop(me: std::sync::Arc<Term>) {
        let mut buf = [0u8; 8192];
        loop {
            match rustainer_core::sys::read(&me.master, &mut buf) {
                Ok(0) | Err(_) => break,
                Ok(n) => {
                    let mut out = me.out.lock().unwrap_or_else(|e| e.into_inner());
                    out.extend_from_slice(&buf[..n]);
                    if out.len() > TERM_BUFFER {
                        let cut = out.len() - TERM_BUFFER;
                        out.drain(..cut);
                        me.dropped
                            .fetch_add(cut as u64, std::sync::atomic::Ordering::Relaxed);
                    }
                }
            }
        }
        me.done.store(true, std::sync::atomic::Ordering::Relaxed);
    }
}

impl Drop for Term {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

impl Server {
    fn term_open(
        &self,
        stream: &mut TcpStream,
        id: &str,
        req: &http::Request,
    ) -> std::io::Result<()> {
        let spec: Value = serde_json::from_slice(&req.body).unwrap_or(Value::Null);
        let shell = spec["command"]
            .as_str()
            .unwrap_or("/bin/sh")
            .trim()
            .to_owned();
        let rows = spec["rows"].as_u64().unwrap_or(24) as u16;
        let cols = spec["cols"].as_u64().unwrap_or(80) as u16;

        let pty = match rustainer_core::pty::open() {
            Ok(p) => p,
            Err(e) => return http::error(stream, 500, &format!("allocating a pty: {e}")),
        };
        let _ = rustainer_core::pty::set_size(&pty.master, rows, cols);

        let mut cmd = std::process::Command::new(&self.exe);
        if let Some(root) = &self.root_arg {
            cmd.arg("--root").arg(root);
        }
        cmd.arg("exec").arg("-t").arg(id);
        cmd.args(shell.split_whitespace());
        // The slave becomes the CLI's stdio. `Stdio::from` takes
        // ownership of each, so three clones - and every one of them
        // must be gone from THIS process, or the master never sees EOF
        // when the session ends.
        let (a, b, c) = match (
            pty.slave.try_clone(),
            pty.slave.try_clone(),
            pty.slave.try_clone(),
        ) {
            (Ok(a), Ok(b), Ok(c)) => (a, b, c),
            _ => return http::error(stream, 500, "could not duplicate the terminal"),
        };
        cmd.stdin(std::process::Stdio::from(a))
            .stdout(std::process::Stdio::from(b))
            .stderr(std::process::Stdio::from(c));
        let child = match cmd.spawn() {
            Ok(c) => c,
            Err(e) => return http::error(stream, 500, &format!("starting the session: {e}")),
        };
        drop(pty.slave);

        let term = std::sync::Arc::new(Term {
            master: pty.master,
            child,
            out: std::sync::Mutex::new(Vec::new()),
            dropped: std::sync::atomic::AtomicU64::new(0),
            done: std::sync::atomic::AtomicBool::new(false),
        });
        let reader = std::sync::Arc::clone(&term);
        std::thread::spawn(move || Term::read_loop(reader));

        let session = format!("{:016x}", now_usec());
        self.terms
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(session.clone(), term);
        http::json(stream, 200, &json!({ "session": session }))
    }

    fn term_read(
        &self,
        stream: &mut TcpStream,
        session: &str,
        req: &http::Request,
    ) -> std::io::Result<()> {
        let from: u64 = req.param("from").and_then(|v| v.parse().ok()).unwrap_or(0);
        let terms = self.terms.lock().unwrap_or_else(|e| e.into_inner());
        let Some(t) = terms.get(session) else {
            return http::error(stream, 404, "no such session");
        };
        let dropped = t.dropped.load(std::sync::atomic::Ordering::Relaxed);
        let out = t.out.lock().unwrap_or_else(|e| e.into_inner());
        let start = from.saturating_sub(dropped) as usize;
        let slice = if start >= out.len() {
            &out[out.len()..]
        } else {
            &out[start..]
        };
        http::json(
            stream,
            200,
            &json!({
                // Lossy on purpose: a terminal carries bytes, and a page
                // cannot render half a UTF-8 sequence either way. The
                // replacement character is visible; a 500 is not useful.
                "data": String::from_utf8_lossy(slice),
                "next": dropped + out.len() as u64,
                "dropped": dropped > from,
                "done": t.done.load(std::sync::atomic::Ordering::Relaxed),
            }),
        )
    }

    fn term_write(
        &self,
        stream: &mut TcpStream,
        session: &str,
        req: &http::Request,
    ) -> std::io::Result<()> {
        let spec: Value = serde_json::from_slice(&req.body).unwrap_or(Value::Null);
        let terms = self.terms.lock().unwrap_or_else(|e| e.into_inner());
        let Some(t) = terms.get(session) else {
            return http::error(stream, 404, "no such session");
        };
        if let Some(rows) = spec["rows"].as_u64()
            && let Some(cols) = spec["cols"].as_u64()
        {
            let _ = rustainer_core::pty::set_size(&t.master, rows as u16, cols as u16);
        }
        if let Some(data) = spec["data"].as_str()
            && !data.is_empty()
        {
            let _ = rustainer_core::sys::write(&t.master, data.as_bytes());
        }
        http::json(stream, 200, &json!({ "ok": true }))
    }

    fn term_close(&self, stream: &mut TcpStream, session: &str) -> std::io::Result<()> {
        self.terms
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(session);
        http::json(stream, 200, &json!({ "ok": true }))
    }

    fn networks(&self) -> Value {
        let mut rows = vec![json!({
            "name": "bridge",
            "subnet": format!("{}/{}", rustainer_core::net::GATEWAY, rustainer_core::net::SUBNET_PREFIX),
            "bridge": rustainer_core::net::BRIDGE,
            "builtin": true,
        })];
        let dir = rustainer_core::state::Paths::data_root().join("networks");
        if let Ok(rd) = std::fs::read_dir(dir) {
            let mut found: Vec<Value> = Vec::new();
            for e in rd.flatten() {
                if e.path().extension().and_then(|x| x.to_str()) != Some("json") {
                    continue;
                }
                if let Ok(t) = std::fs::read_to_string(e.path())
                    && let Ok(n) = serde_json::from_str::<rustainer_core::net::Network>(&t)
                {
                    found.push(json!({
                        "name": n.name,
                        "subnet": format!("{}/{}", n.gateway, n.prefix),
                        "bridge": n.bridge,
                        "builtin": false,
                    }));
                }
            }
            found.sort_by_key(|v| v["name"].as_str().unwrap_or("").to_owned());
            rows.extend(found);
        }
        Value::Array(rows)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_id_that_could_escape_the_state_directory_is_refused() {
        // These arrive in a URL and become a path. Refused, not sanitised:
        // sanitising is where traversal bugs live.
        for bad in ["..", "../etc", "a/b", "", ".hidden", "a\0b"] {
            assert!(!is_safe_id(bad), "{bad:?} was accepted");
        }
        for good in ["9f2c1a4e7b03", "my-container", "web_1", "a.b"] {
            assert!(is_safe_id(good), "{good:?} was refused");
        }
    }

    #[test]
    fn browsing_cannot_leave_the_root_by_dotdot_or_by_symlink() {
        let base = std::env::temp_dir().join(format!("rustainer-browse-{}", std::process::id()));
        let root = base.join("root");
        std::fs::create_dir_all(root.join("sub")).unwrap();
        std::fs::write(base.join("outside"), "host secret").unwrap();
        std::fs::write(root.join("sub/inside"), "hello").unwrap();
        // The two shapes a container can plant: a link by absolute path,
        // and a link that climbs.
        std::os::unix::fs::symlink(base.join("outside"), root.join("abs")).unwrap();
        std::os::unix::fs::symlink("../outside", root.join("climb")).unwrap();
        std::os::unix::fs::symlink("/sub/inside", root.join("ok")).unwrap();

        let listing = browse(&root, "/").unwrap();
        assert_eq!(listing["kind"], "dir");
        assert_eq!(
            listing["entries"][0]["name"], "sub",
            "directories sort first"
        );
        assert_eq!(browse(&root, "sub/inside").unwrap()["text"], "hello");
        // An absolute link is reinterpreted against the root, which is
        // what makes one that points inside the tree keep working.
        assert_eq!(browse(&root, "ok").unwrap()["text"], "hello");
        for escape in ["../outside", "sub/../../outside", "abs", "climb"] {
            let got = browse(&root, escape);
            assert!(
                !format!("{got:?}").contains("host secret"),
                "{escape} read a file outside the root: {got:?}"
            );
        }
        std::fs::remove_dir_all(&base).unwrap();
    }

    #[test]
    fn tailing_returns_the_end_and_never_the_whole_file() {
        let p = std::env::temp_dir().join("rustainer-api-tail-test.log");
        let body: String = (0..5000).map(|i| format!("line {i}\n")).collect();
        std::fs::write(&p, &body).unwrap();
        let (out, times) = tail_file(&p, 3);
        assert_eq!(out.lines().count(), 3);
        assert_eq!(times, vec![None; 3], "no index, so no times - and no error");
        assert!(out.ends_with("line 4999"), "{out}");
        // And a short file comes back whole.
        std::fs::write(&p, "one\ntwo\n").unwrap();
        assert_eq!(tail_file(&p, 10).0, "one\ntwo");
        // With an index: a line takes the time of the last entry at or
        // before where it starts.
        let mut idx = p.clone().into_os_string();
        idx.push(".idx");
        std::fs::write(&idx, "0 111\n4 222\n").unwrap();
        assert_eq!(tail_file(&p, 10).1, vec![Some(111), Some(222)]);
        std::fs::remove_file(&idx).unwrap();
        std::fs::remove_file(&p).ok();
    }
}
