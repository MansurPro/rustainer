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

use anyhow::{Result, bail};
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
    pub image: String,
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
            if svc.image.trim().is_empty() {
                bail!("service {name:?} has no image");
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
        // The single most expensive property of Compose's format: a
        // misspelled key does nothing, silently, and the service starts
        // without the setting you thought you gave it.
        let e = Stack::parse("name=\"x\"\n[services.a]\nimgae=\"alpine\"\n").unwrap_err();
        assert!(e.to_string().contains("imgae"), "{e}");
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
