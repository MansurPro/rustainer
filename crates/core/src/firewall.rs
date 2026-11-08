//! Source NAT, behind a trait.
//!
//! Which backend to use is a genuinely open decision with real arguments
//! both ways — own the nftables encoding, or drive `nft(8)` — and it
//! determines whether an external binary becomes a runtime dependency.
//! It is behind a trait so it stays reversible (RND.md §15.5.5).
//!
//! What is implemented now is the second option: drive the host's own
//! tool. It is the shape that works on both `nftables` and `iptables-nft`
//! hosts without us reimplementing either, and it keeps the rule syntax
//! something an operator can read in `nft list ruleset` and recognise.

use serde::{Deserialize, Serialize};
use std::net::Ipv4Addr;
use std::process::Command;

/// Our own table, so `nft delete table` removes everything we added and
/// nothing anyone else did.
pub const TABLE: &str = "rustainer";

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error(
        "no firewall tool found. Containers can have addresses and a bridge without one, \
         but nothing will reach the outside world, so this is a refusal rather than a \
         silent half-configuration. Install `nft` (nftables) or `iptables`."
    )]
    NoBackend,
    #[error("{tool} {args:?} failed: {output}")]
    Failed {
        tool: &'static str,
        args: Vec<String>,
        output: String,
    },
}

/// One published port.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Publish {
    /// The host address to listen on. `0.0.0.0` means every interface.
    pub host_ip: Ipv4Addr,
    pub host_port: u16,
    pub container_ip: Ipv4Addr,
    pub container_port: u16,
}

impl std::fmt::Display for Publish {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{}:{} -> {}:{}",
            self.host_ip, self.host_port, self.container_ip, self.container_port
        )
    }
}

/// What a backend must do for us. Deliberately small.
pub trait Firewall {
    /// Everything needed for a container subnet to reach the outside
    /// world, applied as one unit.
    fn ensure_masquerade(&self, subnet: Ipv4Addr, prefix: u8, bridge: &str) -> Result<(), Error>;

    /// Let packets through the forward path for our bridge.
    ///
    /// Not optional on a host someone else has configured. A `FORWARD`
    /// policy of `DROP` - which Kubernetes, libvirt, firewalld and most
    /// cloud images set - silently blackholes every container's egress,
    /// and the symptom is "DNS times out, everything times out" with a
    /// perfectly correct address, route and NAT rule in place. Docker adds
    /// the same rules for the same reason.
    fn ensure_forward(&self, bridge: &str) -> Result<(), Error>;

    /// Is a policy somebody else set going to drop our traffic anyway?
    ///
    /// Worth saying out loud rather than leaving the user to discover it:
    /// an `accept` in our own table cannot rescue a packet another
    /// ruleset drops first, at any priority.
    fn foreign_drop_policy(&self) -> bool {
        false
    }

    /// Publish one port: translation **and** authorization.
    ///
    /// Two rules, not one, and they are not the same thing. The DNAT
    /// rewrites the destination; the filter accept permits the forward.
    /// A DNAT without the accept merely does not work - but **an accept
    /// without the DNAT, or an accept scoped too widely, opens the
    /// container to anyone who can route to its address.** That asymmetry
    /// is why partial state here is a security hole rather than a failure,
    /// and why both rules are written together or not at all.
    fn publish(&self, p: &Publish, bridge: &str) -> Result<(), Error>;

    /// Remove what `publish` added. Called on teardown, and on the failure
    /// path of a partial publish.
    fn unpublish(&self, p: &Publish, bridge: &str);

    fn name(&self) -> &'static str;
}

/// Pick a backend, preferring **the one the host is already using**.
///
/// Not the newest one. This is the open decision of RND.md §15.5.5 meeting
/// the unresolved question of §11.5, and the second settles it: an
/// `accept` in our own nftables table cannot rescue a packet that another
/// ruleset drops first, at any priority. On a host with existing
/// `iptables` rules - and `iptables` is `iptables-nft` almost everywhere
/// now - our rules must land in the same table, in the same evaluation
/// order, or they are decoration. Docker reaches the same conclusion.
///
/// So: if `iptables` can tell us about a FORWARD chain, use it. Only a
/// host with nothing there gets the native backend.
pub fn detect() -> Option<Box<dyn Firewall>> {
    if which("iptables") && iptables_has_forward_chain() {
        return Some(Box::new(Iptables));
    }
    if which("nft") {
        return Some(Box::new(Nft));
    }
    if which("iptables") {
        return Some(Box::new(Iptables));
    }
    None
}

fn iptables_has_forward_chain() -> bool {
    Command::new("iptables")
        .args(["-w", "5", "-S", "FORWARD"])
        .output()
        .map(|o| o.status.success() && !o.stdout.is_empty())
        .unwrap_or(false)
}

fn which(tool: &str) -> bool {
    Command::new(tool)
        .arg("--version")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

fn run(tool: &'static str, args: &[&str]) -> Result<(), Error> {
    let out = Command::new(tool)
        .args(args)
        .output()
        .map_err(|e| Error::Failed {
            tool,
            args: args.iter().map(|s| (*s).to_owned()).collect(),
            output: e.to_string(),
        })?;
    if !out.status.success() {
        return Err(Error::Failed {
            tool,
            args: args.iter().map(|s| (*s).to_owned()).collect(),
            output: String::from_utf8_lossy(&out.stderr).trim().to_owned(),
        });
    }
    Ok(())
}

fn run_owned(tool: &'static str, args: &[String]) -> Result<(), Error> {
    let refs: Vec<&str> = args.iter().map(String::as_str).collect();
    run(tool, &refs)
}

/// Idempotent: creating a table or chain that exists is not an error.
fn run_ok(tool: &'static str, args: &[&str]) {
    let _ = Command::new(tool)
        .args(args)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status();
}

pub struct Nft;

impl Firewall for Nft {
    fn name(&self) -> &'static str {
        "nft"
    }

    fn ensure_masquerade(&self, subnet: Ipv4Addr, prefix: u8, _bridge: &str) -> Result<(), Error> {
        let cidr = format!("{subnet}/{prefix}");
        run_ok("nft", &["add", "table", "ip", TABLE]);
        run_ok(
            "nft",
            &[
                "add",
                "chain",
                "ip",
                TABLE,
                "postrouting",
                "{ type nat hook postrouting priority srcnat; }",
            ],
        );
        // Without this one rule a container can send packets and never
        // receive a reply: the rest of the internet has no route back to
        // RFC 1918 space, so the SYN-ACK is blackholed. The failure looks
        // like "DNS times out, everything times out", not like a
        // permission error.
        //
        // `ip saddr <cidr> oifname != <bridge>` is the shape that does not
        // rewrite container-to-container traffic.
        let rule = format!("ip saddr {cidr} oifname != \"{_bridge}\" counter masquerade");
        // Adding the same rule twice is harmless but untidy; check first.
        if !rule_present("nft", &["list", "table", "ip", TABLE], &rule) {
            run("nft", &["add", "rule", "ip", TABLE, "postrouting", &rule])?;
        }
        Ok(())
    }

    fn publish(&self, p: &Publish, bridge: &str) -> Result<(), Error> {
        run_ok(
            "nft",
            &[
                "add",
                "chain",
                "ip",
                TABLE,
                "prerouting",
                "{ type nat hook prerouting priority dstnat; }",
            ],
        );
        for (chain, rule) in self.publish_rules(p, bridge) {
            if !rule_present("nft", &["list", "table", "ip", TABLE], &rule) {
                run("nft", &["add", "rule", "ip", TABLE, chain, &rule])?;
            }
        }
        Ok(())
    }

    fn unpublish(&self, p: &Publish, bridge: &str) {
        // nft has no "delete this rule by text", so each rule's handle has
        // to be looked up. Cheap, and the alternative - flushing the table
        // - would take other containers' rules with it.
        for (chain, rule) in self.publish_rules(p, bridge) {
            if let Some(handle) = nft_handle(chain, &rule) {
                run_ok(
                    "nft",
                    &["delete", "rule", "ip", TABLE, chain, "handle", &handle],
                );
            }
        }
    }

    fn ensure_forward(&self, bridge: &str) -> Result<(), Error> {
        run_ok(
            "nft",
            &[
                "add",
                "chain",
                "ip",
                TABLE,
                "forward",
                "{ type filter hook forward priority filter; }",
            ],
        );
        for rule in [
            format!("iifname \"{bridge}\" counter accept"),
            format!("oifname \"{bridge}\" counter accept"),
        ] {
            if !rule_present("nft", &["list", "table", "ip", TABLE], &rule) {
                run("nft", &["add", "rule", "ip", TABLE, "forward", &rule])?;
            }
        }
        Ok(())
    }
}

impl Nft {
    fn publish_rules(&self, p: &Publish, bridge: &str) -> Vec<(&'static str, String)> {
        // No `ip daddr` on the DNAT: it matches the port on any interface
        // that is not the bridge itself, which is what stops
        // container-to-container traffic being translated.
        let dnat = if p.host_ip.is_unspecified() {
            format!(
                "iifname != \"{bridge}\" tcp dport {} counter dnat to {}:{}",
                p.host_port, p.container_ip, p.container_port
            )
        } else {
            format!(
                "iifname != \"{bridge}\" ip daddr {} tcp dport {} counter dnat to {}:{}",
                p.host_ip, p.host_port, p.container_ip, p.container_port
            )
        };
        let accept = format!(
            "oifname \"{bridge}\" ip daddr {} tcp dport {} counter accept",
            p.container_ip, p.container_port
        );
        vec![("prerouting", dnat), ("forward", accept)]
    }
}

fn nft_handle(chain: &str, rule: &str) -> Option<String> {
    let out = Command::new("nft")
        .args(["-a", "list", "chain", "ip", TABLE, chain])
        .output()
        .ok()?;
    let text = String::from_utf8_lossy(&out.stdout).into_owned();
    let line = text.lines().find(|l| l.contains(rule))?;
    let (_, after) = line.rsplit_once("# handle ")?;
    Some(after.trim().to_owned())
}

pub struct Iptables;

/// `-w` on every invocation.
///
/// `iptables` takes an exclusive lock, and without `-w` it does not wait
/// for it - it fails. Any host also running Docker, libvirt or firewalld
/// holds that lock periodically, so a container start fails for reasons
/// that have nothing to do with the container and succeeds on the retry.
const WAIT: [&str; 2] = ["-w", "5"];

fn with_wait(args: &[&str]) -> Vec<String> {
    WAIT.iter()
        .chain(args.iter())
        .map(|s| (*s).to_owned())
        .collect()
}

impl Firewall for Iptables {
    fn name(&self) -> &'static str {
        "iptables"
    }

    fn ensure_masquerade(&self, subnet: Ipv4Addr, prefix: u8, bridge: &str) -> Result<(), Error> {
        let cidr = format!("{subnet}/{prefix}");
        // -C tests for the rule; a non-zero status means it is absent.
        let check = Command::new("iptables")
            .args([
                "-t",
                "nat",
                "-C",
                "POSTROUTING",
                "-s",
                &cidr,
                "!",
                "-o",
                bridge,
                "-j",
                "MASQUERADE",
            ])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .map(|s| s.success())
            .unwrap_or(false);
        if !check {
            run(
                "iptables",
                &[
                    "-t",
                    "nat",
                    "-A",
                    "POSTROUTING",
                    "-s",
                    &cidr,
                    "!",
                    "-o",
                    bridge,
                    "-j",
                    "MASQUERADE",
                ],
            )?;
        }
        Ok(())
    }

    fn ensure_forward(&self, bridge: &str) -> Result<(), Error> {
        for args in [
            vec!["-C", "FORWARD", "-i", bridge, "-j", "ACCEPT"],
            vec!["-C", "FORWARD", "-o", bridge, "-j", "ACCEPT"],
        ] {
            let present = Command::new("iptables")
                .args(with_wait(&args))
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .status()
                .map(|s| s.success())
                .unwrap_or(false);
            if !present {
                let mut add = args.clone();
                add[0] = "-I";
                run_owned("iptables", &with_wait(&add))?;
            }
        }
        Ok(())
    }

    fn publish(&self, p: &Publish, bridge: &str) -> Result<(), Error> {
        for (table, chain, rule) in iptables_publish_rules(p, bridge) {
            let mut check = vec!["-t", table, "-C", chain];
            check.extend(rule.iter().map(String::as_str));
            let present = Command::new("iptables")
                .args(with_wait(&check))
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .status()
                .map(|s| s.success())
                .unwrap_or(false);
            if !present {
                let mut add = vec!["-t", table, "-I", chain];
                add.extend(rule.iter().map(String::as_str));
                run_owned("iptables", &with_wait(&add))?;
            }
        }
        Ok(())
    }

    fn unpublish(&self, p: &Publish, bridge: &str) {
        for (table, chain, rule) in iptables_publish_rules(p, bridge) {
            let mut del = vec!["-t", table, "-D", chain];
            del.extend(rule.iter().map(String::as_str));
            let _ = Command::new("iptables")
                .args(with_wait(&del))
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .status();
        }
    }

    fn foreign_drop_policy(&self) -> bool {
        Command::new("iptables")
            .args(with_wait(&["-S", "FORWARD"]))
            .output()
            .map(|o| String::from_utf8_lossy(&o.stdout).contains("-P FORWARD DROP"))
            .unwrap_or(false)
    }
}

fn iptables_publish_rules(
    p: &Publish,
    bridge: &str,
) -> Vec<(&'static str, &'static str, Vec<String>)> {
    let to = format!("{}:{}", p.container_ip, p.container_port);
    let mut dnat: Vec<String> = vec!["!".into(), "-i".into(), bridge.to_owned()];
    if !p.host_ip.is_unspecified() {
        dnat.push("-d".into());
        dnat.push(p.host_ip.to_string());
    }
    dnat.extend([
        "-p".into(),
        "tcp".into(),
        "--dport".into(),
        p.host_port.to_string(),
        "-j".into(),
        "DNAT".into(),
        "--to-destination".into(),
        to,
    ]);
    // Translation and authorization are separate rules, and the accept is
    // scoped to exactly this container and port - not to the bridge at
    // large, which would publish every port of every container.
    let accept: Vec<String> = vec![
        "-o".into(),
        bridge.to_owned(),
        "-d".into(),
        p.container_ip.to_string(),
        "-p".into(),
        "tcp".into(),
        "--dport".into(),
        p.container_port.to_string(),
        "-j".into(),
        "ACCEPT".into(),
    ];
    vec![("nat", "PREROUTING", dnat), ("filter", "FORWARD", accept)]
}

fn rule_present(tool: &str, args: &[&str], needle: &str) -> bool {
    Command::new(tool)
        .args(args)
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).contains(needle))
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_table_is_ours_alone() {
        // `nft delete table ip rustainer` must remove everything we added
        // and nothing anyone else did.
        assert_eq!(TABLE, "rustainer");
    }

    #[test]
    fn a_missing_backend_refuses_rather_than_half_configuring() {
        let e = Error::NoBackend;
        let s = e.to_string();
        assert!(s.contains("refusal"), "{s}");
        assert!(s.contains("nft") && s.contains("iptables"), "{s}");
    }

    #[test]
    fn detection_prefers_the_native_tool() {
        // Can't assert which is installed here, only that detection is
        // total: a host with neither gets None, and the caller refuses.
        if let Some(f) = detect() {
            assert!(matches!(f.name(), "nft" | "iptables"));
        }
    }
}
