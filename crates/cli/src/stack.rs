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
    #[serde(default)]
    pub depends_on: Vec<String>,
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

impl Stack {
    pub fn parse(text: &str) -> Result<Self> {
        let stack: Stack = toml::from_str(text).map_err(|e| {
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
            for dep in &svc.depends_on {
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
            for dep in &svc.depends_on {
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
                    !placed.contains(*n) && s.depends_on.iter().all(|d| placed.contains(d))
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
                    !placed.contains(*n) && s.depends_on.iter().all(|d| placed.contains(d))
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
