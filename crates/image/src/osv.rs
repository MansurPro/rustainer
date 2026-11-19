//! Known vulnerabilities in an image's packages, from OSV.
//!
//! Two halves, kept apart on purpose. Reading what is installed is local
//! and pure: it parses the package database the image already carries.
//! Looking those packages up is not: it sends their **names and
//! versions** to `api.osv.dev`. Nothing else leaves the machine - no
//! image name, no file contents - and nothing leaves at all unless
//! somebody runs a scan.
//!
//! What it reads: the distribution's package database (`apk`, `dpkg`),
//! and the two kinds of application dependency that leave an
//! unambiguous record on disk - Python distributions (`*.dist-info`)
//! and Node packages (`node_modules/*/package.json`). What it cannot
//! see: a statically linked binary, a Go or Rust program's compiled-in
//! modules, a vendored copy with no metadata, or anything RPM-based,
//! whose database is SQLite. An image where it finds nothing reports no
//! packages rather than no vulnerabilities.

use std::io::Read;

use serde_json::{Value, json};

/// One installed package, named the way OSV names it.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct Package {
    /// The SOURCE package where the database records one: both Alpine's
    /// and Debian's advisories are filed against the source, so asking
    /// about the binary `libssl3` finds nothing that `openssl` has.
    pub name: String,
    pub version: String,
}

/// `lib/apk/db/installed`: blank-line separated records of `K:value`.
pub fn parse_apk(db: &str) -> Vec<Package> {
    let mut out = Vec::new();
    for record in db.split("\n\n") {
        let field = |k: &str| {
            record
                .lines()
                .find_map(|l| l.strip_prefix(k))
                .map(str::to_owned)
        };
        if let (Some(name), Some(version)) = (field("o:").or(field("P:")), field("V:")) {
            out.push(Package { name, version });
        }
    }
    out.sort();
    out.dedup();
    out
}

/// `var/lib/dpkg/status`: the same shape with `Key: value`.
pub fn parse_dpkg(db: &str) -> Vec<Package> {
    let mut out = Vec::new();
    for record in db.split("\n\n") {
        let field = |k: &str| {
            record
                .lines()
                .find_map(|l| l.strip_prefix(k))
                .map(|v| v.trim().to_owned())
        };
        // A removed package leaves its record behind with its config
        // files; only "install ok installed" is actually on disk.
        if !field("Status:").is_some_and(|s| s.ends_with(" installed")) {
            continue;
        }
        let (Some(binary), Some(version)) = (field("Package:"), field("Version:")) else {
            continue;
        };
        // `Source: openssl (3.0.9-1)` when the source's version differs.
        let (name, version) = match field("Source:") {
            Some(src) => match src.split_once(" (") {
                Some((n, v)) => (n.to_owned(), v.trim_end_matches(')').to_owned()),
                None => (src, version),
            },
            None => (binary, version),
        };
        out.push(Package { name, version });
    }
    out.sort();
    out.dedup();
    out
}

/// The OSV ecosystem an `/etc/os-release` describes, where OSV has one.
pub fn ecosystem(os_release: &str) -> Option<String> {
    let field = |k: &str| {
        os_release
            .lines()
            .find_map(|l| l.strip_prefix(k))
            .map(|v| v.trim().trim_matches('"').to_owned())
    };
    let version = field("VERSION_ID=")?;
    match field("ID=")?.as_str() {
        // 3.21.3 is filed under v3.21.
        "alpine" => {
            let mut parts = version.split('.');
            Some(format!("Alpine:v{}.{}", parts.next()?, parts.next()?))
        }
        "debian" => Some(format!("Debian:{}", version.split('.').next()?)),
        "ubuntu" => {
            // OSV files LTS releases as `Ubuntu:22.04:LTS`: April of an
            // even year.
            let (year, month) = version.split_once('.')?;
            let lts = month == "04" && year.parse::<u32>().is_ok_and(|y| y % 2 == 0);
            Some(format!("Ubuntu:{version}{}", if lts { ":LTS" } else { "" }))
        }
        _ => None,
    }
}

/// `*.dist-info/METADATA`: RFC 822 headers, then a blank line.
pub fn parse_python_metadata(text: &str) -> Option<Package> {
    let head = text.split("\n\n").next()?;
    let field = |k: &str| {
        head.lines()
            .find_map(|l| l.strip_prefix(k))
            .map(|v| v.trim().to_owned())
    };
    Some(Package {
        // PyPI names compare case-insensitively with `-`, `_` and `.`
        // all equal; OSV files them in the normalised form.
        name: field("Name:")?.to_lowercase().replace(['_', '.'], "-"),
        version: field("Version:")?,
    })
}

/// A `package.json` that describes an installed package: it has both a
/// name and a version. An application's own often has neither.
pub fn parse_package_json(text: &str) -> Option<Package> {
    let doc: Value = serde_json::from_str(text).ok()?;
    Some(Package {
        name: doc["name"].as_str()?.to_owned(),
        version: doc["version"].as_str()?.to_owned(),
    })
}

/// How many directory entries the search for application dependencies
/// may visit across all layers. A large image has a few hundred thousand
/// files; past this the scan stops looking rather than run for minutes,
/// and what it found so far is still reported.
const WALK_BUDGET: usize = 400_000;

/// Application dependencies in an image's layers: `(ecosystem, package,
/// layer)`, the layer being the LOWEST one that has it, counted from 1.
///
/// Links are not followed, and whiteouts are not interpreted - a package
/// an upper layer deleted is still reported, attributed to the layer
/// that shipped it. That errs toward saying too much, which is the right
/// way for this to be wrong.
pub fn app_packages(layers: &[std::path::PathBuf]) -> Vec<(&'static str, Package, usize)> {
    fn read(p: &std::path::Path) -> String {
        // Metadata files are small; anything large is not one.
        match std::fs::metadata(p) {
            Ok(m) if m.is_file() && m.len() < 1024 * 1024 => {
                std::fs::read_to_string(p).unwrap_or_default()
            }
            _ => String::new(),
        }
    }
    fn walk(
        dir: &std::path::Path,
        layer: usize,
        budget: &mut usize,
        out: &mut Vec<(&'static str, Package, usize)>,
    ) {
        let Ok(rd) = std::fs::read_dir(dir) else {
            return;
        };
        let in_node_modules = dir.file_name().is_some_and(|n| n == "node_modules");
        for e in rd.flatten() {
            if *budget == 0 {
                return;
            }
            *budget -= 1;
            if !e.file_type().is_ok_and(|t| t.is_dir()) {
                continue;
            }
            let path = e.path();
            let name = e.file_name();
            let name = name.to_string_lossy();
            if name.ends_with(".dist-info") {
                if let Some(p) = parse_python_metadata(&read(&path.join("METADATA"))) {
                    out.push(("PyPI", p, layer));
                }
                continue;
            }
            if in_node_modules
                && !name.starts_with('@')
                && let Some(p) = parse_package_json(&read(&path.join("package.json")))
            {
                out.push(("npm", p, layer));
            }
            // `@scope/name` is one level further down, and a package's
            // own node_modules is where its dependencies are.
            if dir
                .file_name()
                .is_some_and(|n| n.to_string_lossy().starts_with('@'))
                && dir
                    .parent()
                    .and_then(|d| d.file_name())
                    .is_some_and(|n| n == "node_modules")
                && let Some(p) = parse_package_json(&read(&path.join("package.json")))
            {
                out.push(("npm", p, layer));
            }
            walk(&path, layer, budget, out);
        }
    }
    let mut out = Vec::new();
    let mut budget = WALK_BUDGET;
    for (i, layer) in layers.iter().enumerate() {
        walk(layer, i + 1, &mut budget, &mut out);
    }
    // Lowest layer wins, one row per (ecosystem, name, version).
    out.sort_by(|a, b| (a.0, &a.1, a.2).cmp(&(b.0, &b.1, b.2)));
    out.dedup_by(|b, a| a.0 == b.0 && a.1 == b.1);
    out
}

/// A CVSS 3.x base score from its vector, or `None` for any other
/// scheme. OSV carries the vector and not the number, and the number is
/// what anybody sorts by.
pub fn cvss3_score(vector: &str) -> Option<f64> {
    if !vector.starts_with("CVSS:3") {
        return None;
    }
    let get = |k: &str| {
        vector
            .split('/')
            .find_map(|p| p.strip_prefix(k)?.strip_prefix(':'))
    };
    let changed = get("S")? == "C";
    let av = match get("AV")? {
        "N" => 0.85,
        "A" => 0.62,
        "L" => 0.55,
        _ => 0.2,
    };
    let ac = if get("AC")? == "L" { 0.77 } else { 0.44 };
    let pr = match (get("PR")?, changed) {
        ("N", _) => 0.85,
        ("L", false) => 0.62,
        ("L", true) => 0.68,
        (_, false) => 0.27,
        (_, true) => 0.5,
    };
    let ui = if get("UI")? == "N" { 0.85 } else { 0.62 };
    let cia = |k: &str| match get(k) {
        Some("H") => Some(0.56),
        Some("L") => Some(0.22),
        Some(_) => Some(0.0),
        None => None,
    };
    let iss = 1.0 - (1.0 - cia("C")?) * (1.0 - cia("I")?) * (1.0 - cia("A")?);
    let impact = if changed {
        7.52 * (iss - 0.029) - 3.25 * (iss - 0.02f64).powi(15)
    } else {
        6.42 * iss
    };
    if impact <= 0.0 {
        return Some(0.0);
    }
    let exploit = 8.22 * av * ac * pr * ui;
    let raw = if changed {
        (1.08 * (impact + exploit)).min(10.0)
    } else {
        (impact + exploit).min(10.0)
    };
    // The spec's "round up to one decimal".
    Some((raw * 10.0 - 1e-9).ceil() / 10.0)
}

fn post(agent: &ureq::Agent, url: &str, body: &Value) -> Result<Value, String> {
    let resp = agent
        .post(url)
        .header("Content-Type", "application/json")
        .send(body.to_string().as_bytes())
        .map_err(|e| format!("{url}: {e}"))?;
    read(url, resp)
}

fn read(url: &str, resp: ureq::http::Response<ureq::Body>) -> Result<Value, String> {
    let status = resp.status().as_u16();
    let mut bytes = Vec::new();
    resp.into_body()
        .into_reader()
        .take(32 * 1024 * 1024)
        .read_to_end(&mut bytes)
        .map_err(|e| format!("{url}: {e}"))?;
    if !(200..300).contains(&status) {
        return Err(format!("{url}: HTTP {status}"));
    }
    serde_json::from_slice(&bytes).map_err(|e| format!("{url}: {e}"))
}

/// One vulnerability record, reduced to what is shown.
fn describe(agent: &ureq::Agent, base: &str, ecosystem: &str, id: &str, p: &Package) -> Value {
    let mut row = json!({ "id": id, "package": p.name, "version": p.version });
    let url = format!("{base}/v1/vulns/{id}");
    let detail = agent
        .get(&url)
        .call()
        .map_err(|e| format!("{url}: {e}"))
        .and_then(|r| read(&url, r))
        .unwrap_or(Value::Null);
    let vector = detail["severity"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|s| s["score"].as_str())
        .find(|s| s.starts_with("CVSS:3"));
    row["score"] = json!(vector.and_then(cvss3_score));
    row["summary"] = json!(
        detail["summary"]
            .as_str()
            .or(detail["details"].as_str())
            .map(|s| {
                s.lines()
                    .next()
                    .unwrap_or("")
                    .chars()
                    .take(160)
                    .collect::<String>()
            })
    );
    // The CVE it is filed under upstream, which is the name
    // anybody will search for.
    row["cve"] = json!(
        detail["upstream"]
            .as_array()
            .into_iter()
            .chain(detail["aliases"].as_array())
            .flatten()
            .filter_map(|a| a.as_str())
            .find(|a| a.starts_with("CVE-"))
    );
    // Fixed where, for THIS release of the distribution.
    row["fixed"] = json!(
        detail["affected"]
            .as_array()
            .into_iter()
            .flatten()
            .filter(|a| a["package"]["ecosystem"] == ecosystem
                && a["package"]["name"] == p.name.as_str())
            .flat_map(|a| a["ranges"].as_array().into_iter().flatten())
            .flat_map(|r| r["events"].as_array().into_iter().flatten())
            .find_map(|e| e["fixed"].as_str())
    );
    row
}

/// How many vulnerability records are fetched in full. Each is its own
/// request, so this bounds how long a scan of a very stale image takes;
/// past it the ids are still reported, without a score or a fix.
pub const MAX_DETAILS: usize = 300;

/// Ask OSV about `packages`. Returns a JSON document rather than a type:
/// its only consumers print it or hand it to a page.
pub fn scan(base: &str, ecosystem: &str, packages: &[Package]) -> Result<Value, String> {
    let agent = ureq::Agent::config_builder()
        .http_status_as_error(false)
        .timeout_global(Some(std::time::Duration::from_secs(60)))
        .build()
        .new_agent();

    // Eight requests side by side rather than one large one: the
    // service answers a batch at the speed of its slowest package, and
    // a single batch of sixty took ten seconds where eight take two.
    let per = packages.len().div_ceil(8).clamp(1, 500);
    let replies: Vec<Result<Vec<(String, &Package)>, String>> = std::thread::scope(|s| {
        let workers: Vec<_> = packages
            .chunks(per)
            .map(|chunk| {
                let agent = agent.clone();
                s.spawn(move || {
                    let queries: Vec<Value> = chunk
                        .iter()
                        .map(|p| json!({ "package": { "name": p.name, "ecosystem": ecosystem }, "version": p.version }))
                        .collect();
                    let reply = post(
                        &agent,
                        &format!("{base}/v1/querybatch"),
                        &json!({ "queries": queries }),
                    )?;
                    let mut found = Vec::new();
                    for (p, r) in chunk
                        .iter()
                        .zip(reply["results"].as_array().into_iter().flatten())
                    {
                        for v in r["vulns"].as_array().into_iter().flatten() {
                            if let Some(id) = v["id"].as_str() {
                                found.push((id.to_owned(), p));
                            }
                        }
                    }
                    Ok(found)
                })
            })
            .collect();
        workers
            .into_iter()
            .map(|w| w.join().unwrap_or_else(|_| Err("a lookup panicked".into())))
            .collect()
    });
    // (vulnerability id, the package that has it). Any failed request
    // fails the scan: a partial answer would read as "these are all".
    let mut hits: Vec<(String, &Package)> = Vec::new();
    for r in replies {
        hits.extend(r?);
    }

    // One request per record, so several at a time: sequentially a
    // stale image took a quarter of a minute, nearly all of it waiting.
    let detailed = &hits[..hits.len().min(MAX_DETAILS)];
    let per = detailed.len().div_ceil(8).max(1);
    let mut findings: Vec<Value> = std::thread::scope(|s| {
        let workers: Vec<_> = detailed
            .chunks(per)
            .map(|chunk| {
                let agent = agent.clone();
                s.spawn(move || {
                    chunk
                        .iter()
                        .map(|(id, p)| describe(&agent, base, ecosystem, id, p))
                        .collect::<Vec<_>>()
                })
            })
            .collect();
        workers
            .into_iter()
            .flat_map(|w| w.join().unwrap_or_default())
            .collect()
    });
    for (id, p) in hits.iter().skip(MAX_DETAILS) {
        findings.push(json!({ "id": id, "package": p.name, "version": p.version }));
    }
    // Worst first; unscored last, since "unknown" is not "harmless" but
    // cannot be ranked above a known critical either.
    findings.sort_by(|a, b| {
        b["score"]
            .as_f64()
            .unwrap_or(-1.0)
            .total_cmp(&a["score"].as_f64().unwrap_or(-1.0))
    });
    Ok(json!({
        "ecosystem": ecosystem,
        "packages": packages.len(),
        "findings": findings,
        "details_truncated": hits.len() > MAX_DETAILS,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn apk_packages_are_named_by_their_origin() {
        let db = "P:libssl3\nV:3.3.2-r1\no:openssl\n\nP:busybox\nV:1.37.0-r9\n\nP:libcrypto3\nV:3.3.2-r1\no:openssl\n";
        let got = parse_apk(db);
        // Two binaries from one source are one thing to ask about.
        assert_eq!(got.len(), 2);
        assert!(got.contains(&Package {
            name: "openssl".into(),
            version: "3.3.2-r1".into()
        }));
        assert!(got.contains(&Package {
            name: "busybox".into(),
            version: "1.37.0-r9".into()
        }));
    }

    #[test]
    fn dpkg_reads_only_what_is_installed_and_prefers_the_source() {
        let db = "Package: libssl3\nStatus: install ok installed\nSource: openssl\nVersion: 3.0.9-1\n\n\
                  Package: gone\nStatus: deinstall ok config-files\nVersion: 1\n\n\
                  Package: libc6\nStatus: install ok installed\nSource: glibc (2.36-9)\nVersion: 2.36-9+b1\n\n\
                  Package: bash\nStatus: install ok installed\nVersion: 5.2-1\n";
        let got = parse_dpkg(db);
        assert_eq!(
            got,
            vec![
                Package {
                    name: "bash".into(),
                    version: "5.2-1".into()
                },
                Package {
                    name: "glibc".into(),
                    version: "2.36-9".into()
                },
                Package {
                    name: "openssl".into(),
                    version: "3.0.9-1".into()
                },
            ]
        );
    }

    #[test]
    fn os_release_maps_to_the_name_osv_files_it_under() {
        assert_eq!(
            ecosystem("ID=alpine\nVERSION_ID=3.21.3\n").as_deref(),
            Some("Alpine:v3.21")
        );
        assert_eq!(
            ecosystem("ID=debian\nVERSION_ID=\"12\"\n").as_deref(),
            Some("Debian:12")
        );
        assert_eq!(
            ecosystem("ID=ubuntu\nVERSION_ID=\"22.04\"\n").as_deref(),
            Some("Ubuntu:22.04:LTS")
        );
        assert_eq!(
            ecosystem("ID=ubuntu\nVERSION_ID=\"24.10\"\n").as_deref(),
            Some("Ubuntu:24.10")
        );
        assert_eq!(ecosystem("ID=fedora\nVERSION_ID=41\n"), None);
        // Debian testing has no VERSION_ID, and so no release to ask about.
        assert_eq!(ecosystem("ID=debian\n"), None);
    }

    #[test]
    fn application_packages_are_found_and_placed_in_their_lowest_layer() {
        let base = std::env::temp_dir().join(format!("rustainer-appdeps-{}", std::process::id()));
        let (l1, l2) = (base.join("l1"), base.join("l2"));
        let py = "usr/lib/python3/site-packages/Flask_Login-0.6.dist-info";
        let node = "app/node_modules";
        for (dir, text) in [
            (
                l1.join(py),
                "Name: Flask_Login\nVersion: 0.6.0\n\nA description\nVersion: 9\n",
            ),
            // The same distribution again, higher up: one row, layer 1.
            (l2.join(py), "Name: Flask_Login\nVersion: 0.6.0\n"),
        ] {
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::write(dir.join("METADATA"), text).unwrap();
        }
        for (dir, text) in [
            (
                l2.join(node).join("left-pad"),
                r#"{"name":"left-pad","version":"1.3.0"}"#,
            ),
            (
                l2.join(node).join("@scope/tool"),
                r#"{"name":"@scope/tool","version":"2.0.0"}"#,
            ),
            (
                l2.join(node).join("left-pad/node_modules/inner"),
                r#"{"name":"inner","version":"0.1.0"}"#,
            ),
            // The application's own manifest is not an installed package.
            (l2.join("app"), r#"{"name":"my-app","version":"1.0.0"}"#),
        ] {
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::write(dir.join("package.json"), text).unwrap();
        }
        let got: Vec<(String, String, usize)> = app_packages(&[l1, l2])
            .into_iter()
            .map(|(eco, p, layer)| (format!("{eco}:{}", p.name), p.version, layer))
            .collect();
        assert_eq!(
            got,
            vec![
                ("PyPI:flask-login".to_owned(), "0.6.0".to_owned(), 1),
                ("npm:@scope/tool".to_owned(), "2.0.0".to_owned(), 2),
                ("npm:inner".to_owned(), "0.1.0".to_owned(), 2),
                ("npm:left-pad".to_owned(), "1.3.0".to_owned(), 2),
            ]
        );
        std::fs::remove_dir_all(&base).unwrap();
    }

    #[test]
    fn cvss_scores_match_the_published_ones() {
        // The two every scanner is checked against, and one with a
        // changed scope, where the formula branches.
        assert_eq!(
            cvss3_score("CVSS:3.1/AV:N/AC:L/PR:N/UI:N/S:U/C:H/I:H/A:H"),
            Some(9.8)
        );
        assert_eq!(
            cvss3_score("CVSS:3.1/AV:N/AC:L/PR:N/UI:N/S:U/C:N/I:N/A:H"),
            Some(7.5)
        );
        assert_eq!(
            cvss3_score("CVSS:3.1/AV:N/AC:L/PR:N/UI:R/S:C/C:L/I:L/A:N"),
            Some(6.1)
        );
        assert_eq!(
            cvss3_score("CVSS:3.0/AV:L/AC:H/PR:H/UI:R/S:U/C:N/I:N/A:N"),
            Some(0.0)
        );
        assert_eq!(cvss3_score("CVSS:4.0/AV:N/AC:L"), None);
    }
}
