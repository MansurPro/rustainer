//! Stacks: several containers described in one file, started together.
//!
//! Compose-shaped, deliberately not Compose.
//!
//! # Why not YAML
//!
//! Because a configuration language should not be able to surprise you,
//! and YAML can. `ports: - 8080:80` is a sexagesimal number in YAML 1.1,
//! `no` is a boolean, and whether two spaces or four are correct depends
//! on what the line above did. Every one of those has cost somebody an
//! afternoon. TOML has one way to write a string, one way to write a
//! list, and comments.
//!
//! # Why unknown keys are an error
//!
//! A misspelled key in a Compose file does nothing at all, silently, and
//! the service starts without the setting you thought you gave it. That
//! is the single most expensive property of the format. Here, `imgae =
//! "alpine"` is a refusal that names the line.
//!
//! # What a stack is not
//!
//! It is not an orchestrator. There is no reconciliation loop, no
//! rescheduling, no health-gated rollout. `up` starts what is not running,
//! in dependency order; `down` stops it in reverse. The containers are
//! ordinary containers and `rustainer ps` shows them as such — which is
//! the property that makes the whole thing debuggable with the tools you
//! already have.

use std::collections::{BTreeMap, BTreeSet};

use anyhow::{Context, Result, bail};
use serde::Deserialize;

#[derive(Debug, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Stack {
    /// Prefixes every container name, so one host can run two stacks.
    pub name: String,
    pub services: BTreeMap<String, Service>,
    /// Files to put inside containers that ask for them.
    ///
    /// `configs.<name>.file` is a path, relative to the stack file.
    /// A service lists the names it wants and gets each one at
    /// `/<name>`, read-only.
    #[serde(default)]
    pub configs: BTreeMap<String, FileSource>,
    /// The same, mounted under `/run/secrets/` and refused if the
    /// source file is readable by anyone but its owner.
    ///
    /// That check is the entire difference between the two, and it is
    /// the one worth having: a secret in a world-readable file in the
    /// repository is the most common way one leaks, and a tool that
    /// mounts it without comment has helped.
    #[serde(default)]
    pub secrets: BTreeMap<String, FileSource>,
}

/// Where a config or secret comes from.
#[derive(Debug, Clone, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct FileSource {
    /// A path, relative to the stack file.
    pub file: String,
}

#[derive(Debug, Deserialize, PartialEq, Default)]
#[serde(deny_unknown_fields)]
pub struct Service {
    /// The image to run. Optional only when `build` is given, in which
    /// case it names the tag the build produces.
    #[serde(default)]
    pub image: String,
    /// Build this service's image from a Dockerfile before starting it.
    ///
    /// The value is a context directory, relative to the stack file -
    /// not to the working directory - because a stack file that means
    /// something different depending on where you ran it from is the
    /// whole problem with relative paths in configuration.
    #[serde(default)]
    pub build: Option<String>,
    /// `no` (the default), `always`, `unless-stopped`, `on-failure`, or
    /// `on-failure:N`. The same strings `run --restart` takes.
    #[serde(default)]
    pub restart: Option<String>,
    /// Profiles this service belongs to.
    ///
    /// A service with profiles is NOT started unless one of them is
    /// named with `--profile`. A service with none always is, which is
    /// the same rule Compose uses and the only one that keeps an
    /// unprofiled file behaving as it did.
    #[serde(default)]
    pub profiles: Vec<String>,
    /// Overrides the image's own entrypoint and command.
    ///
    /// A LIST, always - `["/bin/sh", "-c", "echo hi; sleep 1"]` - and
    /// never a string. Compose accepts both, and the string form is a
    /// reliable way to lose an afternoon: it is split on spaces, so the
    /// quotes in `sh -c "echo hi; sleep 1"` become part of the arguments
    /// and the shell is handed `"echo` as its whole script. There is no
    /// shell here to re-quote it, and inventing a quoting dialect for a
    /// config file is worse than requiring a list.
    #[serde(default)]
    pub command: Vec<String>,
    #[serde(default)]
    pub env: Vec<String>,
    /// `8080:80`, as `-p` takes it.
    #[serde(default)]
    pub ports: Vec<String>,
    /// `name:/path` or `/host:/path[:ro]`, as `-v` takes it.
    #[serde(default)]
    pub volumes: Vec<String>,
    /// Services that must be started before this one.
    ///
    /// Either a list of names, or a table giving a condition per name:
    ///
    /// ```toml
    /// depends_on = ["db"]
    /// depends_on = { db = "healthy" }
    /// ```
    ///
    /// `started` (the default) means the container exists and is
    /// running. `healthy` means its `healthcheck` has passed, and `up`
    /// waits for that before starting this service.
    #[serde(default)]
    pub depends_on: DependsOn,
    /// Inherit every field of another service in this stack, then
    /// override what is set here.
    #[serde(default)]
    pub extends: Option<String>,
    /// A command that decides whether this service is ready.
    #[serde(default)]
    pub healthcheck: Option<HealthCheck>,
    /// Paths to watch while `stack watch` runs, and what to do when
    /// they change.
    #[serde(default)]
    pub watch: Vec<Watch>,
    /// Configs this service wants, by name. Each arrives at `/<name>`.
    #[serde(default)]
    pub configs: Vec<String>,
    /// Secrets this service wants. Each arrives at `/run/secrets/<name>`.
    #[serde(default)]
    pub secrets: Vec<String>,
    /// How many containers to run for this service.
    ///
    /// One by default, and one keeps the plain `<stack>-<service>`
    /// name: a file that never mentions replicas must not have its
    /// container names change underneath it. Above one they are
    /// `<stack>-<service>-1`, `-2`, and so on.
    #[serde(default = "one_replica")]
    pub replicas: u32,
    /// Compose's `deploy.placement.constraints`: conditions on the node
    /// a service may run on, e.g. `node.platform.arch == aarch64`.
    ///
    /// There is one node here, so this cannot choose between hosts. It
    /// does what a one-node swarm does with the same file: evaluates
    /// each constraint against this machine and refuses to start the
    /// service if one does not hold - which is the useful half, because
    /// it is what stops an amd64-only service being started on arm.
    #[serde(default)]
    pub placement: Vec<String>,
    #[serde(default)]
    pub memory: Option<String>,
    #[serde(default)]
    pub cpus: Option<f64>,
    #[serde(default)]
    pub pids_limit: Option<u64>,
    #[serde(default)]
    pub user: Option<String>,
    #[serde(default)]
    pub workdir: Option<String>,
    /// `bridge` (the default), `none`, or `host`.
    #[serde(default)]
    pub network: Option<String>,
}

/// `depends_on`, in either of the two shapes Compose accepts.
#[derive(Debug, Clone, Deserialize, PartialEq)]
#[serde(untagged)]
pub enum DependsOn {
    /// `depends_on = ["db"]` - started, not healthy.
    Names(Vec<String>),
    /// `depends_on = { db = "healthy" }`.
    Conditions(BTreeMap<String, String>),
}

impl Default for DependsOn {
    fn default() -> Self {
        DependsOn::Names(Vec::new())
    }
}

impl DependsOn {
    /// Every service named, whatever the shape.
    pub fn names(&self) -> Vec<&str> {
        match self {
            DependsOn::Names(v) => v.iter().map(String::as_str).collect(),
            DependsOn::Conditions(m) => m.keys().map(String::as_str).collect(),
        }
    }

    /// Does this dependency have to be HEALTHY, not merely started?
    pub fn wants_healthy(&self, dep: &str) -> bool {
        match self {
            DependsOn::Names(_) => false,
            DependsOn::Conditions(m) => m.get(dep).map(String::as_str) == Some("healthy"),
        }
    }

    fn validate(&self) -> Result<()> {
        if let DependsOn::Conditions(m) = self {
            for (name, cond) in m {
                if cond != "started" && cond != "healthy" {
                    bail!(
                        "{cond:?} is not a condition for {name:?}. Use \"started\" \
                         (the default) or \"healthy\", which waits for that \
                         service's healthcheck to pass."
                    );
                }
            }
        }
        Ok(())
    }
}

/// One placement constraint: `<key> == <value>` or `<key> != <value>`.
#[derive(Debug, PartialEq)]
pub struct Constraint {
    pub key: String,
    pub equal: bool,
    pub value: String,
}

impl Constraint {
    /// The keys a single machine can answer for itself. `node.labels.*`
    /// and `node.role` are properties an orchestrator assigns, and
    /// there is none here to have assigned them - so they are refused
    /// by name rather than treated as never matching.
    pub const KEYS: [&str; 3] = ["node.hostname", "node.platform.os", "node.platform.arch"];

    pub fn parse(s: &str) -> Result<Self, String> {
        let (key, equal, value) = if let Some((k, v)) = s.split_once("!=") {
            (k, false, v)
        } else if let Some((k, v)) = s.split_once("==") {
            (k, true, v)
        } else {
            return Err("expected `key == value` or `key != value`".into());
        };
        let (key, value) = (key.trim(), value.trim());
        if !Self::KEYS.contains(&key) {
            return Err(format!(
                "{key:?} is not something one machine can answer. Supported: {}",
                Self::KEYS.join(", ")
            ));
        }
        if value.is_empty() {
            return Err("the value is empty".into());
        }
        Ok(Self {
            key: key.to_owned(),
            equal,
            value: value.to_owned(),
        })
    }

    /// Does this hold for a node with these properties?
    pub fn holds(&self, hostname: &str, os: &str, arch: &str) -> bool {
        let actual = match self.key.as_str() {
            "node.hostname" => hostname,
            "node.platform.os" => os,
            _ => arch,
        };
        // Docker reports `x86_64` and `aarch64`; people also write the
        // image-platform spellings, and a constraint that silently never
        // matched because of that would be a bad afternoon.
        let same = |a: &str, b: &str| {
            let norm = |x: &str| match x {
                "amd64" => "x86_64".to_owned(),
                "arm64" => "aarch64".to_owned(),
                other => other.to_lowercase(),
            };
            norm(a) == norm(b)
        };
        same(actual, &self.value) == self.equal
    }
}

/// One watched path.
///
/// Compose's `develop.watch`, all three actions. `sync` was refused
/// for a long time, because copying into a running container needs a
/// way to write into its filesystem from outside that a container
/// cannot redirect; the confined path walk built for the file browser
/// is that way, so it is here now.
#[derive(Debug, Clone, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Watch {
    /// Relative to the stack file, like everything else here.
    pub path: String,
    /// `rebuild` (the default) builds the image again and replaces the
    /// container; `restart` replaces the container without rebuilding,
    /// which is what a changed config file needs.
    /// `sync` copies changed files into the running container at
    /// `target` and restarts nothing.
    #[serde(default = "rebuild")]
    pub action: String,
    /// Where in the container `path` goes. Required by `sync`, and
    /// meaningless to the other two.
    #[serde(default)]
    pub target: Option<String>,
}

fn rebuild() -> String {
    "rebuild".into()
}

/// What makes a service ready.
///
/// Evaluated by `up` while it waits, not by a supervisor running a
/// probe loop for the life of the container. That is the smaller half
/// of what Compose's healthchecks do and it is the half `depends_on`
/// actually needs: the question being answered is "may I start the next
/// service yet", and nothing here reacts to a container becoming
/// unhealthy later.
#[derive(Debug, Clone, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct HealthCheck {
    /// The command, as a list. Exit 0 means healthy.
    pub test: Vec<String>,
    /// Seconds between attempts. 1 by default.
    #[serde(default = "one")]
    pub interval: u64,
    /// How many attempts before giving up. 30 by default.
    #[serde(default = "thirty")]
    pub retries: u64,
    /// Seconds to wait before the first attempt. 0 by default.
    #[serde(default)]
    pub start_period: u64,
}

fn one() -> u64 {
    1
}
fn one_replica() -> u32 {
    1
}
fn thirty() -> u64 {
    30
}

impl Stack {
    /// One document. `parse_many` is what the CLI uses, because `-f` is
    /// repeatable; this is the single-file shorthand the tests read
    /// better with, and it goes through exactly the same path.
    #[cfg(test)]
    pub fn parse(text: &str) -> Result<Self> {
        Self::parse_many(std::slice::from_ref(&text.to_owned()))
    }

    /// Deserialize and validate an already-merged document.
    fn from_value(doc: toml::Value) -> Result<Self> {
        let stack: Stack = doc.try_into().map_err(|e: toml::de::Error| {
            let m = e.to_string();
            // serde's own message for this is "invalid type: string",
            // which is true and useless. The reader needs the shape.
            if m.contains("invalid type: string") && m.contains("command") {
                anyhow::anyhow!(
                    "{m}\n\n`command` is a list of arguments, not a string:\n    \
                     command = [\"/bin/sh\", \"-c\", \"echo hi; sleep 1\"]\n\n\
                     A string would have to be split on something, and splitting on \
                     spaces turns the quotes in `sh -c \"a; b\"` into part of the \
                     arguments."
                )
            } else {
                anyhow::anyhow!(m)
            }
        })?;
        if stack.name.is_empty()
            || !stack
                .name
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
        {
            bail!(
                "{:?} is not a usable stack name: it becomes part of every \
                 container's name, so it must be letters, digits, `-` or `_`",
                stack.name
            );
        }
        if stack.services.is_empty() {
            bail!("the stack has no services, so there is nothing to start");
        }
        for (name, svc) in &stack.services {
            if svc.image.trim().is_empty() && svc.build.is_none() {
                bail!(
                    "service {name:?} has neither `image` nor `build`, so there is \
                     nothing to run. Give it an image to pull, or a `build` context \
                     to build one from."
                );
            }
            if let Some(r) = &svc.restart {
                // Parsed here rather than at start time: a stack file with
                // a typo in it should be refused before half of it is
                // running.
                crate::supervise_restart_check(r)
                    .with_context(|| format!("service {name:?}: `restart`"))?;
            }
            svc.depends_on
                .validate()
                .with_context(|| format!("service {name:?}: `depends_on`"))?;
            if let Some(h) = &svc.healthcheck
                && h.test.is_empty()
            {
                bail!("service {name:?} has a `healthcheck` with no `test` command");
            }
            for c in &svc.configs {
                if !stack.configs.contains_key(c) {
                    bail!(
                        "service {name:?} wants config {c:?}, which this stack does not \
                         define. Add `[configs.{c}]` with a `file`."
                    );
                }
            }
            for c in &svc.secrets {
                if !stack.secrets.contains_key(c) {
                    bail!(
                        "service {name:?} wants secret {c:?}, which this stack does not \
                         define. Add `[secrets.{c}]` with a `file`."
                    );
                }
            }
            for w in &svc.watch {
                if !["rebuild", "restart", "sync"].contains(&w.action.as_str()) {
                    bail!(
                        "service {name:?}: {:?} is not a watch action. Use \"rebuild\", \
                         \"restart\" or \"sync\".",
                        w.action
                    );
                }
                if w.action == "sync" && !w.target.as_deref().is_some_and(|t| t.starts_with('/')) {
                    bail!(
                        "service {name:?} watches {:?} with action \"sync\", which needs \
                         a `target`: the absolute path in the container to copy into.",
                        w.path
                    );
                }
                if w.action == "rebuild" && svc.build.is_none() {
                    bail!(
                        "service {name:?} watches {:?} with action \"rebuild\", but the \
                         service has no `build` to rebuild. Use \"restart\", or give \
                         it a build context.",
                        w.path
                    );
                }
            }
            for c in &svc.placement {
                if let Err(e) = Constraint::parse(c) {
                    bail!("service {name:?}: placement constraint {c:?}: {e}");
                }
            }
            if svc.replicas == 0 {
                bail!(
                    "service {name:?} asks for 0 replicas. Remove the service, or \
                     put it behind a profile, rather than defining one that never runs."
                );
            }
            for dep in svc.depends_on.names() {
                if svc.depends_on.wants_healthy(dep)
                    && stack
                        .services
                        .get(dep)
                        .is_some_and(|d| d.healthcheck.is_none())
                {
                    bail!(
                        "service {name:?} waits for {dep:?} to be healthy, but {dep:?} \
                         has no `healthcheck`, so nothing would ever report it healthy. \
                         Give {dep:?} a healthcheck, or depend on it being started."
                    );
                }
                if !stack.services.contains_key(dep) {
                    bail!(
                        "service {name:?} depends on {dep:?}, which this stack does not \
                         define. The services it does define are: {}",
                        stack
                            .services
                            .keys()
                            .cloned()
                            .collect::<Vec<_>>()
                            .join(", ")
                    );
                }
            }
        }
        Ok(stack)
    }

    /// Merge several stack documents, later over earlier.
    ///
    /// One mechanism serves two features: several `-f` files, and
    /// `extends`. Both are "take that table and override parts of it",
    /// and writing it twice would be writing the same subtle rule twice
    /// - tables merge key by key, everything else replaces.
    ///
    /// A list REPLACES rather than appends. Appending looks friendlier
    /// and is a trap: an override file that sets `command` would append
    /// to the base command and run something nobody wrote.
    fn merge(base: &mut toml::Value, over: &toml::Value) {
        match (base, over) {
            (toml::Value::Table(b), toml::Value::Table(o)) => {
                for (k, v) in o {
                    match b.get_mut(k) {
                        Some(existing) => Self::merge(existing, v),
                        None => {
                            b.insert(k.clone(), v.clone());
                        }
                    }
                }
            }
            (b, o) => *b = o.clone(),
        }
    }

    /// Parse one or more documents as a single stack.
    ///
    /// Later documents override earlier ones, which is what `-f a -f b`
    /// means everywhere else. `extends` is resolved afterwards, on the
    /// merged result, so a service can extend one defined in a different
    /// file.
    pub fn parse_many(texts: &[String]) -> Result<Self> {
        let mut merged: Option<toml::Value> = None;
        for (n, t) in texts.iter().enumerate() {
            let v: toml::Value = toml::from_str(t)
                .map_err(|e| anyhow::anyhow!("file {} is not valid TOML: {e}", n + 1))?;
            match &mut merged {
                None => merged = Some(v),
                Some(m) => Self::merge(m, &v),
            }
        }
        let mut merged = merged.unwrap_or_else(|| toml::Value::Table(Default::default()));
        Self::resolve_extends(&mut merged)?;
        Self::from_value(merged)
    }

    /// Fold every `extends` into the service that declares it.
    ///
    /// Depth first, so a chain works, with the visit set carrying the
    /// cycle: `a extends b extends a` is a stack file that cannot be
    /// resolved and saying which services are in the loop is the useful
    /// half of the message.
    fn resolve_extends(doc: &mut toml::Value) -> Result<()> {
        let Some(services) = doc.get("services").and_then(|s| s.as_table()).cloned() else {
            return Ok(());
        };
        let mut resolved: BTreeMap<String, toml::Value> = BTreeMap::new();
        for name in services.keys() {
            let mut seen: Vec<String> = Vec::new();
            let v = Self::resolve_one(name, &services, &mut seen, &mut resolved)?;
            resolved.insert(name.clone(), v);
        }
        let table = doc.as_table_mut().expect("the document is a table");
        table.insert(
            "services".into(),
            toml::Value::Table(resolved.into_iter().collect()),
        );
        Ok(())
    }

    fn resolve_one(
        name: &str,
        services: &toml::map::Map<String, toml::Value>,
        seen: &mut Vec<String>,
        done: &mut BTreeMap<String, toml::Value>,
    ) -> Result<toml::Value> {
        if let Some(v) = done.get(name) {
            return Ok(v.clone());
        }
        if seen.iter().any(|s| s == name) {
            seen.push(name.to_owned());
            bail!(
                "`extends` goes in a circle and cannot be resolved: {}",
                seen.join(" -> ")
            );
        }
        let Some(me) = services.get(name) else {
            bail!(
                "a service extends {name:?}, which this stack does not define. \
                 The services it does define are: {}",
                services.keys().cloned().collect::<Vec<_>>().join(", ")
            );
        };
        let Some(base_name) = me.get("extends").and_then(|v| v.as_str()) else {
            return Ok(me.clone());
        };
        if base_name == name {
            bail!("service {name:?} extends itself");
        }
        seen.push(name.to_owned());
        let mut base = Self::resolve_one(base_name, services, seen, done)?;
        seen.pop();
        Self::merge(&mut base, me);
        // The marker is spent: leaving it in would be harmless but would
        // let `rustainer stack ps` print a field that no longer means
        // anything.
        if let Some(t) = base.as_table_mut() {
            t.remove("extends");
        }
        Ok(base)
    }

    /// Expand `${VAR}` and `${VAR:-default}` before the TOML is parsed.
    ///
    /// Values come from `vars` - which `read_stack` fills from a `.env`
    /// beside the stack file and then from the process environment, so
    /// the environment wins, as it does in Compose.
    ///
    /// **An undefined variable with no default is an error**, not an
    /// empty string. Compose substitutes empty and warns; the result is
    /// `ports = [":80"]` and a stack that fails somewhere else entirely.
    /// `${VAR:-}` is how you ask for empty on purpose.
    pub fn interpolate(text: &str, vars: &BTreeMap<String, String>) -> Result<String> {
        let mut out = String::with_capacity(text.len());
        let b = text.as_bytes();
        let mut i = 0;
        while i < b.len() {
            // `$$` is a literal `$`, so a password that contains ${ can
            // still be written down.
            if b[i] == b'$' && i + 1 < b.len() && b[i + 1] == b'$' {
                out.push('$');
                i += 2;
                continue;
            }
            if b[i] == b'$' && i + 1 < b.len() && b[i + 1] == b'{' {
                let Some(end) = text[i + 2..].find('}').map(|e| i + 2 + e) else {
                    bail!("`${{` at byte {i} is never closed with `}}`");
                };
                let body = &text[i + 2..end];
                let (name, default) = match body.split_once(":-") {
                    Some((n, d)) => (n, Some(d)),
                    None => (body, None),
                };
                let name = name.trim();
                if name.is_empty() {
                    bail!("`${{}}` has no variable name in it");
                }
                match vars.get(name) {
                    Some(v) => out.push_str(v),
                    None => match default {
                        Some(d) => out.push_str(d),
                        None => bail!(
                            "{name} is not set, and `${{{name}}}` has no default. \
                             Set it in the environment or in a .env file beside the \
                             stack file, or write `${{{name}:-something}}`."
                        ),
                    },
                }
                i = end + 1;
                continue;
            }
            let ch = text[i..].chars().next().unwrap();
            out.push(ch);
            i += ch.len_utf8();
        }
        Ok(out)
    }

    /// Parse a `.env` file: `KEY=value` per line, `#` comments, blanks
    /// ignored. Deliberately not a shell: no `export`, no substitution,
    /// no quoting rules to get wrong.
    pub fn parse_dotenv(text: &str) -> Result<BTreeMap<String, String>> {
        let mut out = BTreeMap::new();
        for (n, line) in text.lines().enumerate() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let Some((k, v)) = line.split_once('=') else {
                bail!("line {} of the .env file is not KEY=value: {line:?}", n + 1);
            };
            // One layer of surrounding quotes is removed, because every
            // .env file in the world has them and leaving them in makes
            // a password that silently includes a quote character.
            let v = v.trim();
            let v = v
                .strip_prefix('"')
                .and_then(|r| r.strip_suffix('"'))
                .or_else(|| v.strip_prefix('\'').and_then(|r| r.strip_suffix('\'')))
                .unwrap_or(v);
            out.insert(k.trim().to_owned(), v.to_owned());
        }
        Ok(out)
    }

    /// The services that `--profile` selects.
    ///
    /// A service with no profiles is always in. A service with profiles
    /// is in only when one of them was asked for. Dependencies are NOT
    /// pulled in implicitly: a service that is left out while something
    /// that depends on it is kept is an error, because starting it
    /// anyway would quietly ignore the profile and leaving it out would
    /// break the dependant.
    pub fn select_profiles(&mut self, enabled: &[String]) -> Result<()> {
        let keep: BTreeSet<String> = self
            .services
            .iter()
            .filter(|(_, s)| {
                s.profiles.is_empty() || s.profiles.iter().any(|p| enabled.contains(p))
            })
            .map(|(n, _)| n.clone())
            .collect();
        for (name, svc) in &self.services {
            if !keep.contains(name) {
                continue;
            }
            for dep in svc.depends_on.names() {
                if !keep.contains(dep) {
                    bail!(
                        "service {name:?} is in this run but depends on {dep:?}, which \
                         its profiles leave out. Add {dep:?} to a profile you asked \
                         for, or give {name:?} the same profiles it has."
                    );
                }
            }
        }
        self.services.retain(|n, _| keep.contains(n));
        if self.services.is_empty() {
            bail!(
                "no service is in any of the profiles you asked for, so there is \
                 nothing to start"
            );
        }
        Ok(())
    }

    /// The container name for a service: `<stack>-<service>`.
    ///
    /// Flat and predictable on purpose. These are ordinary containers, and
    /// `rustainer ps`, `logs`, `exec` and the page all work on them
    /// without knowing a stack exists.
    pub fn container_name(&self, service: &str) -> String {
        format!("{}-{}", self.name, service)
    }

    /// Every container name for a service, which is one name unless it
    /// asked for replicas.
    ///
    /// A single replica keeps the unsuffixed name on purpose: adding
    /// `-1` to every existing stack's containers would rename things
    /// people have in scripts, for the benefit of a feature they are
    /// not using.
    pub fn container_names(&self, service: &str) -> Vec<String> {
        let n = self.services.get(service).map(|s| s.replicas).unwrap_or(1);
        if n <= 1 {
            vec![self.container_name(service)]
        } else {
            (1..=n)
                .map(|i| format!("{}-{}-{}", self.name, service, i))
                .collect()
        }
    }

    /// Start order: dependencies first, ties broken alphabetically so the
    /// same file always produces the same order.
    ///
    /// A cycle is named rather than reported as "a cycle exists", because
    /// the useful part of the message is which services are in it.
    /// The same graph, grouped into levels: every service in a level is
    /// independent of every other one in it.
    ///
    /// `start_order` flattens the graph into a single sequence and is
    /// kept as it is, because the sequence it produces is the one `up`
    /// has always used and a test pins it. This exists for `down`, where
    /// the useful fact is not the order but the INDEPENDENCE: services
    /// in one level have no reason to wait for each other, and stopping
    /// them one at a time multiplies the grace period by their number.
    /// Three services that ignore SIGTERM at a five second grace is
    /// fifteen seconds of waiting for no reason.
    pub fn start_levels(&self) -> Result<Vec<Vec<String>>> {
        let mut levels: Vec<Vec<String>> = Vec::new();
        let mut placed: BTreeSet<String> = BTreeSet::new();
        while placed.len() < self.services.len() {
            let ready: Vec<String> = self
                .services
                .iter()
                .filter(|(n, s)| {
                    !placed.contains(*n) && s.depends_on.names().iter().all(|d| placed.contains(*d))
                })
                .map(|(n, _)| n.clone())
                .collect();
            if ready.is_empty() {
                let stuck: Vec<&str> = self
                    .services
                    .keys()
                    .filter(|n| !placed.contains(*n))
                    .map(String::as_str)
                    .collect();
                bail!(
                    "these services depend on each other in a cycle, so none of them can \
                     be started first: {}. Break the cycle by removing a `depends_on`.",
                    stuck.join(", ")
                );
            }
            for n in &ready {
                placed.insert(n.clone());
            }
            levels.push(ready);
        }
        Ok(levels)
    }

    pub fn start_order(&self) -> Result<Vec<String>> {
        let mut done: Vec<String> = Vec::new();
        let mut placed: BTreeSet<String> = BTreeSet::new();

        // Kahn's algorithm, taking the alphabetically first ready service
        // each round. O(n²) over services, and a stack with enough
        // services for that to matter has a different problem.
        while placed.len() < self.services.len() {
            let ready: Vec<String> = self
                .services
                .iter()
                .filter(|(n, s)| {
                    !placed.contains(*n) && s.depends_on.names().iter().all(|d| placed.contains(*d))
                })
                .map(|(n, _)| n.clone())
                .collect();
            let Some(next) = ready.into_iter().next() else {
                let stuck: Vec<&str> = self
                    .services
                    .keys()
                    .filter(|n| !placed.contains(*n))
                    .map(String::as_str)
                    .collect();
                bail!(
                    "these services depend on each other in a cycle, so none of them can \
                     be started first: {}. Break the cycle by removing a `depends_on`.",
                    stuck.join(", ")
                );
            };
            placed.insert(next.clone());
            done.push(next);
        }
        Ok(done)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const EXAMPLE: &str = r#"
name = "blog"

[services.db]
image = "postgres:17-alpine"
env = ["POSTGRES_PASSWORD=dev"]
volumes = ["blogdb:/var/lib/postgresql/data"]

[services.web]
image = "nginx:1.27-alpine"
ports = ["8080:80"]
depends_on = ["db"]
"#;

    #[test]
    fn a_stack_parses_and_orders_its_services() {
        let s = Stack::parse(EXAMPLE).unwrap();
        assert_eq!(s.name, "blog");
        assert_eq!(s.start_order().unwrap(), vec!["db", "web"]);
        assert_eq!(s.container_name("web"), "blog-web");
        assert_eq!(
            s.services["db"].volumes,
            ["blogdb:/var/lib/postgresql/data"]
        );
        assert!(s.services["db"].command.is_empty());
    }

    #[test]
    fn a_string_command_is_refused_with_the_shape_that_works() {
        // Compose accepts a string here and splits it on spaces, so the
        // quotes in `sh -c "a; b"` become part of the arguments and the
        // shell is handed `"a;` as its whole script.
        let e =
            Stack::parse("name=\"x\"\n[services.a]\nimage=\"i\"\ncommand=\"/bin/sh -c 'a; b'\"\n")
                .unwrap_err();
        let m = e.to_string();
        assert!(m.contains("list of arguments"), "{m}");
        assert!(m.contains("command = ["), "{m}");
    }

    #[test]
    fn a_misspelled_key_is_refused_rather_than_ignored() {
        // A service that starts without the setting you thought you gave
        // it is the most expensive kind of typo, so an unknown key is an
        // error and the error names the key.
        //
        // Not a point of difference, though: measured against Compose
        // v2.40 with scripts/stack-bench.sh, `docker compose` refuses an
        // unknown service key too. An earlier version of this comment
        // claimed otherwise. Parity, and worth knowing rather than
        // assuming.
        let e = Stack::parse("name=\"x\"\n[services.a]\nimgae=\"alpine\"\n").unwrap_err();
        assert!(e.to_string().contains("imgae"), "{e}");
    }

    #[test]
    fn independent_services_land_in_one_level() {
        // What `down` relies on: services with no dependency between
        // them are stopped at the same time, so the grace period is
        // paid once rather than once per service.
        let s = Stack::parse(
            "name=\"x\"\n[services.a]\nimage=\"i\"\n[services.b]\nimage=\"i\"\n\
             [services.c]\nimage=\"i\"\ndepends_on=[\"a\"]\n",
        )
        .unwrap();
        let levels = s.start_levels().unwrap();
        assert_eq!(levels, vec![vec!["a", "b"], vec!["c"]]);
        // And every service appears exactly once, in a level after
        // everything it depends on.
        let flat: Vec<String> = levels.into_iter().flatten().collect();
        assert_eq!(flat.len(), 3);
        assert!(
            flat.iter().position(|n| n == "a") < flat.iter().position(|n| n == "c"),
            "{flat:?}"
        );
    }

    fn vars(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
            .collect()
    }

    #[test]
    fn interpolation_substitutes_defaults_and_refuses_the_undefined() {
        let v = vars(&[("TAG", "3.21")]);
        assert_eq!(
            Stack::interpolate("image = \"alpine:${TAG}\"", &v).unwrap(),
            "image = \"alpine:3.21\""
        );
        assert_eq!(
            Stack::interpolate("x = \"${MISSING:-fallback}\"", &v).unwrap(),
            "x = \"fallback\""
        );
        // Empty on purpose is spelled out, so it cannot happen by accident.
        assert_eq!(
            Stack::interpolate("x = \"${M:-}\"", &v).unwrap(),
            "x = \"\""
        );
        // `$$` is a literal dollar.
        assert_eq!(
            Stack::interpolate("x = \"a$$b\"", &v).unwrap(),
            "x = \"a$b\""
        );

        // The one that matters: Compose substitutes empty and warns, which
        // turns `${PORT}:80` into `:80` and fails somewhere else entirely.
        let e = Stack::interpolate("ports = [\"${PORT}:80\"]", &v).unwrap_err();
        assert!(e.to_string().contains("PORT is not set"), "{e}");
        let e = Stack::interpolate("x = \"${UNCLOSED\"", &v).unwrap_err();
        assert!(e.to_string().contains("never closed"), "{e}");
    }

    #[test]
    fn dotenv_is_not_a_shell() {
        let m = Stack::parse_dotenv("# a comment\n\nTAG=3.21\nQUOTED=\"a b\"\n").unwrap();
        assert_eq!(m.get("TAG").unwrap(), "3.21");
        assert_eq!(m.get("QUOTED").unwrap(), "a b");
        assert!(Stack::parse_dotenv("NOTKEYVALUE\n").is_err());
    }

    #[test]
    fn profiles_select_services_and_catch_a_broken_dependency() {
        let text = "name=\"x\"\n\
                    [services.web]\nimage=\"i\"\n\
                    [services.debugger]\nimage=\"i\"\nprofiles=[\"debug\"]\n";
        // Unprofiled services are always in; profiled ones are not.
        let mut s = Stack::parse(text).unwrap();
        s.select_profiles(&[]).unwrap();
        assert_eq!(s.services.keys().collect::<Vec<_>>(), vec!["web"]);

        let mut s = Stack::parse(text).unwrap();
        s.select_profiles(&["debug".into()]).unwrap();
        assert_eq!(s.services.len(), 2);

        // A kept service depending on an excluded one is refused rather
        // than started without it.
        let mut s = Stack::parse(
            "name=\"x\"\n[services.web]\nimage=\"i\"\ndepends_on=[\"db\"]\n\
             [services.db]\nimage=\"i\"\nprofiles=[\"extra\"]\n",
        )
        .unwrap();
        let e = s.select_profiles(&[]).unwrap_err();
        assert!(e.to_string().contains("profiles leave out"), "{e}");
    }

    #[test]
    fn a_service_needs_an_image_or_a_build() {
        let e = Stack::parse("name=\"x\"\n[services.a]\n").unwrap_err();
        assert!(e.to_string().contains("neither `image` nor `build`"), "{e}");
        // `build` alone is enough: the tag is derived from the names.
        let s = Stack::parse("name=\"x\"\n[services.a]\nbuild=\".\"\n").unwrap();
        assert_eq!(s.services["a"].build.as_deref(), Some("."));
    }

    #[test]
    fn a_bad_restart_policy_is_refused_at_parse_time() {
        // Before anything starts, not when that one service does.
        let e = Stack::parse("name=\"x\"\n[services.a]\nimage=\"i\"\nrestart=\"sometimes\"\n")
            .unwrap_err();
        assert!(e.to_string().contains("restart"), "{e}");
    }

    #[test]
    fn several_files_merge_later_over_earlier() {
        let base = "name=\"x\"\n[services.web]\nimage=\"base\"\n\
                    command=[\"a\",\"b\"]\nenv=[\"K=1\"]\n"
            .to_owned();
        let over = "[services.web]\nimage=\"prod\"\n[services.extra]\nimage=\"i\"\n".to_owned();
        let s = Stack::parse_many(&[base, over]).unwrap();
        assert_eq!(s.services["web"].image, "prod");
        // A key the override did not mention survives.
        assert_eq!(s.services["web"].env, vec!["K=1"]);
        // A service only the override defines is added.
        assert!(s.services.contains_key("extra"));
    }

    #[test]
    fn an_override_replaces_a_list_rather_than_appending_to_it() {
        // Appending looks friendlier and would make an override that
        // sets `command` run something nobody wrote.
        let base = "name=\"x\"\n[services.a]\nimage=\"i\"\ncommand=[\"one\"]\n".to_owned();
        let over = "[services.a]\ncommand=[\"two\"]\n".to_owned();
        let s = Stack::parse_many(&[base, over]).unwrap();
        assert_eq!(s.services["a"].command, vec!["two"]);
    }

    #[test]
    fn extends_inherits_then_overrides() {
        let s = Stack::parse(
            "name=\"x\"\n[services.base]\nimage=\"i\"\nenv=[\"A=1\"]\nuser=\"nobody\"\n\
             [services.child]\nextends=\"base\"\nuser=\"root\"\n",
        )
        .unwrap();
        assert_eq!(s.services["child"].image, "i");
        assert_eq!(s.services["child"].env, vec!["A=1"]);
        assert_eq!(s.services["child"].user.as_deref(), Some("root"));
        // The marker is spent once it is resolved.
        assert_eq!(s.services["child"].extends, None);
    }

    #[test]
    fn extends_in_a_circle_names_the_loop() {
        let e = Stack::parse(
            "name=\"x\"\n[services.a]\nimage=\"i\"\nextends=\"b\"\n\
             [services.b]\nimage=\"i\"\nextends=\"a\"\n",
        )
        .unwrap_err();
        let m = e.to_string();
        assert!(m.contains("circle"), "{m}");
        assert!(m.contains("->"), "{m}");
    }

    #[test]
    fn depends_on_takes_both_shapes() {
        let s = Stack::parse(
            "name=\"x\"\n[services.db]\nimage=\"i\"\n\
             [services.db.healthcheck]\ntest=[\"true\"]\n\
             [services.web]\nimage=\"i\"\ndepends_on={db=\"healthy\"}\n",
        )
        .unwrap();
        assert_eq!(s.services["web"].depends_on.names(), vec!["db"]);
        assert!(s.services["web"].depends_on.wants_healthy("db"));
        // The list form still parses, and means "started".
        let s = Stack::parse(
            "name=\"x\"\n[services.db]\nimage=\"i\"\n\
             [services.web]\nimage=\"i\"\ndepends_on=[\"db\"]\n",
        )
        .unwrap();
        assert!(!s.services["web"].depends_on.wants_healthy("db"));
    }

    #[test]
    fn waiting_for_health_needs_something_that_reports_it() {
        // The trap: `condition: service_healthy` against a service with
        // no healthcheck waits forever in Compose. Refused here.
        let e = Stack::parse(
            "name=\"x\"\n[services.db]\nimage=\"i\"\n\
             [services.web]\nimage=\"i\"\ndepends_on={db=\"healthy\"}\n",
        )
        .unwrap_err();
        assert!(e.to_string().contains("has no `healthcheck`"), "{e}");

        let e = Stack::parse(
            "name=\"x\"\n[services.db]\nimage=\"i\"\n\
             [services.web]\nimage=\"i\"\ndepends_on={db=\"whenever\"}\n",
        )
        .unwrap_err();
        // `{:#}`, not `{}`: anyhow's Display shows only the outermost
        // context, and the sentence worth asserting on is the innermost.
        assert!(format!("{e:#}").contains("is not a condition"), "{e:#}");
    }

    #[test]
    fn one_replica_keeps_the_plain_name() {
        // Adding `-1` to every existing stack's containers would rename
        // things people have in scripts, for a feature they are not
        // using.
        let s = Stack::parse("name=\"x\"\n[services.web]\nimage=\"i\"\n").unwrap();
        assert_eq!(s.container_names("web"), vec!["x-web"]);

        let s = Stack::parse("name=\"x\"\n[services.web]\nimage=\"i\"\nreplicas=3\n").unwrap();
        assert_eq!(
            s.container_names("web"),
            vec!["x-web-1", "x-web-2", "x-web-3"]
        );
    }

    #[test]
    fn zero_replicas_is_refused_rather_than_silently_nothing() {
        let e = Stack::parse("name=\"x\"\n[services.web]\nimage=\"i\"\nreplicas=0\n").unwrap_err();
        assert!(e.to_string().contains("0 replicas"), "{e}");
    }

    #[test]
    fn a_service_cannot_ask_for_a_config_the_stack_does_not_define() {
        let e = Stack::parse("name=\"x\"\n[services.a]\nimage=\"i\"\nconfigs=[\"nginx\"]\n")
            .unwrap_err();
        assert!(e.to_string().contains("wants config"), "{e}");
        let e =
            Stack::parse("name=\"x\"\n[services.a]\nimage=\"i\"\nsecrets=[\"db\"]\n").unwrap_err();
        assert!(e.to_string().contains("wants secret"), "{e}");
    }

    #[test]
    fn configs_and_secrets_parse_and_attach() {
        let s = Stack::parse(
            "name=\"x\"\n[configs.site]\nfile=\"nginx.conf\"\n\
             [secrets.dbpass]\nfile=\"secret.txt\"\n\
             [services.a]\nimage=\"i\"\nconfigs=[\"site\"]\nsecrets=[\"dbpass\"]\n",
        )
        .unwrap();
        assert_eq!(s.configs["site"].file, "nginx.conf");
        assert_eq!(s.secrets["dbpass"].file, "secret.txt");
        assert_eq!(s.services["a"].configs, vec!["site"]);
        assert_eq!(s.services["a"].secrets, vec!["dbpass"]);
    }

    #[test]
    fn placement_constraints_are_parsed_and_judged_against_one_node() {
        let c = Constraint::parse("node.platform.arch == arm64").unwrap();
        assert!(
            c.holds("box", "linux", "aarch64"),
            "arm64 and aarch64 are one architecture"
        );
        assert!(!c.holds("box", "linux", "x86_64"));
        let c = Constraint::parse("node.hostname!=build-1").unwrap();
        assert!(c.holds("box", "linux", "aarch64"));
        assert!(!c.holds("build-1", "linux", "aarch64"));
        // What only an orchestrator could know is refused by name.
        for bad in [
            "node.role == manager",
            "node.labels.zone == a",
            "node.hostname",
            "node.hostname == ",
        ] {
            assert!(Constraint::parse(bad).is_err(), "{bad:?} was accepted");
        }
        let e = Stack::parse(
            "name=\"x\"\n[services.a]\nimage=\"i\"\nplacement=[\"node.role == manager\"]\n",
        )
        .unwrap_err();
        assert!(
            e.to_string()
                .contains("not something one machine can answer"),
            "{e}"
        );
    }

    #[test]
    fn watch_actions_are_checked_against_what_the_service_can_do() {
        // An action nobody has is refused by name, and "sync" has to
        // say where in the container the files go.
        let e = Stack::parse(
            "name=\"x\"\n[services.a]\nimage=\"i\"\n\
             [[services.a.watch]]\npath=\"src\"\naction=\"mirror\"\n",
        )
        .unwrap_err();
        assert!(e.to_string().contains("not a watch action"), "{e}");
        for target in ["", "target=\"app\"\n"] {
            let e = Stack::parse(&format!(
                "name=\"x\"\n[services.a]\nimage=\"i\"\n\
                 [[services.a.watch]]\npath=\"src\"\naction=\"sync\"\n{target}"
            ))
            .unwrap_err();
            assert!(e.to_string().contains("needs a `target`"), "{e}");
        }
        let s = Stack::parse(
            "name=\"x\"\n[services.a]\nimage=\"i\"\n\
             [[services.a.watch]]\npath=\"src\"\naction=\"sync\"\ntarget=\"/app\"\n",
        )
        .unwrap();
        assert_eq!(s.services["a"].watch[0].target.as_deref(), Some("/app"));

        // "rebuild" on a service with nothing to build is a file that
        // cannot mean what it says.
        let e = Stack::parse(
            "name=\"x\"\n[services.a]\nimage=\"i\"\n\
             [[services.a.watch]]\npath=\"src\"\n",
        )
        .unwrap_err();
        assert!(e.to_string().contains("no `build` to rebuild"), "{e}");

        // And the two that work.
        let s = Stack::parse(
            "name=\"x\"\n[services.a]\nimage=\"i\"\n\
             [[services.a.watch]]\npath=\"conf\"\naction=\"restart\"\n",
        )
        .unwrap();
        assert_eq!(s.services["a"].watch[0].action, "restart");
        let s = Stack::parse(
            "name=\"x\"\n[services.a]\nbuild=\".\"\n\
             [[services.a.watch]]\npath=\"src\"\n",
        )
        .unwrap();
        assert_eq!(s.services["a"].watch[0].action, "rebuild");
    }

    #[test]
    fn a_cycle_is_refused_by_levels_too() {
        let s = Stack::parse(
            "name=\"x\"\n[services.a]\nimage=\"i\"\ndepends_on=[\"b\"]\n\
             [services.b]\nimage=\"i\"\ndepends_on=[\"a\"]\n",
        )
        .unwrap();
        let e = s.start_levels().unwrap_err();
        assert!(e.to_string().contains("cycle"), "{e}");
    }

    #[test]
    fn a_cycle_names_the_services_in_it() {
        let e = Stack::parse(
            "name=\"x\"\n\
             [services.a]\nimage=\"i\"\ndepends_on=[\"b\"]\n\
             [services.b]\nimage=\"i\"\ndepends_on=[\"a\"]\n",
        )
        .unwrap()
        .start_order()
        .unwrap_err();
        let m = e.to_string();
        assert!(m.contains("a, b"), "{m}");
        assert!(m.contains("cycle"), "{m}");
    }

    #[test]
    fn a_dependency_that_does_not_exist_lists_the_ones_that_do() {
        let e = Stack::parse(
            "name=\"x\"\n[services.a]\nimage=\"i\"\ndepends_on=[\"ghost\"]\n\
             [services.b]\nimage=\"i\"\n",
        )
        .unwrap_err();
        let m = e.to_string();
        assert!(m.contains("ghost"), "{m}");
        assert!(m.contains("a, b"), "{m}");
    }

    #[test]
    fn the_order_is_stable_for_the_same_file() {
        // Two runs of the same stack must produce the same order, or
        // "it worked yesterday" becomes a real sentence.
        let s = Stack::parse(
            "name=\"x\"\n[services.c]\nimage=\"i\"\n[services.a]\nimage=\"i\"\n\
             [services.b]\nimage=\"i\"\ndepends_on=[\"c\"]\n",
        )
        .unwrap();
        assert_eq!(s.start_order().unwrap(), vec!["a", "c", "b"]);
        assert_eq!(s.start_order().unwrap(), s.start_order().unwrap());
    }

    #[test]
    fn a_stack_with_no_services_is_refused() {
        assert!(Stack::parse("name=\"x\"\n[services]\n").is_err());
    }

    #[test]
    fn a_name_that_cannot_be_a_container_name_is_refused() {
        assert!(Stack::parse("name=\"a/b\"\n[services.a]\nimage=\"i\"\n").is_err());
        assert!(Stack::parse("name=\"\"\n[services.a]\nimage=\"i\"\n").is_err());
    }
}
