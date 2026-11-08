//! The registry client: ping, challenge, token, manifest, blobs.
//!
//! Every byte that arrives is verified against the digest that asked for
//! it. The `Docker-Content-Digest` header is never trusted — it is the
//! server telling us what it thinks it sent.

use std::io::Read;

use sha2::{Digest, Sha256};

use crate::reference::Reference;

/// The full Accept list, sent on **every** manifest request.
///
/// Content negotiation does not converge: registries disagree about what
/// they return for the same tag, and at least one makes tag→digest a
/// function of `(tag, Accept)`. Sending the same explicit list every time
/// is what makes the result reproducible.
const ACCEPT: &str = "application/vnd.oci.image.index.v1+json, \
                      application/vnd.oci.image.manifest.v1+json, \
                      application/vnd.docker.distribution.manifest.list.v2+json, \
                      application/vnd.docker.distribution.manifest.v2+json";

const SCHEMA1: [&str; 2] = [
    "application/vnd.docker.distribution.manifest.v1+json",
    "application/vnd.docker.distribution.manifest.v1+prettyjws",
];

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("{0}")]
    Http(String),
    #[error(
        "{registry} returned HTTP {status} for {what}{}",
        if body.is_empty() { String::new() } else { format!(": {body}") }
    )]
    Status {
        registry: String,
        status: u16,
        what: String,
        body: String,
    },
    #[error(
        "digest mismatch for {what}: asked for {expected}, the bytes hash to {actual}. \
         The registry sent something other than what was requested."
    )]
    DigestMismatch {
        what: String,
        expected: String,
        actual: String,
    },
    #[error(
        "this image is a Docker schema 1 manifest ({media_type}), which is deprecated and \
         is not implemented. Schema 1 manifests are signed, and the signature covers a \
         rearranged form of the document - so the bytes on the wire do not hash to the \
         digest the registry reports, and verifying them the ordinary way is impossible. \
         Pull a schema 2 or OCI image instead."
    )]
    Schema1 { media_type: String },
    #[error("no manifest in the index matches {os}/{arch}")]
    NoPlatform { os: String, arch: String },
    #[error("parsing {what}: {source}")]
    Parse {
        what: String,
        #[source]
        source: serde_json::Error,
    },
}

/// What a manifest request returned, classified **by what arrived** rather
/// than by what was asked for.
///
/// The classification order has a spec basis and a CVE behind it:
/// manifests and indexes were once not self-describing, so one digest could
/// be read as either, and a changed `Content-Type` between two pulls of the
/// same bytes changed their meaning (CVE-2021-41190). Classify, then reject
/// schema 1, then verify, then parse — any other order rejects manifests a
/// major registry serves correctly.
#[derive(Debug)]
pub enum Fetched {
    Index(Index),
    Manifest(Manifest),
}

#[derive(Clone, Debug, serde::Deserialize)]
pub struct Descriptor {
    #[serde(rename = "mediaType")]
    pub media_type: String,
    pub digest: String,
    pub size: u64,
    #[serde(default)]
    pub platform: Option<Platform>,
}

#[derive(Clone, Debug, serde::Deserialize)]
pub struct Platform {
    #[serde(default)]
    pub os: String,
    #[serde(default)]
    pub architecture: String,
    #[serde(default)]
    pub variant: Option<String>,
}

#[derive(Clone, Debug, serde::Deserialize)]
pub struct Index {
    #[serde(default)]
    pub manifests: Vec<Descriptor>,
}

#[derive(Clone, Debug, serde::Deserialize)]
pub struct Manifest {
    pub config: Descriptor,
    #[serde(default)]
    pub layers: Vec<Descriptor>,
}

#[derive(Clone, Debug, serde::Deserialize)]
pub struct ImageConfig {
    #[serde(default)]
    pub rootfs: RootFs,
    #[serde(default)]
    pub config: Option<ConfigSection>,
}

#[derive(Clone, Debug, Default, serde::Deserialize)]
pub struct RootFs {
    #[serde(default, rename = "diff_ids")]
    pub diff_ids: Vec<String>,
}

#[derive(Clone, Debug, Default, serde::Deserialize)]
pub struct ConfigSection {
    #[serde(default, rename = "Env")]
    pub env: Option<Vec<String>>,
    #[serde(default, rename = "Entrypoint")]
    pub entrypoint: Option<Vec<String>>,
    #[serde(default, rename = "Cmd")]
    pub cmd: Option<Vec<String>>,
    #[serde(default, rename = "WorkingDir")]
    pub working_dir: Option<String>,
    #[serde(default, rename = "User")]
    pub user: Option<String>,
}

pub struct Client {
    agent: ureq::Agent,
    token: Option<String>,
}

impl Default for Client {
    fn default() -> Self {
        Self::new()
    }
}

impl Client {
    pub fn new() -> Self {
        Self {
            // A 401 is not an error here, it is the first half of the
            // authentication protocol - and we need its headers to find the
            // realm. With the default configuration ureq turns it into an
            // `Error` that carries no response, leaving nothing to read the
            // challenge from.
            agent: ureq::Agent::config_builder()
                .http_status_as_error(false)
                .build()
                .new_agent(),
            token: None,
        }
    }

    /// Fetch a manifest or index, verified.
    pub fn manifest(&mut self, r: &Reference) -> Result<(Fetched, String, Vec<u8>), Error> {
        let url = format!(
            "{}/v2/{}/manifests/{}",
            r.base_url(),
            r.name,
            r.manifest_ref()
        );
        let (body, content_type) = self.get(r, &url, ACCEPT, "the manifest")?;

        // 1. Classify, and reject schema 1 BEFORE hashing. Its signature
        //    covers a rearranged document, so the bytes never hash to the
        //    reported digest and a verifier that hashes first reports a
        //    mismatch instead of the real problem.
        let in_doc: serde_json::Value = serde_json::from_slice(&body).unwrap_or_default();
        let media = in_doc
            .get("mediaType")
            .and_then(|m| m.as_str())
            .unwrap_or(&content_type)
            .to_owned();
        if SCHEMA1.contains(&media.as_str())
            || in_doc.get("schemaVersion").and_then(|v| v.as_u64()) == Some(1)
        {
            return Err(Error::Schema1 { media_type: media });
        }

        // 2. Verify, if we asked by digest. A tag has nothing to check
        //    against; the digest we compute becomes the image's identity.
        let actual = sha256_hex(&body);
        if let Some(want) = &r.digest
            && *want != actual
        {
            return Err(Error::DigestMismatch {
                what: "the manifest".into(),
                expected: want.clone(),
                actual,
            });
        }
        let digest = sha256_hex(&body);

        // 3. Parse, by what the document says it is.
        let fetched = if media.contains("manifest.list") || media.contains("image.index") {
            Fetched::Index(
                serde_json::from_slice(&body).map_err(|source| Error::Parse {
                    what: "the index".into(),
                    source,
                })?,
            )
        } else {
            Fetched::Manifest(
                serde_json::from_slice(&body).map_err(|source| Error::Parse {
                    what: "the manifest".into(),
                    source,
                })?,
            )
        };
        Ok((fetched, digest, body))
    }

    /// Fetch a manifest by digest, verifying it is the one asked for.
    pub fn manifest_by_digest(&mut self, r: &Reference, digest: &str) -> Result<Manifest, Error> {
        let mut by_digest = r.clone();
        by_digest.digest = Some(digest.to_owned());
        match self.manifest(&by_digest)? {
            (Fetched::Manifest(m), _, _) => Ok(m),
            (Fetched::Index(_), _, _) => Err(Error::Parse {
                what: "a platform manifest".into(),
                source: serde::de::Error::custom("the registry returned another index"),
            }),
        }
    }

    /// Fetch a blob, verifying its digest as it streams.
    pub fn blob(&mut self, r: &Reference, digest: &str) -> Result<Vec<u8>, Error> {
        let url = format!("{}/v2/{}/blobs/{}", r.base_url(), r.name, digest);
        let (body, _) = self.get(r, &url, "*/*", "a blob")?;
        let actual = sha256_hex(&body);
        if actual != digest {
            return Err(Error::DigestMismatch {
                what: format!("blob {digest}"),
                expected: digest.to_owned(),
                actual,
            });
        }
        Ok(body)
    }

    /// One request, with lazy authentication.
    ///
    /// `GET /v2/` is skipped: `mcr.microsoft.com` never challenges and Hub
    /// always does, so handling the 401 on the real request is the only
    /// shape that covers both — and it saves a round trip.
    fn get(
        &mut self,
        r: &Reference,
        url: &str,
        accept: &str,
        what: &str,
    ) -> Result<(Vec<u8>, String), Error> {
        for attempt in 0..2 {
            let mut req = self.agent.get(url).header("Accept", accept);
            if let Some(t) = &self.token {
                req = req.header("Authorization", &format!("Bearer {t}"));
            }
            let resp = req.call().map_err(|e| Error::Http(e.to_string()))?;
            let status = resp.status().as_u16();

            if status == 401 && attempt == 0 {
                let challenge = resp
                    .headers()
                    .get("www-authenticate")
                    .and_then(|v| v.to_str().ok())
                    .unwrap_or("")
                    .to_owned();
                self.authenticate(r, &challenge)?;
                continue;
            }
            if !(200..300).contains(&status) {
                let mut body = Vec::new();
                let _ = resp.into_body().into_reader().read_to_end(&mut body);
                let body = String::from_utf8_lossy(&body).chars().take(300).collect();
                return Err(Error::Status {
                    registry: r.endpoint(),
                    status,
                    what: what.to_owned(),
                    body,
                });
            }

            let content_type = resp
                .headers()
                .get("content-type")
                .and_then(|v| v.to_str().ok())
                .unwrap_or("")
                .to_owned();
            let mut body = Vec::new();
            resp.into_body()
                .into_reader()
                .read_to_end(&mut body)
                .map_err(|e| Error::Http(e.to_string()))?;
            return Ok((body, content_type));
        }
        Err(Error::Http("authentication did not converge".into()))
    }

    fn authenticate(&mut self, r: &Reference, challenge: &str) -> Result<(), Error> {
        let realm = challenge_field(challenge, "realm").unwrap_or_else(|| {
            // mcr.microsoft.com never challenges at all; if something else
            // 401s without a realm, the registry's own /token is the only
            // sensible guess.
            format!("{}/token", r.base_url())
        });
        let service = challenge_field(challenge, "service").unwrap_or_else(|| r.endpoint());

        // The scope is synthesised, NOT echoed. A challenge's scope is not
        // reliably about the resource asked for: ghcr.io returns the
        // literal placeholder `repository:user/image:pull`, and
        // public.ecr.aws returns `scope="aws"`. A client that echoes it
        // back requests a token for a repository called `user/image`.
        let scope = format!("repository:{}:pull", r.name);
        let token_url = format!("{realm}?service={service}&scope={scope}");

        let resp = self
            .agent
            .get(&token_url)
            .call()
            .map_err(|e| Error::Http(format!("fetching a token from {realm}: {e}")))?;
        if !(200..300).contains(&resp.status().as_u16()) {
            return Err(Error::Status {
                registry: r.endpoint(),
                status: resp.status().as_u16(),
                what: "an authentication token".into(),
                body: String::new(),
            });
        }
        let mut raw = Vec::new();
        resp.into_body()
            .into_reader()
            .read_to_end(&mut raw)
            .map_err(|e| Error::Http(format!("reading the token response: {e}")))?;
        let body: serde_json::Value = serde_json::from_slice(&raw)
            .map_err(|e| Error::Http(format!("parsing the token response: {e}")))?;
        let token = body
            .get("token")
            .or_else(|| body.get("access_token"))
            .and_then(|t| t.as_str())
            .ok_or_else(|| Error::Http("the token response contained no token".into()))?;
        self.token = Some(token.to_owned());
        Ok(())
    }
}

/// Pull `name="value"` out of a `WWW-Authenticate` header.
pub fn challenge_field(challenge: &str, name: &str) -> Option<String> {
    let needle = format!("{name}=\"");
    let start = challenge.find(&needle)? + needle.len();
    let rest = &challenge[start..];
    let end = rest.find('"')?;
    Some(rest[..end].to_owned())
}

/// Pick the manifest for this machine.
///
/// Both sides are normalised before comparing, and `architecture ==
/// "unknown"` is dropped: attestation manifests use it, and a client that
/// does not skip them can select one and then fail to find any layers.
pub fn select_platform<'a>(
    index: &'a Index,
    os: &str,
    arch: &str,
) -> Result<&'a Descriptor, Error> {
    let want_arch = normalise_arch(arch);
    let found = index.manifests.iter().find(|d| {
        let Some(p) = &d.platform else { return false };
        if p.architecture == "unknown" || p.os == "unknown" {
            return false;
        }
        p.os == os && normalise_arch(&p.architecture) == want_arch
    });
    found.ok_or_else(|| Error::NoPlatform {
        os: os.to_owned(),
        arch: want_arch,
    })
}

pub fn normalise_arch(a: &str) -> String {
    match a {
        "x86_64" | "amd64" => "amd64".into(),
        "aarch64" | "arm64" => "arm64".into(),
        other => other.to_owned(),
    }
}

pub fn sha256_hex(bytes: &[u8]) -> String {
    let mut h = Sha256::new();
    h.update(bytes);
    format!("sha256:{:x}", h.finalize())
}

/// Hash a reader as it is consumed, for the uncompressed `diff_id`.
pub struct HashingReader<R> {
    inner: R,
    hasher: Sha256,
}

impl<R: Read> HashingReader<R> {
    pub fn new(inner: R) -> Self {
        Self {
            inner,
            hasher: Sha256::new(),
        }
    }
    pub fn digest(self) -> String {
        format!("sha256:{:x}", self.hasher.finalize())
    }
}

impl<R: Read> Read for HashingReader<R> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let n = self.inner.read(buf)?;
        self.hasher.update(&buf[..n]);
        Ok(n)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_challenge_is_parsed_for_realm_and_service_only() {
        let c = r#"Bearer realm="https://auth.docker.io/token",service="registry.docker.io""#;
        assert_eq!(
            challenge_field(c, "realm").as_deref(),
            Some("https://auth.docker.io/token")
        );
        assert_eq!(
            challenge_field(c, "service").as_deref(),
            Some("registry.docker.io")
        );

        // ghcr.io returns a literal placeholder scope, and public.ecr.aws
        // returns "aws". Echoing either back asks for a token to the wrong
        // repository, which is why the scope is synthesised instead.
        let ghcr = r#"Bearer realm="https://ghcr.io/token",service="ghcr.io",scope="repository:user/image:pull""#;
        assert_eq!(
            challenge_field(ghcr, "scope").as_deref(),
            Some("repository:user/image:pull"),
            "this is the placeholder we deliberately ignore"
        );
        assert_eq!(challenge_field("Bearer", "realm"), None);
    }

    #[test]
    fn the_accept_list_is_sent_in_full_and_never_varies() {
        // At least one registry makes tag -> digest a function of
        // (tag, Accept). Varying the list changes which image you get.
        for t in [
            "oci.image.index.v1+json",
            "oci.image.manifest.v1+json",
            "docker.distribution.manifest.list.v2+json",
            "docker.distribution.manifest.v2+json",
        ] {
            assert!(ACCEPT.contains(t), "{t} missing from the Accept list");
        }
    }

    #[test]
    fn architectures_are_normalised_on_both_sides() {
        assert_eq!(normalise_arch("x86_64"), "amd64");
        assert_eq!(normalise_arch("amd64"), "amd64");
        assert_eq!(normalise_arch("aarch64"), "arm64");
        assert_eq!(normalise_arch("arm64"), "arm64");
    }

    fn index_json() -> Index {
        serde_json::from_str(
            r#"{"manifests":[
                {"mediaType":"m","digest":"sha256:a","size":1,
                 "platform":{"os":"linux","architecture":"amd64"}},
                {"mediaType":"m","digest":"sha256:b","size":1,
                 "platform":{"os":"linux","architecture":"arm64"}},
                {"mediaType":"m","digest":"sha256:c","size":1,
                 "platform":{"os":"unknown","architecture":"unknown"}}
            ]}"#,
        )
        .unwrap()
    }

    #[test]
    fn platform_selection_normalises_before_comparing() {
        let idx = index_json();
        assert_eq!(
            select_platform(&idx, "linux", "x86_64").unwrap().digest,
            "sha256:a"
        );
        assert_eq!(
            select_platform(&idx, "linux", "aarch64").unwrap().digest,
            "sha256:b"
        );
    }

    #[test]
    fn an_attestation_manifest_is_never_selected() {
        // architecture "unknown" marks an attestation. Selecting one finds
        // no layers and fails much later, somewhere confusing.
        let idx = index_json();
        assert!(select_platform(&idx, "unknown", "unknown").is_err());
    }

    #[test]
    fn an_absent_platform_is_a_clear_error() {
        let idx = index_json();
        let e = select_platform(&idx, "linux", "riscv64").unwrap_err();
        assert!(e.to_string().contains("riscv64"));
    }

    #[test]
    fn the_schema1_refusal_explains_why_it_cannot_be_verified() {
        let e = Error::Schema1 {
            media_type: SCHEMA1[0].into(),
        };
        let s = e.to_string();
        assert!(s.contains("signature"), "{s}");
        assert!(s.contains("schema 2"), "{s}");
    }

    #[test]
    fn digests_are_computed_the_way_the_registry_names_them() {
        assert_eq!(
            sha256_hex(b""),
            "sha256:e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
    }

    #[test]
    fn the_hashing_reader_digests_what_it_passes_through() {
        let mut r = HashingReader::new(&b"hello"[..]);
        let mut out = Vec::new();
        std::io::copy(&mut r, &mut out).unwrap();
        assert_eq!(out, b"hello");
        assert_eq!(r.digest(), sha256_hex(b"hello"));
    }
}
