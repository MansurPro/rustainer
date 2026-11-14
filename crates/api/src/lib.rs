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
}

impl Server {
    pub fn new(paths: Paths, root_arg: Option<String>) -> Self {
        Self {
            paths,
            exe: PathBuf::from("/proc/self/exe"),
            root_arg,
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
            ("GET", "/api/volumes") => return http::json(stream, 200, &self.volumes()),
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
                ("POST", "stop") => self.run_cli(stream, &["stop", "-t", "5", &id]),
                ("POST", "kill") => self.run_cli(stream, &["stop", "-t", "0", &id]),
                ("POST", "exec") => self.exec(stream, &id, req),
                ("DELETE", "") => self.run_cli(stream, &["rm", "--force", &id]),
                ("GET", "") => http::json(stream, 200, &self.one(&id)),
                _ => http::error(stream, 405, "no such container action"),
            };
        }

        if let Some(name) = path.strip_prefix("/api/volumes/") {
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
        json!({
            "running": true,
            "cgroup": true,
            "memory_current": read("memory.current").and_then(|s| s.parse::<u64>().ok()),
            "memory_max": read("memory.max"),
            "memory_peak": read("memory.peak").and_then(|s| s.parse::<u64>().ok()),
            "pids_current": read("pids.current").and_then(|s| s.parse::<u64>().ok()),
            "pids_max": read("pids.max"),
            "cpu_usec": cpu_usec,
            "sampled_at_usec": now_usec(),
        })
    }

    fn logs(&self, stream: &mut TcpStream, id: &str, req: &http::Request) -> std::io::Result<()> {
        let tail: usize = req
            .param("tail")
            .and_then(|v| v.parse().ok())
            .unwrap_or(400)
            .min(5000);
        let mut out = tail_file(&self.paths.stdout_log(id), tail);
        let err = tail_file(&self.paths.stderr_log(id), tail);
        http::json(
            stream,
            200,
            &json!({ "stdout": std::mem::take(&mut out), "stderr": err }),
        )
    }

    fn images(&self) -> Value {
        let store = Store::new(Store::default_root());
        let list = store.list().unwrap_or_default();
        Value::Array(
            list.into_iter()
                .map(|(reference, digest)| json!({ "reference": reference, "digest": digest }))
                .collect(),
        )
    }

    fn volumes(&self) -> Value {
        let root = Store::default_root().join("volumes");
        let mut rows = Vec::new();
        if let Ok(rd) = std::fs::read_dir(&root) {
            for e in rd.flatten() {
                if !e.path().is_dir() {
                    continue;
                }
                rows.push(json!({
                    "name": e.file_name().to_string_lossy(),
                    "path": e.path().display().to_string(),
                    "entries": std::fs::read_dir(e.path()).map(|d| d.count()).unwrap_or(0),
                }));
            }
        }
        rows.sort_by_key(|v| v["name"].as_str().unwrap_or("").to_owned());
        Value::Array(rows)
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
    fn run_cli(&self, stream: &mut TcpStream, args: &[&str]) -> std::io::Result<()> {
        let mut cmd = std::process::Command::new(&self.exe);
        if let Some(root) = &self.root_arg {
            cmd.arg("--root").arg(root);
        }
        cmd.args(args);
        let out = match cmd.output() {
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
fn tail_file(path: &Path, n: usize) -> String {
    const WINDOW: u64 = 256 * 1024;
    let Ok(mut f) = std::fs::File::open(path) else {
        return String::new();
    };
    let len = f.metadata().map(|m| m.len()).unwrap_or(0);
    let start = len.saturating_sub(WINDOW);
    if start > 0 {
        use std::io::Seek;
        let _ = f.seek(std::io::SeekFrom::Start(start));
    }
    let mut buf = Vec::new();
    let _ = f.take(WINDOW).read_to_end(&mut buf);
    let text = String::from_utf8_lossy(&buf);
    // A window boundary usually lands mid-line; drop that fragment rather
    // than show half a line as if it were whole.
    let text = if start > 0 {
        text.split_once('\n').map(|(_, rest)| rest).unwrap_or("")
    } else {
        &text
    };
    let lines: Vec<&str> = text.lines().collect();
    lines[lines.len().saturating_sub(n)..].join("\n")
}

fn now_usec() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_micros() as u64)
        .unwrap_or(0)
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
    fn tailing_returns_the_end_and_never_the_whole_file() {
        let p = std::env::temp_dir().join("rustainer-api-tail-test.log");
        let body: String = (0..5000).map(|i| format!("line {i}\n")).collect();
        std::fs::write(&p, &body).unwrap();
        let out = tail_file(&p, 3);
        assert_eq!(out.lines().count(), 3);
        assert!(out.ends_with("line 4999"), "{out}");
        // And a short file comes back whole.
        std::fs::write(&p, "one\ntwo\n").unwrap();
        assert_eq!(tail_file(&p, 10), "one\ntwo");
        std::fs::remove_file(&p).ok();
    }
}
