//! Turning `alpine` into `docker.io/library/alpine:latest`.
//!
//! Every step of this is **convention**, not specification. The
//! distribution spec says nothing about `docker.io`, nothing about
//! `library/`, and nothing about `latest`. They are what the ecosystem
//! happens to agree on, and a client that treats them as spec will be
//! surprised by the one that disagrees.

/// A resolved image reference.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Reference {
    /// The registry host as written, e.g. `docker.io`.
    pub registry: String,
    /// The repository path, e.g. `library/alpine`.
    pub name: String,
    /// `latest` unless a tag was given.
    pub tag: String,
    /// Set when the reference pinned a digest instead of a tag.
    pub digest: Option<String>,
}

impl Reference {
    /// The host to actually connect to.
    ///
    /// `docker.io` is **not** a registry: it serves a web page. The API
    /// lives at `registry-1.docker.io`, and every client hardcodes the
    /// remap. Nothing in the specification says so.
    pub fn endpoint(&self) -> String {
        match self.registry.as_str() {
            "docker.io" | "index.docker.io" => "registry-1.docker.io".into(),
            other => other.into(),
        }
    }

    pub fn base_url(&self) -> String {
        let host = self.endpoint();
        // Plain HTTP only for a loopback registry, which is how local test
        // registries are run. Everything else is TLS, with no flag to turn
        // that off: a flag like that is only ever used by accident.
        if host.starts_with("localhost:") || host.starts_with("127.0.0.1:") {
            format!("http://{host}")
        } else {
            format!("https://{host}")
        }
    }

    /// What to ask the registry for: a digest if we were given one.
    pub fn manifest_ref(&self) -> &str {
        self.digest.as_deref().unwrap_or(&self.tag)
    }

    /// A filesystem-safe form, for the tag index.
    pub fn store_key(&self) -> String {
        format!("{}_{}_{}", self.registry, self.name, self.manifest_ref())
            .replace(['/', ':', '@'], "_")
    }

    pub fn parse(s: &str) -> Result<Self, String> {
        let s = s.trim();
        if s.is_empty() {
            return Err("empty image reference".into());
        }

        let (rest, digest) = match s.split_once('@') {
            Some((r, d)) => {
                if !d.starts_with("sha256:") || d.len() != 71 {
                    return Err(format!(
                        "{d:?} is not a sha256 digest. Only sha256 is implemented; the spec \
                         allows other algorithms and nothing in practice uses them."
                    ));
                }
                (r, Some(d.to_owned()))
            }
            None => (s, None),
        };

        // The first component is a registry only if it looks like a host:
        // it contains a dot or a colon, or it is exactly `localhost`. That
        // heuristic is the whole reason `alpine` and `myhost.com/alpine`
        // can be told apart, and it is convention, not specification.
        let (registry, remainder) = match rest.split_once('/') {
            Some((first, tail))
                if first.contains('.') || first.contains(':') || first == "localhost" =>
            {
                (first.to_owned(), tail.to_owned())
            }
            _ => ("docker.io".to_owned(), rest.to_owned()),
        };

        let (name, tag) = match remainder.rsplit_once(':') {
            // A colon in the last path component is a tag; a colon earlier
            // belongs to a port, which the registry split already took.
            Some((n, t)) if !t.contains('/') => (n.to_owned(), t.to_owned()),
            _ => (remainder.clone(), "latest".to_owned()),
        };

        if name.is_empty() {
            return Err(format!("{s:?} has no repository name"));
        }

        // Docker Hub's official images live under `library/`, and the
        // single-component form is expanded client-side. Only Hub does this.
        let name =
            if (registry == "docker.io" || registry == "index.docker.io") && !name.contains('/') {
                format!("library/{name}")
            } else {
                name
            };

        Ok(Self {
            registry,
            name,
            tag,
            digest,
        })
    }
}

impl std::fmt::Display for Reference {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match &self.digest {
            Some(d) => write!(f, "{}/{}@{}", self.registry, self.name, d),
            None => write!(f, "{}/{}:{}", self.registry, self.name, self.tag),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(s: &str) -> Reference {
        Reference::parse(s).unwrap()
    }

    #[test]
    fn a_bare_name_becomes_a_hub_official_image() {
        let r = p("alpine");
        assert_eq!(r.registry, "docker.io");
        assert_eq!(r.name, "library/alpine");
        assert_eq!(r.tag, "latest");
        assert_eq!(r.endpoint(), "registry-1.docker.io");
    }

    #[test]
    fn a_user_image_is_not_given_the_library_prefix() {
        assert_eq!(p("someone/thing").name, "someone/thing");
    }

    #[test]
    fn a_host_is_recognised_by_a_dot_a_colon_or_localhost() {
        assert_eq!(p("ghcr.io/o/r").registry, "ghcr.io");
        assert_eq!(p("localhost:5000/r").registry, "localhost:5000");
        assert_eq!(p("localhost/r").registry, "localhost");
        // No dot, no colon: a Hub user, not a host.
        assert_eq!(p("myuser/r").registry, "docker.io");
    }

    #[test]
    fn a_registry_port_is_not_mistaken_for_a_tag() {
        let r = p("localhost:5000/alpine");
        assert_eq!(r.registry, "localhost:5000");
        assert_eq!(r.name, "alpine");
        assert_eq!(r.tag, "latest");
    }

    #[test]
    fn a_tag_and_a_digest_are_both_understood() {
        assert_eq!(p("alpine:3.21").tag, "3.21");
        let d = "sha256:".to_owned() + &"a".repeat(64);
        let r = p(&format!("alpine@{d}"));
        assert_eq!(r.digest.as_deref(), Some(d.as_str()));
        assert_eq!(r.manifest_ref(), d);
    }

    #[test]
    fn a_malformed_digest_is_refused_rather_than_sent() {
        assert!(Reference::parse("alpine@sha256:short").is_err());
        assert!(Reference::parse("alpine@md5:abc").is_err());
        assert!(Reference::parse("").is_err());
    }

    #[test]
    fn only_a_loopback_registry_is_reached_over_plain_http() {
        assert!(p("alpine").base_url().starts_with("https://"));
        assert!(p("localhost:5000/a").base_url().starts_with("http://"));
        assert!(p("127.0.0.1:5000/a").base_url().starts_with("http://"));
    }
}
