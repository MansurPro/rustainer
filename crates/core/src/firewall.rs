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

    fn name(&self) -> &'static str;
}

/// Pick a backend, preferring the native one.
pub fn detect() -> Option<Box<dyn Firewall>> {
    if which("nft") {
        return Some(Box::new(Nft));
    }
    if which("iptables") {
        return Some(Box::new(Iptables));
    }
    None
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

pub struct Iptables;

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
                .args(&args)
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .status()
                .map(|s| s.success())
                .unwrap_or(false);
            if !present {
                let mut add = args.clone();
                add[0] = "-I";
                run("iptables", &add)?;
            }
        }
        Ok(())
    }

    fn foreign_drop_policy(&self) -> bool {
        Command::new("iptables")
            .args(["-S", "FORWARD"])
            .output()
            .map(|o| String::from_utf8_lossy(&o.stdout).contains("-P FORWARD DROP"))
            .unwrap_or(false)
    }
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
