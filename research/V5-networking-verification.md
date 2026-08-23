# V5 — Adversarial verification: NETWORKING cluster

**Source under review:** `research/D4-networking.md` (2133 lines, ~422 claims), everything **except §12 (Rust crates)** — with the one crate-adjacent architectural question (§12.1's async adjudication) explicitly in scope.

**Verifier stance:** falsification, not summary. I did not write the source. Verdicts are one of:
**CONFIRMED** · **CORRECTED** · **REFUTED** · **OUTDATED** · **UNSUPPORTED** · **MISCLASSIFIED**.

**Evidence channels used**
- Live read-only inspection of `host-a` = `HOST-A`, Ubuntu 24.10, kernel `6.11.0-29-generic`, Docker `28.3.1`, `iptables v1.8.10 (nf_tables)`, `nftables 1.1.0`. **Uptime at verification time: 189 days** (source says 186 — consistent, source was written 3 days earlier). No mutating command was run.
- Linux kernel source at tag `v6.11` (matching the host's kernel), fetched from `raw.githubusercontent.com/torvalds/linux/v6.11/…`.
- `Documentation/networking/ip-sysctl.rst` at `master`.
- Project sources (moby, netavark, rust-netlink) and crates.io.

**Standing caveat applied throughout:** the host's Docker version, its `iptables-nft` backend, its `ufw`+`fail2ban` combination and its systemd-resolved stub are *properties of this box*, not facts about Linux. Where the source generalizes from them I say so.

**`old-host` is unreachable** (`old-host.example.edu` → NXDOMAIN). Every claim resting solely on `old-host` observation is marked **UNSUPPORTED — pending host**, with the exact command that would settle it. Those claims are not *disbelieved*; they are unverifiable from here.

---

## PART A — The two load-bearing findings

### A1. `ip_forward` "resets every other IPv4 sysctl" — **REFUTED** (§1.1 line 70, §9.8 Rule 2 line 1625, §11.2 line 1805)

This is the document's single strongest and most specific kernel claim, repeated three times and escalated in §11.2 to *"the single most dangerous operation in the ladder"*. **It is wrong**, and it is wrong in a way worth understanding: the document quotes the kernel documentation **accurately**, and the kernel documentation is **stale**.

**The claim as stated (§1.1):**
> *"Booby trap: writing `net.ipv4.ip_forward` resets other config. … So if we set `ip_forward=1` after configuring per-interface knobs (`rp_filter`, `accept_local`, `route_localnet`, `arp_ignore`…), we silently blow them away."*

**The quote is real.** `Documentation/networking/ip-sysctl.rst` at `master` still says, verbatim:
> *"This variable is special, its change resets all configuration parameters to their default state (RFC1122 for hosts, RFC1812 for routers)"*

**The kernel does not do that.** `net/ipv4/devinet.c` at **v6.11** (the host's exact kernel). The write handler is `devinet_sysctl_forward()`, and the *entire* side-effect path is `inet_forward_change()`:

```c
static void inet_forward_change(struct net *net)
{
	struct net_device *dev;
	int on = IPV4_DEVCONF_ALL(net, FORWARDING);

	IPV4_DEVCONF_ALL(net, ACCEPT_REDIRECTS) = !on;
	IPV4_DEVCONF_DFLT(net, FORWARDING) = on;
	inet_netconf_notify_devconf(net, RTM_NEWNETCONF, NETCONFA_FORWARDING,
				    NETCONFA_IFINDEX_ALL, net->ipv4.devconf_all);
	inet_netconf_notify_devconf(net, RTM_NEWNETCONF, NETCONFA_FORWARDING,
				    NETCONFA_IFINDEX_DEFAULT, net->ipv4.devconf_dflt);

	for_each_netdev(net, dev) {
		struct in_device *in_dev;
		if (on)
			dev_disable_lro(dev);
		in_dev = __in_dev_get_rtnl(dev);
		if (in_dev) {
			IN_DEV_CONF_SET(in_dev, FORWARDING, on);
			inet_netconf_notify_devconf(net, RTM_NEWNETCONF,
						    NETCONFA_FORWARDING,
						    dev->ifindex, &in_dev->cnf);
		}
	}
}
```

**The complete, exhaustive list of what a write to `net.ipv4.ip_forward` changes:**

| What changes | To what | Note |
|---|---|---|
| `conf/all/accept_redirects` | `!on` (i.e. `0` when you enable forwarding) | the *only* survivor of the doc's "RFC1122 vs RFC1812" wording |
| `conf/default/forwarding` | `on` | affects interfaces created *later* |
| `conf/<every existing iface>/forwarding` | `on` | this is the `conf/all` → `conf/<iface>` fan-out |
| LRO on every netdev | **disabled**, if `on` | `dev_disable_lro()`. Not restored when you set `ip_forward` back to 0. |
| route cache | flushed (`rt_cache_flush`) | in the caller |

**What it does NOT touch, contrary to the document's explicit list:** `rp_filter`, `accept_local`, `route_localnet`, `arp_ignore`, `arp_announce`, `arp_filter`, `send_redirects`, `secure_redirects`, `log_martians`, `proxy_arp`, `shared_media`, `bootp_relay`, `medium_id`, `tag`. **None of them.** The document names four knobs as at-risk; all four are safe.

**Two further corrections that fall out of the same code:**

1. **A no-op write is genuinely a no-op.** The gate is `if (write && *valp != val)`. Writing `1` to a host already at `1` never enters `inet_forward_change()` at all. The document's §1.1 warning — *"a blind write on a host that already had it enabled is a gratuitous reset of the admin's tuning"* — is **REFUTED at the kernel level**: it resets nothing, because the kernel short-circuits on an unchanged value. The read-then-write recommendation is still fine practice (and avoids the write syscall), but the *stated reason* for it is not real.

2. **The real, undocumented side-effect the document missed is `dev_disable_lro()`.** Enabling forwarding permanently disables Large Receive Offload on **every network device in the namespace** — including the host's physical NICs — and does not re-enable it when forwarding is turned back off. That *is* a persistent, silent, host-wide mutation with a measurable performance consequence, and it is the one genuine "we changed the admin's machine" effect in this operation. It is also unavoidable and correct (LRO coalesces segments that a router then cannot re-segment), but it belongs in the doc where the false claim currently is.

**Live confirmation on the host, which is exactly the predicted narrow pattern and not the broad one:**
```
net.ipv4.ip_forward                 = 1
net.ipv4.conf.all.forwarding        = 1      <- the write itself
net.ipv4.conf.default.forwarding    = 1      <- set by inet_forward_change
net.ipv4.conf.eth0.forwarding       = 1      <- set by inet_forward_change (fan-out)
net.ipv4.conf.all.accept_redirects  = 0      <- set to !on by inet_forward_change
net.ipv4.conf.eth0.accept_redirects = 0

net.ipv4.conf.all.rp_filter         = 2      <- Ubuntu's sysctl.d value, NOT reset to kernel default 0
net.ipv4.conf.eth0.rp_filter        = 2      <- survived
net.ipv4.conf.*.route_localnet      = 0      <- untouched
net.ipv4.conf.*.accept_local        = 0      <- untouched
net.ipv4.conf.*.arp_ignore          = 0      <- untouched
```
`rp_filter = 2` is Ubuntu's distro setting, not the kernel default (`0`). It is intact on a host where `ip_forward` was flipped to 1 by Docker. **If the document's claim were true, `rp_filter` would read `0` here.** It reads `2`.

**Is it "a garbled version of the `conf/all` vs `conf/<iface>` interaction"?** — Partly, and the prompt's hypothesis is close but not exact. It is not a garbling by the author; it is a faithful quote of documentation that has been stale since roughly the 2.2/2.4 era, when the RFC1122-host / RFC1812-router distinction did drive a wider set of parameters. What remains of that distinction in a modern kernel is `accept_redirects` and nothing else. The `conf/all` → `conf/<iface>` fan-out *is* real and *is* in this function, but it fans out exactly one parameter (`forwarding`), not "all configuration parameters".

**Verdict: REFUTED.** The recommendation it produces ("set `ip_forward` first, read-then-write") is harmless and mildly good hygiene, so no downstream design decision breaks. But **§11.2's ranking is wrong**: `ip_forward` is *not* the most dangerous single operation in the ladder. On the evidence in this very document, that title belongs to **§4.6's finding that a `-p` publish silently punches through `ufw`** — a real, verified, exploitable-by-accident exposure — or to §9.2's stale-DNAT-onto-a-recycled-IP misroute. Both are demonstrated; the sysctl reset is not.

**Fix for the source, precisely:** replace the §1.1 quote-and-warning with: *"writing `conf/all/forwarding` sets `conf/all/accept_redirects` to its inverse, propagates `forwarding` to `conf/default` and to every existing interface, and permanently disables LRO on every netdev. The `ip-sysctl.rst` claim that it 'resets all configuration parameters' is stale documentation; `inet_forward_change()` in `net/ipv4/devinet.c` is the whole of it."* Then delete §9.8 Rule 2's justification and demote §11.2's "most dangerous operation" ranking.

---
### A2. The live production leak (§9.3) — **CONFIRMED**, and the document's *diagnosis* is **CORRECTED** in a way that strengthens its own §9 conclusion

I re-verified every clause independently, and then went hunting for the innocent explanations the prompt asked about. **There is no innocent explanation. The leak is real.**

**Re-verified, clause by clause (all read-only, 2026-09-05, uptime 189 days):**

| Document's claim | Independent observation | Verdict |
|---|---|---|
| bridge `br-ac0ab0eb6175` UP, `172.19.0.1/16` | `26: br-ac0ab0eb6175: <NO-CARRIER,BROADCAST,MULTICAST,UP> … inet 172.19.0.1/16` | CONFIRMED |
| zero ports | `bridge link show` lists 5 ports, **all** on `br-5e08a4608e57`. Zero on the orphan. `ip -d link` reports `fdb_n_learned 0`. | CONFIRMED |
| surviving MASQUERADE rule | `nat/POSTROUTING` rule 3: `MASQUERADE … !br-ac0ab0eb6175 172.19.0.0/16`, **0 pkts / 0 bytes** | CONFIRMED (and inert) |
| two `raw/PREROUTING` anti-spoof DROPs | rules 1 and 2: `DROP !br-ac0ab0eb6175 → 172.19.0.2` and `→ 172.19.0.3`, **0 pkts / 0 bytes each** | CONFIRMED (and inert) |
| `docker network inspect` → not found | `[]` + `Error response from daemon: network ac0ab0eb6175 not found` | CONFIRMED |
| boltdb has no record | Parsed `local-kv.db` directly: **0** occurrences of `172.19`, **0** of `ac0ab0eb6175`. (28 occurrences of `172.18`, for contrast.) | CONFIRMED |
| 6 sandbox records vs 5 netns mounts | Exactly 6 `docker/network/v1.0/sandbox/*` keys; 5 files in `/run/docker/netns/`; 5 containers. The extra is `sandbox/c35b41bcf51c9dc2…` — the **same id the document names**. | CONFIRMED |
| 4 network records | Exactly 4 `network/*` keys, matching the 4 in `docker network ls`. No fifth. | CONFIRMED |

**Innocent explanations, each ruled out:**

1. **"A network Docker still tracks under another name."** Ruled out three ways: only 4 `network/*` keys exist in the store; `docker network ls` shows 4; and the store contains zero bytes matching either the network id prefix or its subnet. There is nothing for the name to be.
2. **"A swarm or overlay artifact."** `docker info` → `Swarm=inactive`. Overlay networks would also come from the *global*-scope pool (`10.0.0.0/8` in /24s, §10.1), not `172.19.0.0/16` from the local pool. Wrong pool, wrong feature, feature off.
3. **"A compose artifact Docker still owns."** All 5 containers belong to one compose project (`aikserver-*`), all 5 endpoints are on network `5e08a4608e57…`, and `docker ps -a` shows no stopped container at all. There is no container, running or stopped, on `172.19.x`.
4. **"An in-flight operation."** `dockerd` (pid 70234) has been running since **Mon Mar 2 15:45:42 2026** — 187 days. The orphan bridge is `ifindex 26`, created *before* the live bridge at `ifindex 67`. Nothing is in flight.
5. **"A second engine — podman, a second dockerd, a hand-made bridge."** `ps` shows exactly one `dockerd`, one `containerd`, five `containerd-shim-runc-v2`, no podman. And the two `raw/PREROUTING` anti-spoof DROPs are a **Docker-28-specific** rule shape (§4.5) that no admin writes by hand; a human-made bridge would not come with them.
6. **"The rules are harmful and something is actively broken."** No — all three leaked rules show `0 packets / 0 bytes`. The document says this too and is right: the leak is *inert*, and that is precisely what makes it survive unnoticed.

**CORRECTION to the document's diagnosis.** §9.3 concludes:
> *"Docker's reconciliation runs at daemon start (§9.4) and this daemon has not restarted since March. **Any design whose only reconciliation point is 'when the long-lived process starts' will drift exactly this way.**"*

The premise is wrong and the conclusion is therefore under-stated. **A daemon restart would not have cleaned this up.** `removeIPChains()` (`daemon/libnetwork/drivers/bridge/internal/iptabler/network.go`, `master`) removes exactly eleven *Docker-owned* chains:

```go
{Name: dockerChain,          Table: iptables.Nat},
{Name: dockerForwardChain,   Table: iptables.Filter},
{Name: dockerBridgeChain,    Table: iptables.Filter},
{Name: dockerChain,          Table: iptables.Filter},
{Name: dockerCTChain,        Table: iptables.Filter},
{Name: dockerInternalChain,  Table: iptables.Filter},
{Name: isolationChain1, isolationChain2, oldIsolationChain, oldIngressChain ×2},
```
plus one legacy `filter/FORWARD` jump. **It never touches built-in `nat/POSTROUTING` or built-in `raw/PREROUTING`** — and the leaked MASQUERADE and both anti-spoof DROPs live in exactly those two built-in chains. Nor does anything at startup enumerate `br-*` devices and delete ones with no matching network record.

The document already holds the primary source that explains why, at §9.4 rule 4, quoted from moby: *"Without seeing config (interface names, addresses, and so on), the iptabler's cleaner can't clean up network or port-specific rules that may have been added to iptables built-in chains."* **It just never connected that quote to §9.3.** Connected, it says: rules Docker writes into *shared, built-in* chains are structurally outside its own reconciliation, because reconstructing the rule text requires config that was deleted along with the network record.

**Why this strengthens §9's conclusion rather than weakening it.** The lesson is not "reconcile more often". Reconciling at every start would not have found this. The lesson is the one §9.7 already reaches by a different route and which this evidence now makes load-bearing:
- **reconciliation must be keyed to a live-set you own**, not to rule text you must reconstruct (CNI's `GC` shape: *"here is everything that should exist, delete the rest"*), and
- **attribution must be structural** — §5.4's *"one table of our own"* and §1.2's *"our own netns directory"*. `delete table inet mydocker` (§9.4 rule 3) reclaims all of this in one statement precisely because nothing of ours lives in a chain somebody else owns.

So: **§9.3's observation CONFIRMED, its causal story CORRECTED, its design conclusion upheld and upgraded.** The failure this host exhibits is not "the daemon didn't restart"; it is *"the engine wrote state into chains it does not own, and then lost the key needed to find it again."* That is a sharper argument for §5.4 and §9.7 than the one the document makes.

**Residual honesty:** the document's §9.3 `[O] MED` — *"cause not determined"* — stands. I also could not determine which operation produced the orphan (that requires mutation). The dev-VM experiment the document proposes is the right one; I'd add one step to it: after each kill point, also **restart dockerd** and re-diff, to demonstrate the built-in-chain residue survives reconciliation.

---
## PART B — The async adjudication (§12.1), and the licensing landmine

### B1. Is async forced into the runtime core? — **CONFIRMED, all three parts.** The answer is **no**.

The document's conclusion is that there is *no* collision with D2 §8's sync-only, never-had-a-second-thread requirement, because `rtnetlink` is avoided entirely and the codec crates carry zero async dependencies. I checked all three legs independently against crates.io and the repositories.

**Leg 1 — `rtnetlink` has no synchronous API. CONFIRMED.**
`rtnetlink/src/connection.rs` at `main` exposes exactly five public constructors and **not one of them is blocking**:

| Constructor | Feature gate | Returns |
|---|---|---|
| `new_connection()` | `tokio_socket` | `(Connection<RouteNetlinkMessage>, Handle, UnboundedReceiver<…>)` |
| `new_multicast_connection(groups)` | `tokio_socket` | same shape |
| `new_connection_with_socket<S>()` | none | `(Connection<RouteNetlinkMessage, S>, Handle, UnboundedReceiver<…>)` |
| `new_multicast_connection_with_socket<S>(groups)` | none | same shape |
| `from_socket<S>(socket)` | none | same shape |

Every one returns a `netlink_proto::Connection`, which is a `Future` the caller must drive, plus a `futures_channel::mpsc::UnboundedReceiver`. There is no `S: Socket` (blocking) escape hatch.

And the dependency graph closes the last door: **`rtnetlink` 0.23.0 requires `netlink-proto`, `futures-channel` and `futures-util` as non-optional normal dependencies.** Only `tokio` and `async-global-executor` are optional. So `default-features = false` removes the *tokio socket*, not the async machinery. The document's claim that "`netlink-proto` … is the reason `rtnetlink` cannot be sync" is correct and now has a dependency-graph proof, not just an API reading.

**Leg 2 — the codec crates carry zero async dependencies. CONFIRMED.**

| Crate | Version | Normal dependencies | Async? |
|---|---|---|---|
| `netlink-packet-route` | 0.33.0 | `bitflags ^2`, `libc ^0.66`, `log ^0.4.20`, `netlink-packet-core ^0.9.0`, `zerocopy ^0.8` — all required, none optional | **none** |
| `netlink-packet-core` | 0.9.0 | (no normal deps surfaced; `netlink-packet-route` appears only as a **dev**-dependency) | **none** |
| `netlink-sys` | 0.9.0 | required: `bytes ^1.8`, `libc ^0.2.164`, `log ^0.4.8`. **Optional: `tokio`, `futures-util`, `async-io`, `mio`** | **opt-in only** |

`netlink-sys`'s default feature set is empty, so `netlink-sys = { version = "0.9", default-features = false }` compiles a plain blocking `RawFd` wrapper and links no executor. The document's Route 1 is sound: **you can build the entire rtnetlink data path with a blocking socket and no futures anywhere in the tree.**

**Leg 3 — what netavark actually depends on. CONFIRMED, with one important qualification the document should carry.**

`containers/netavark` `Cargo.toml` at `main`:
- **No `rtnetlink`.** Confirmed — it is absent from the dependency list entirely.
- `netlink-packet-route = "0.33.0"`, `netlink-packet-core = "0.9.0"`, `netlink-sys = "0.9.0"` — exactly as the document states, exact versions match.
- `nftables = "0.6.3"` — confirms §12.3's account of the nftables path.
- `tokio = { version = "1.53.1", features = ["rt", "rt-multi-thread", "signal", "fs", "time"] }`, alongside `tonic`, `tonic-prost`, `prost`, `tokio-stream`, `mozim` (a DHCP client), `futures-*`, `hyper-util`, `zbus`.

**The qualification.** The document says netavark's tokio is *"for the DHCP-proxy gRPC service … a separate binary path. The netlink core is sync."* That is true, and the dependency list corroborates it (`tonic`/`prost`/`mozim` are the DHCP-proxy stack). But note `rt-multi-thread` is enabled — netavark's crate graph **does** contain a multi-threaded tokio runtime, it merely does not construct one on the netlink path.

More importantly: **netavark is weaker evidence for D2 §8's requirement than the document implies, because netavark never calls `clone()`.** It is a CNI-style plugin invoked as a short-lived process by Podman, given a netns path. The thing D2 §8 is protecting — *"the process that calls `clone()` must never have had a second thread"* — is not a property netavark has to maintain. So the citation supports the narrow claim (*"the sync codec + sync socket combination works in production"*) and does **not** support the broad one (*"a container engine can keep its clone-calling process thread-free while using these crates"*). Nobody has demonstrated the broad claim, because netavark isn't the right witness for it. The document's own §12.1 recommendation is nevertheless correct — it just rests on Leg 2, not on netavark.

**Adjudication: async is NOT forced into the runtime core.** Route 1 is available, it is dependency-verified, and the one production Rust witness took it. **CONFIRMED.**

One correction of emphasis: the document frames this as *"there is no conflict because `rtnetlink` is avoided."* The sharper statement is *"there is no conflict because the async lives entirely in `netlink-proto`, which is a `rtnetlink`-only dependency; the codec and socket layers below it were designed to be usable without it, and are."* That is a structural fact about the `rust-netlink` family's layering, and it is the reason Route 1 exists at all rather than being a lucky accident.

---

### B2. `rustables` is GPL-3.0-or-later — **CONFIRMED**

crates.io API, `rustables` latest:
- **`"license": "GPL-3.0-or-later"`** — confirmed verbatim.
- version **0.8.8**, published **2026-06-23**, repository **`https://gitlab.com/rustwall/rustables`**, **47 750** lifetime downloads.

Every figure in §12.3's `rustables` paragraph matches to the digit (0.8.8, 2026-06-23, gitlab.com/rustwall/rustables, "47.8k lifetime"). The licensing hazard is real: `rustables` is a fork of `nftnl-rs` (MIT OR Apache-2.0) that relicensed to GPL-3.0-or-later, so linking it does make the resulting binary GPL-3.0. For a container runtime — a category where downstream embedding is normal and MIT/Apache-2.0 is universal — the document's "do not adopt, and the reason is the licence" is correct and the reasoning is the right shape (a licence change is a project-level human decision, not a dependency choice).

**No correction.** This is the cleanest fully-confirmed claim in the crate-adjacent material.

---
## PART C — §4: the `docker run -p 8080:80 nginx` trace

This is the trace the prompt flags as most contaminated by folklore. I checked every rule, every hop and every counter against the live ruleset.

### C1. §4.1 the live ruleset — **CONFIRMED, byte for byte**

`iptables -t nat -S` reproduced **exactly** as printed in §4.1, including rule order, the two per-bridge `RETURN`s, both DNAT rules, and all three MASQUERADE rules. `iptables -t raw -S` reproduced exactly: seven per-container-IP DROPs. `filter/FORWARD` reproduced exactly, including the `DOCKER-USER` → `DOCKER-FORWARD` → `ufw-*` ordering, `-P FORWARD DROP`, and the `DOCKER-CT` / `DOCKER-BRIDGE` / `DOCKER-ISOLATION-STAGE-1/2` structure. `DOCKER-USER` is present and empty (`-N DOCKER-USER` and nothing else).

One trivial inaccuracy, flagged for completeness only: §4.6 quotes `ufw-user-input` as three rules. It actually has five — the `:22` rule carries `-m comment --comment "'dapp_OpenSSH'"`, and there are additionally `-p udp --dport 443 -j ACCEPT` (QUIC) and a duplicate `-p tcp --dport 443`. This changes nothing about the argument.

### C2. §4.2 / §4.3 the hop-by-hop path — **CONFIRMED**

Hook order (`raw` -300 → conntrack -200 → `nat/PREROUTING` dstnat -100 → routing → `filter/FORWARD` 0 → `nat/POSTROUTING` srcnat 100), the `--dst-type LOCAL` → `fib daddr type local` translation, the "DNAT applies only to the first packet of a flow" claim, the "reverse translation happens in conntrack, not by any rule" claim, and "inbound published-port traffic is not masqueraded, the container sees the real client IP" — all correct, and the last is directly derivable from the `-s 172.18.0.0/16` match, as the document says.

The counter argument is now stronger than when the document was written. Live `nft list chain ip nat PREROUTING` / `DOCKER`:
```
PREROUTING: fib daddr type local  counter packets 14556180 bytes 784449234 jump DOCKER
DOCKER:     tcp dport 80  ... dnat to 172.18.0.4:80   counter packets 24331 bytes 1339553
            tcp dport 443 ... dnat to 172.18.0.4:443  counter packets 59249 bytes 3413677
```
Both DNAT counters have roughly doubled since the document was written while `raw/PREROUTING` has passed 279M packets / 301 GB. The ratio (≈83k DNAT hits against 279M packets through the raw hook) is the "nat counts connections, not packets" claim made quantitative. **CONFIRMED and reinforced.**

The `-A DOCKER ! -i br-… -p tcp --dport 80` rule genuinely has **no `-d` match**, so the publish really does cover `10.0.0.5` (VPC) and `10.1.0.2` (eth1) as well as the public address. Both private addresses verified present. **CONFIRMED** — and this is the most underappreciated finding in §4.

### C3. §4.4 `docker-proxy` — **CONFIRMED on mechanism, MISCLASSIFIED on its headline live proof**

**What is confirmed.** Four `docker-proxy` processes, exactly the argv the document prints, all with `-use-listen-fd`. `ss -ltnp` shows them holding `0.0.0.0:80`, `0.0.0.0:443`, `[::]:80`, `[::]:443`. `ip6tables -t nat -S` has a `DOCKER` chain that is **empty** — zero IPv6 DNAT rules. `net.ipv6.conf.all.forwarding = 0`. The four reasons the proxy exists (address-family bridging, holding the port, loopback publishing without `route_localnet`, hairpin) are each individually correct, and Docker's documentation supports the first verbatim: *"Ports on the host's IPv6 addresses will map to the container's IPv4 address if no host IP is given in a port mapping, the bridge network is IPv4-only, and `--userland-proxy=true` (default)."*

**The proxy-is-a-bystander claim is confirmed and now has better data.** CPU time after 24 days 13 hours of uptime:
```
2588176  0.0.0.0:80   TIME 00:00:03
2588184  [::]:80      TIME 00:00:03
2588192  0.0.0.0:443  TIME 00:00:04
2588199  [::]:443     TIME 00:00:03
```
Three to four seconds of CPU across 24 days, against 83 580 DNAT'd connections. **CONFIRMED**: for ordinary IPv4 inbound, DNAT does the work.

**What is MISCLASSIFIED.** §4.4 says, tagged `[O] HIGH` and headlined *"The case where DNAT genuinely cannot work — and this host proves it"*:
> *"Therefore: **every IPv6 connection to this production web server is being carried by `docker-proxy`, in userspace, and nothing else.**"*

**This host cannot demonstrate that, because this host has no IPv6 connectivity at all.**
```
$ ip -6 addr show scope global      # → EMPTY, exit 0
$ ip -6 route show
fe80::/64 dev eth0  … (link-local only, on every interface)
                                    # no global address, no default route, no GUA
```
There is no global IPv6 address on `eth0`, `eth1`, or anywhere; the only IPv6 routes are link-local `fe80::/64`. **No IPv6 connection from the internet can reach this server.** The CPU numbers corroborate it from the other side: the two `[::]` proxies show the same 3 seconds as the two `0.0.0.0` ones, i.e. they have handled essentially nothing. The count of IPv6 connections carried by `docker-proxy` on this host is, to a very good approximation, **zero**.

So the claim is **vacuously true and presented as an empirical demonstration.** The *structural* argument — a NAT rule cannot change address family, so `[::]:80 → 172.18.0.4:80` is impossible with DNAT and requires a process that terminates one connection and opens another — is **correct**, is the real content, and is independently supported by Docker's own docs. But it is a **fact about NAT**, not an observation about this box, and the document tags it `[O]` and says *"this host proves it"* and *"Verified live above."* That is precisely the single-host-observation-masquerading-as-fact failure mode, in the direction of over-claiming the evidence rather than over-generalizing it.

**Verdict: MISCLASSIFIED.** Retag the IPv6→IPv4 argument `[F] HIGH` (kernel/NAT semantics + vendor docs) and delete "this host proves it". The honest `[O]` on this host is narrower and still useful: *"a host with no IPv6 connectivity still gets four `docker-proxy` processes, two of which bind `[::]` and will never see a packet — i.e. Docker starts the address-family-bridging helper unconditionally, without checking whether the host has IPv6 at all."* That is a real observation, it is one this host does prove, and it is directly actionable for §4.4's recommendation: **our helper should be started only when the host actually has a global address in the family being bridged.** Two idle supervised processes per published port, on the majority of hosts that have no IPv6, is pure cost.

**§4.4's `route_localnet` account — CONFIRMED.** `route_localnet` is `0` on **every** interface on this host — `all`, `default`, `lo`, `eth0`, `eth1`, all three bridges (checked individually). Consistent with the document's reading: userland-proxy is on, nothing is published to a loopback address, so Docker never set it. The `ip-sysctl.rst` quote is verbatim correct (*"Do not consider loopback addresses as martian source or destination while routing… default FALSE"*). The pre-28.0.0 L2 leak is confirmed in Docker's own docs, quoted verbatim: *"In releases older than 28.0.0, hosts within the same L2 segment (for example, hosts connected to the same network switch) can reach ports published to localhost."* Note the docs do **not** name `route_localnet` — the document supplies that mechanism itself, and it is the right mechanism, but it is an inference rather than a quote. Correctly tagged `[F]/[C] MED–HIGH`.

**§4.4's recommendation is improved by the above.** "Do not use `route_localnet`; a `bind()` is free" — correct and well-argued. Add: gate the helper on the host actually having an address in that family.

### C4. §4.5 Docker 28 `raw/PREROUTING` hardening — **CONFIRMED**

Seven rules, one per live container IP, exact shape `-d <ip> ! -i <its bridge> -j DROP`, all at `raw` priority -300 (before conntrack, before nat), all with zero counters. `gateway_mode` values confirmed against Docker's docs: `nat` (default), `nat-unprotected`, `routed`, `isolated`. The `routed` description — *"sets no NAT/masquerading but maintains firewall rules"* — matters for §11 below.

### C5. §4.6 `DOCKER-USER` and the ufw bypass — **CONFIRMED, and it is stronger than the document says**

Both mechanisms verified: ufw's user policy lives in `ufw-user-input` off **`INPUT`**, published-port traffic traverses **`FORWARD`**, and Docker's two jumps sit at positions 1 and 2 of `FORWARD`, ahead of all six ufw chains. A terminal `ACCEPT` in `DOCKER-FORWARD` ends traversal of `filter` at the forward hook, so the ufw chains are dead code for this traffic. `DOCKER-USER` exists and is empty. `-P FORWARD DROP`.

**Strengthening the claim:** the document says *"`ufw deny 80` does not stop `-p 80:80`."* True — but a knowledgeable admin's *correct* incantation, `ufw route deny 80`, **also fails**, because it lands in `ufw-user-forward`, reached from `ufw-before-forward` at FORWARD position 4, i.e. still after `DOCKER-FORWARD` has already ACCEPTed. So there is **no ufw command at all** that closes a Docker-published port on this host; the only working lever is `DOCKER-USER`. That makes §4.6's recommendation (warn the user on first `-p` when a foreign firewall manager is detected) more valuable, not less, and it is the strongest security finding in the whole document — stronger than the `ip_forward` claim it displaces (see A1).

---
## PART D — §5: iptables vs nftables

### D1. §5.1 the `iptables-nft` translation layer and the dual view — **CONFIRMED**

Every element reproduced live:
- `nft list tables` → `table ip filter`, `table ip6 filter`, `table ip nat`, `table ip6 nat`, `table ip raw`, `table inet f2b-table`. Exactly the document's list.
- The udata marker warning is real: `nft list table ip nat` prints `# Warning: table ip nat is managed by iptables-nft, do not touch!`
- The mechanical translation is exactly as described. `-m addrtype --dst-type LOCAL` → `fib daddr type local`; `-i`/`-o` → `iifname`/`oifname`; `-s` → `ip saddr`; `-j MASQUERADE` → `masquerade`; and `counter` is added to every translated rule.
- **The invisibility claim, verified as a hard number:** `iptables -t filter -S | grep -ci f2b` → **0**. fail2ban's rule is live and currently rejecting `198.51.100.30` on port 22, and `iptables-save` does not mention it. An admin auditing this host with `iptables-save` would conclude fail2ban is not running. **CONFIRMED.**

### D2. §5.2 the ordering hazard — **CONFIRMED**

```
table inet f2b-table {
    set addr-set-sshd { type ipv4_addr; elements = { 198.51.100.30 } }
    chain f2b-chain {
        type filter hook input priority filter - 1; policy accept;
        tcp dport 22 ip saddr @addr-set-sshd reject with icmp port-unreachable
    }
}
```
`priority filter - 1` = **-1**, one below the `filter` (0) priority where `iptables-nft`'s `INPUT` chain lives, exactly as the document states. Three independent firewall writers on a stock Ubuntu server (Docker and ufw via iptables-nft, fail2ban via native nft), two of the three invisible to the other's tooling. **CONFIRMED**, and it is a fair generalization — this is a default package configuration on the most common server distro, not an exotic setup.

The general statement (within a table, order is position; across tables at one hook, priority decides; a `drop` anywhere is final and an `accept` is per-table) is correct nftables semantics.

### D3. §5.3 "what Docker and Podman do today" — **Docker part CONFIRMED with an addition; netavark part OUTDATED**

**Docker — CONFIRMED, plus two things the document should carry.**
`docker info` on the host → `{"Driver":"iptables"}`, confirming the `FirewallBackend` field and the iptables default. Two additions from current docs:
1. There is a **`--firewall-backend`** daemon flag, documented as *"Firewall backend to use, iptables or nftables."* The document mentions the API surface but not the operator-facing switch.
2. Docker's packet-filtering documentation calls the nftables backend **experimental**, and — importantly — notes a behavioural *difference*: *"With the experimental nftables backend, Docker does not enable IP forwarding itself."* That partially contradicts the document's quoted line *"For bridge networks, iptables and nftables have the same functionality."* Both sentences are on Docker's docs; they are not consistent. **CORRECTED:** the backends are functionally equivalent for bridge-network *rules*, but they differ on whether the daemon flips `ip_forward` for you. Worth knowing, because §11's ladder puts that sysctl write at stage 3.

**netavark — OUTDATED.** §5.3 says:
> *"netavark has **both** an `iptables` and an `nftables` firewall driver, and **nftables is now the default; the iptables driver is deprecated and slated for removal**."* … `[LOW]` on the exact removal timeline.

The removal has **already happened** on `main`. Two independent checks:
1. `containers/netavark/src/firewall/` contains exactly five files: `firewalld.rs`, `fwnone.rs`, `mod.rs`, `nft.rs`, `state.rs`. **There is no `iptables.rs`.**
2. `src/firewall/mod.rs` defines only `FIREWALLD`, `NFTABLES`, `NONE` as driver-name constants, and the compile-time default is selected between `#[cfg(default_fw = "nftables")]` and `#[cfg(default_fw = "none")]`. **The string `iptables` does not appear as a driver at all.**

So the correct present-tense statement is: *"netavark's iptables driver has been removed; the drivers are `nftables`, `firewalld` and `none`, and `nftables` is the compile-time default."* The Fedora change proposal the document cites (`Changes/NetavarkNftablesDefault`, **Fedora 41**) only ever proposed *switching the default* and explicitly kept a "keep using iptables as default" contingency — it never announced deprecation. The document's `[LOW]` hedge on the timeline was the right instinct; the answer is that the timeline was *shorter* than "next year", not longer. **OUTDATED — corrected to "already removed".**

This is a net *strengthening* of §5.4: the document argues "direction of travel — netavark has already made nftables the default and deprecated iptables." The reality is one step further along, so the argument is stronger than stated.

### D4. §5.4's recommendation and the §12.3 self-correction — **the self-correction is itself CORRECT**

§12.3 corrects §5.4's `[R] MED` (take the `nftnl` crate / `libnftnl`) with: nobody in this space drives nftables via `libnftnl`; moby uses `nft -f -` (default build) or `libnftables` via cgo, and netavark uses `nft -j` through the `nftables-rs` crate. **Verified:**
- netavark's `Cargo.toml` at `main` declares `nftables = "0.6.3"` and no `nftnl`, no `rustables`, no `libnftnl`-adjacent crate. Its only nftables source file is `nft.rs`.
- The `libnftnl` vs `libnftables` distinction §12.3 draws is the real and frequently-confused one: `libnftnl` builds netlink messages; `libnftables` is the parser/engine behind the `nft` binary and takes the same text/JSON syntax. Moby's cgo build tag is `libnftables`, not `libnftnl`.

**The self-correction is right, and §5.4's `[R] MED` should be treated as superseded rather than merely annotated.** The document does mark it *"superseded by §12.3; read that first"*, which is honest but leaves a wrong recommendation in place three sections earlier. Recommend deleting the `nftnl` paragraph outright.

**One genuine weakness in §5.4 that survives.** Tradeoff bullet 3 — *"Coexistence with an iptables-based `FORWARD DROP` policy… a DROP anywhere wins"* — is correct and is correctly labelled the strongest argument against native nft. This host demonstrates it: `-P FORWARD DROP` is set in `table ip filter`. If we ship a `table inet mydocker` with an `accept` at the forward hook, **that policy DROP still applies**, because policy is evaluated at the end of the *base chain* in `table ip filter` and an `accept` in a different table does not suppress it. The document states this. What it does not state, and should: the mitigation is not only "warn" — it is to also add an `accept` rule *ahead of* the foreign chain in a table at a **lower priority number**, which nftables lets us do and iptables does not. That is §5.4's own point 3 applied to its own point 6, and the document never joins them.

---

## PART E — §6: DNS

### E1. §6.1 the `127.0.0.11` embedded resolver — **CONFIRMED end to end, exactly**

Every artefact reproduced live from the nginx container (pid 2588141):
```
/etc/resolv.conf  ->  nameserver 127.0.0.11 / search . / options edns0 trust-ad ndots:0
                      # Based on host file: '/etc/resolv.conf' (internal resolver)
                      # ExtServers: [host(127.0.0.53)]

netns nat:  -A OUTPUT      -d 127.0.0.11/32 -j DOCKER_OUTPUT
            -A POSTROUTING -d 127.0.0.11/32 -j DOCKER_POSTROUTING
            -A DOCKER_OUTPUT      -d 127.0.0.11/32 -p tcp --dport 53 -j DNAT --to-destination 127.0.0.11:39251
            -A DOCKER_OUTPUT      -d 127.0.0.11/32 -p udp --dport 53 -j DNAT --to-destination 127.0.0.11:55634
            -A DOCKER_POSTROUTING -s 127.0.0.11/32 -p tcp --sport 39251 -j SNAT --to-source :53
            -A DOCKER_POSTROUTING -s 127.0.0.11/32 -p udp --sport 55634 -j SNAT --to-source :53

netns sockets: UNCONN 127.0.0.11:55634  dockerd pid=70234 fd=52
               LISTEN 127.0.0.11:39251  dockerd pid=70234 fd=54
```
Identical port numbers, identical fds, identical pid — the document's observation is exact and reproducible 3 days later. The socket-creation trick of §1.5 (socket's netns fixed at `socket(2)` time from the creating thread's netns) is demonstrated live: `dockerd` lives in the host netns and owns two listeners inside a container netns. **CONFIRMED.** This is the single best-evidenced observation in the document.

The SNAT-back-to-`:53` rationale (glibc and musl both verify the reply comes from the address:port queried) is correct and is standard resolver behaviour.

**The `[MED]` port rationale — the hedge is CORRECT, and it stays UNSUPPORTED.** §6.1 argues the random-high-port design exists for *conflict avoidance* with a container that binds `0.0.0.0:53`, and honestly says *"I did not find it written down by a Docker maintainer. What would settle it: the commit message / design doc for libnetwork's resolver."* I looked: `daemon/libnetwork/resolver.go` on `master` contains **no comment explaining the choice**. It defines `dnsPort = "53"`, calls `r.setupNAT()` with the error string `"setting up DNAT/SNAT rules failed"`, and takes the port as a parameter to `SetupFunc(port uint16)` — i.e. the socket is bound ephemeral and the port read back. **The document's `[MED]` and its "what would settle it" both stand unchanged.** The proposed explanation remains the only one consistent with the observed design, and it is still not sourced. Verdict: **UNSUPPORTED (correctly self-flagged).** The practical corollary the document draws — the port is per-container and random, so the rules must be written after the bind — is directly observable and is the part that matters for design.

### E2. §6.2 `/etc/hosts` and `/etc/resolv.conf` generation — **CONFIRMED**

The generated `/etc/hosts` is reproduced exactly: six boilerplate lines plus `172.18.0.4	4a820bcb9df9`. Both files are bind-mounted from `/var/lib/docker/containers/<id>/`, not part of the image.

### E3. §6.3 the systemd-resolved interaction — **CONFIRMED, and this is the one place a single-host observation IS a fair generalization**

```
$ readlink -f /etc/resolv.conf   -> /run/systemd/resolve/stub-resolv.conf
$ cat /etc/resolv.conf           -> nameserver 127.0.0.53 / options edns0 trust-ad / search .
$ ss -ltnp | grep :53
LISTEN 127.0.0.54:53      systemd-resolve pid=551 fd=17
LISTEN 127.0.0.53%lo:53   systemd-resolve pid=551 fd=15
```
Note `127.0.0.53%lo` — the stub is bound **scoped to `lo`**, which makes the document's point sharper than it makes it: the stub is not merely at a loopback address, it is bound to the host's loopback *device*, so it is unreachable from a container netns twice over.

The mechanism claim — *"`127.0.0.53` is meaningless inside a container netns; each netns has its own loopback"* — is a fact about Linux, not about this box, and is correctly tagged `[F] HIGH`. The *prevalence* claim (systemd-resolved is the default on Ubuntu, Fedora, Arch) is a claim about distros, verifiable independently, and is right. Docker's own annotation in the generated file (`(internal resolver)` and `ExtServers: [host(127.0.0.53)]`) is reproduced verbatim and is the document's own primary source. **CONFIRMED.**

§6.3's `[F] MED` on Docker's *precise* decision tree ("if every nameserver is loopback, fall back to the embedded resolver") remains an inference from the generated file's comments plus documented behaviour, correctly hedged. Not settled here either; `libnetwork/resolvconf` remains the thing to read.

### E4. §6.5's three-stage ladder — sound, with one caveat

Stage 1 (filter loopback nameservers out of the host `resolv.conf`, fall back to a configured default, refuse to hardcode `8.8.8.8`) is correct and is genuinely ~30 lines. Stage 2's honest ceiling (a `/etc/hosts` snapshot goes stale on restart-with-new-IP, the same flaw that killed `--link`) is correctly stated.

**Caveat on §6.5 stage 3's shortcut.** The document recommends *"prefer binding `127.0.0.11:53` directly and skipping the DNAT/SNAT rules entirely."* That is a reasonable trade, but note it costs something the document does not price: **you can no longer serve DNS to a container that shares the netns** (`--network=container:X`, pods — §7.3) *and* runs its own `0.0.0.0:53`, and the failure is at container-*start* for the second container, not at resolver-setup. Also, with the DNAT design the resolver's socket can be re-bound on a different port without touching the container's view; with a fixed `:53` bind it cannot. Small, but the "clear, loud failure" the document promises lands on the *user's* container rather than on us.

---
## PART F — §11: the corrected ladder, attacked

The prompt asks me to attack three specific claims. Two survive; one does not, and its failure changes the ladder.

### F1. "`--network=host` is not rung 0 but a permanent orthogonal mode" — **CONFIRMED in substance, CORRECTED on cost, and the framing is slightly inflated**

**The substance is right.** `--network=host` shares no code with anything above it, is never superseded (Docker still ships it; it is the correct answer for high-throughput workloads; §8.6 makes it one of the two honest answers for rootless users), and in OCI terms it is the *absence* of a `{"type": "network"}` entry in `linux.namespaces`. Calling it "rung 0" does imply you grow past it, and you don't. **CONFIRMED.**

**CORRECTED — "1 line" is wrong, and the table's "zero isolation" understates a specific hazard.**

1. **It is not one line, because `-p` must be suppressed.** Publishing a port is meaningless in host netns (the container *is* on the host's addresses), and Docker warns and discards published ports in this mode. An engine that omits `CLONE_NEWNET` and then still runs its §4 DNAT/anti-spoof/helper path against a container with no distinct IP will write nonsense rules. Mode H is "omit one clone flag **and short-circuit the entire §4 publish path**". Call it ~10 lines with a real branch in the port code, not 1.
2. **`/etc/resolv.conf` still has to be decided.** In host netns the host's `127.0.0.53` stub *is* reachable, so — uniquely — copying the host file verbatim is correct here and wrong everywhere else (§6.3). That is a second branch.
3. **The isolation hole worth naming explicitly: the abstract UNIX socket namespace is per-netns.** `network_namespaces(7)` states that a network namespace isolates *"the UNIX domain abstract socket namespace"*. So `--network=host` hands the container the host's **abstract** socket namespace — every `@`-prefixed listener a host daemon has open. That is a well-known lateral-movement surface and it is not what most people picture when they read "zero isolation, zero overhead". It belongs in the table's cost column.

**Framing note.** The document presents this as a *correction* of the proposed ladder. It is really a terminological preference: the original ladder plausibly meant "ordered by implementation effort", in which "you have written no networking code yet" is a legitimate first state. The substantive content is right and useful; the "it is wrong in one specific place" framing overstates the disagreement. Minor, but this is a document that elsewhere is careful about the fact/recommendation boundary.

### F2. "The first genuinely useful rung is bridge+NAT, because before it images cannot `apt-get`" — **CORRECTED: the bridge is not the ingredient that makes it useful**

**The `apt-get` argument itself is right**, and the document even catches the trap in it: stage 2 as defined (veth + addresses + manual routes, *no* forwarding, *no* MASQUERADE) gives host↔container only, and a container's RFC1918 source has no return path from the internet. And egress without DNS is useless on systemd-resolved hosts, which §11.1's first ordering note correctly folds `resolv.conf` rewriting into stage 3 to fix. **All correct.**

**But stage 3 bundles two independent things and only one of them is load-bearing.** Stage 3 is "+ bridge, `ip_forward`, MASQUERADE". Delete the bridge and you still have full internet egress:
- veth pair, host end addressed (a /30 or /31 per container), container end addressed, default route via the host end;
- `net.ipv4.ip_forward=1`;
- `MASQUERADE` for the container prefix.

That is a complete, working, `apt-get`-capable container network **with no bridge at all**. It is not hypothetical: it is exactly the **CNI `ptp` plugin**, whose documentation says *"The ptp plugin creates a point-to-point link between a container and the host by using a veth device"*, with an `ipMasq` option — *"set up IP Masquerade on the host for traffic originating from ip of this network and destined outside of this network"* — and an `ipMasqBackend` choosing iptables or nftables. No bridge is involved anywhere in it.

**So the correct statement is: the first genuinely useful rung is NAT egress + DNS, which requires veth + `ip_forward` + MASQUERADE + `resolv.conf`, and does *not* require a bridge.** The bridge buys exactly two things: container↔container **L2** adjacency, and a single gateway address shared by many containers instead of a /30 each. Both are real, neither is what makes `apt-get` work.

Why this matters for the roadmap rather than being pedantry: the bridge is where several of the document's own hard questions live — `bridge-nf-call-iptables` (§3.4), hairpin (§3.5), `IFLA_BRPORT_ISOLATED` vs `--icc` (§7.2), FDB/STP behaviour (§3.1), MTU-inheritance-that-doesn't-happen (§2.5), and the `br-*` device that leaks forever (§9.1, and §9.3's live leak is *a bridge*). A routed stage that reaches the internet with **zero** of that is a strictly cheaper first useful rung. The document's own §9.1 table lists the bridge as class C, "leaks forever" — and the routed model has no class-C device at all, because the veth is class A (self-healing).

**Recommended re-split of stage 3:**
- **3a — routed NAT egress.** veth (no bridge) + `ip_forward` + MASQUERADE + `resolv.conf` rewriting. This is the first user-useful rung. Host state created: one nft table, one global sysctl, **no bridge**.
- **3b — bridge.** Add the bridge when container↔container by L2, a shared gateway, or address density actually demands it. This is where §3.4/§3.5/§7.2 get paid for.

### F3. "Stage 2 is not user-useful, a test fixture rather than a feature" — **REFUTED as stated; it understates what a routed setup gives you**

Two independent counter-examples, both shipping:

1. **CNI `ptp`** (above) is a first-class, widely-deployed main plugin whose entire model is "veth + routes, no bridge". Kubernetes clusters run on it.
2. **Docker 28's own `gateway_mode: routed`**, confirmed in Docker's port-publishing docs: it *"sets no NAT/masquerading but maintains firewall rules"* — i.e. real container addresses, routed, with published-ports-only reachability preserved. That is precisely a productised "stage 2 plus filtering".

And a routed model has architectural advantages the document never credits:
- **Intra-network policy becomes ordinary L3 filtering.** §3.4 spends a page on why `--icc=false` is hard: same-bridge traffic is L2-switched and invisible to iptables unless you load `br_netfilter` (a global module with conntrack side-effects) or use `IFLA_BRPORT_ISOLATED` (all-or-nothing per port). **On a routed setup none of that exists** — container↔container traffic is *routed through the host*, so it traverses `filter/FORWARD` and per-pair policy is a normal rule. The document's own conclusion that *"anyone needing real policy should use separate networks"* is a limitation of the bridge, not of container networking.
- **No `br_netfilter` question, no hairpin question, no STP question, no FDB, no bridge MTU-inheritance surprise** (§2.5's finding that veth does *not* inherit the bridge MTU stops mattering when there is no bridge).
- **Nothing in class C.** No shared device with a "who deletes it?" problem (§9.6's answer, "nobody does", becomes moot).

Costs of routed, stated fairly, because they are why Docker defaults to bridge: an address (or a /30) per container instead of one gateway for all; a host route per container; container↔container traffic pays a routing hop instead of an L2 hop; and broadcast/multicast between containers does not work (which some legacy software wants).

**Verdict: REFUTED.** Stage 2's honest label is not "test fixture" but **"the routed model, one step short of useful"** — and the step is two rules, not a bridge. Keeping "test fixture" in the document risks the project skipping straight to a bridge and inheriting §3.4/§3.5/§7.2/§9's whole bridge tax on day one for a benefit (L2 adjacency) that a single-host MVP may never need.

### F4. §11.2's remaining answers — mostly CONFIRMED, one ranking REFUTED

- *"Which stage carries the most host-state risk? Stage 4"* — **CONFIRMED**, and the five sub-reasons (specific-IP rules, a bound host port, conntrack, a shared ruleset with ufw/firewalld/fail2ban, per-container frequency) are each independently verified in §4 and §9.
- *"The single most dangerous operation is stage 3's `ip_forward` write"* — **REFUTED**, see A1. The kernel does not do what the claim says. The most dangerous *operation* in the ladder is stage 4's first `-p` publish on a host running a foreign firewall manager (§4.6), because it silently defeats the admin's firewall with no warning and no ufw command can undo it.
- *"Where does the OCI boundary sit — build networking in, not as a hook"* — **CONFIRMED.** The three decisive properties are correctly quoted from `runtime.md`: `createRuntime` failure is fatal, `poststop` failure is *specified* to be ignored, and a hook has no return channel. Point 3 (an allocation that cannot be reported is not an interface, it is a side effect) is the strongest argument in the section and is sound.
- *"Support hooks for other people's plugins; keep the internal trait CNI-shaped"* — sound, and §9.5's observation that CNI's `GC` verb *is* the reconcile pass §9.7 needs is a genuinely good structural insight, independently confirmed against the CNI spec.

---
## PART G — §8: rootless networking

### G1. §8.5 "the widely-repeated claim is wrong" about privileged ports — **CONFIRMED on the kernel rule, CORRECTED on the practical consequence**

**The kernel citations are exact.** `net/ipv4/af_inet.c` at v6.11, verbatim:
```c
err = -EACCES;
if (!(flags & BIND_NO_CAP_NET_BIND_SERVICE) &&
    snum && inet_port_requires_bind_service(net, snum) &&
    !ns_capable(net->user_ns, CAP_NET_BIND_SERVICE))
	goto out;
```
Reproduced character-for-character from the document. `inet_port_requires_bind_service(net, port)` returns `port < READ_ONCE(net->ipv4.sysctl_ip_prot_sock)` and lives in `include/net/ip.h`, not `af_inet.c` — a trivial citation imprecision, the code itself is right.

**The rule, stated precisely, is what the document says it is:** both the *threshold* (`net->ipv4.sysctl_ip_prot_sock`, per-netns, default 1024) **and** the *capability* (`ns_capable(net->user_ns, CAP_NET_BIND_SERVICE)`, evaluated against the **netns's owning user namespace**) are namespaced. The `ip-sysctl.rst` entry is quoted correctly (*"This is a per-namespace sysctl… Privileged ports require root or CAP_NET_BIND_SERVICE in order to bind to them. To disable all privileged ports, set this to 0."*). **CONFIRMED.**

**CORRECTED — the "no sysctl change at all" conclusion holds only while the process still has the capability, and in practice that is often false.**

§8.5 concludes: *"Inside a rootless container's netns — owned by the userns where we are uid 0 — `ns_capable(net->user_ns, CAP_NET_BIND_SERVICE)` succeeds, so **nginx binds `:80` with no sysctl change at all.**"*

That is true for a process that *retains* `CAP_NET_BIND_SERVICE`. It is false for the extremely common case of an image that runs as a non-root `USER` with no file capabilities: the effective set is empty, `ns_capable` fails regardless of who owns the netns, and the bind returns `-EACCES`. The userns ownership buys you nothing if the capability has been dropped.

**And this is not theoretical — it is what Docker actually does about it, observable live on this host.** All five containers report:
```
$ docker inspect <id> --format '{{json .HostConfig.Sysctls}}'   ->  null    (all five)
$ nsenter -t <pid> -n cat /proc/sys/net/ipv4/ip_unprivileged_port_start
0        <- for ALL FIVE containers
```
Host value: `1024`. Container value: `0`. **No container requested it** (`HostConfig.Sysctls` is `null` for every one). **Docker sets `net.ipv4.ip_unprivileged_port_start=0` in every container netns, by default, unasked.** It also keeps `CAP_NET_BIND_SERVICE` in the default bounding set (`CapEff: 00000000a80425fb`, bit 10 set) — so it does *both*, belt and braces, and the sysctl is the half that covers the non-root-USER case the capability does not.

**So the document's correction is right about the folklore and incomplete about the fix.** The corrected remedy list for §8.5:

| Case | What actually gates it | Remedy |
|---|---|---|
| Container process binds `:80` **inside** its own netns, as uid 0 with caps | `ns_capable(net->user_ns, CAP_NET_BIND_SERVICE)` — succeeds | nothing needed |
| Container process binds `:80` inside its netns as a **non-root USER** | capability gone ⇒ fails | **set `ip_unprivileged_port_start=0` in the container netns** — per-netns, free, no host mutation. *This is what Docker does and the document omits it.* |
| Publishing to a **host** port `< 1024` from a rootless engine | `ns_capable(init_user_ns, …)` in the initial netns — fails | the document's three remedies (use ≥1024 / host-wide sysctl / setuid file cap) are correct and correctly ordered |

The document's remedy 2 (`net.ipv4.ip_unprivileged_port_start=0` **on the host**) is correctly flagged as a persistent global mutation to avoid. Its own better answer — the per-netns version — is present in §8.2's `ping_group_range` discussion (*"it is per-netns and writable by userns-root, so we can fix this ourselves inside the container netns"*) but is never applied to the port threshold. Applying it closes the gap.

**Bonus live confirmation of §8.2's `ping_group_range` recommendation.** Host: `1 0` (nobody, the upstream default). Every container netns: `0 2147483647`. **Docker widens it per container, exactly as §8.2 recommends we do.** The document reached the right conclusion from `old-host` plus libslirp source; this is independent confirmation from Docker's production behaviour on a different host and kernel. **CONFIRMED and strengthened.**

### G2. §8.1–§8.4, §8.6, §8.7 — **UNSUPPORTED — pending host** (not disbelieved; unverifiable from here)

Every empirical claim in these subsections rests on `old-host`, which is unreachable (`old-host.example.edu` → NXDOMAIN). The kernel-source halves are verifiable and I checked them; the observation halves are not. Specifically pending:

| Claim | Status of the source half | Command that would settle the observation |
|---|---|---|
| §8.1 the `EPERM` wall: veth into the host netns from `unshare -Urn` fails | `rtnl_link_get_net_capable()` → `netlink_ns_capable(skb, net->user_ns, CAP_NET_ADMIN)` **verified in v6.11 source** | `sleep 300 & H=$!; unshare -Urn sh -c "ip link add v0 type veth peer name v1; ip link set v1 netns $H; echo exit=\$?"` |
| §8.1 full bridge+veth+nft-nat stack works *inside* `unshare -Urn` | plausible from the same source | `unshare -Urn sh -c 'ip link add br0 type bridge && ip link add v0 type veth peer name v1 && ip link set v0 master br0 && ip addr add 10.99.0.1/24 dev br0 && ip link set br0 up && nft add table ip t && nft add chain ip t c "{ type nat hook postrouting priority srcnat; }"'` |
| §1.4 / §8.2 the `setns` double-`CAP_SYS_ADMIN` check and the slirp4netns failure | `netns_install()` **verified verbatim in v6.11** — both `ns_capable(net->user_ns,…)` and `ns_capable(nsset->cred->user_ns,…)` | `unshare -Urn sh -c 'ip link set lo up; sleep 8' & sleep 1; slirp4netns --configure --mtu=65520 $! tap0` |
| §8.1 `mkdir /var/run/netns` fails as root-in-userns | fact about DAC vs capabilities, sound | `unshare -Urmn sh -c 'mkdir -p /var/run/netns'` |
| §1.1 fresh-netns sysctl table (`ip_forward=1`, `ping_group_range=65534 65534`, etc.) | see H1 below — **the `ip_forward` half is now independently confirmed on a different host** | `unshare -Urn sh -c 'for f in ipv4/ip_forward ipv4/ping_group_range ipv4/ip_unprivileged_port_start ipv4/conf/all/rp_filter; do printf "%s=" $f; cat /proc/sys/net/$f; done'` |
| §8.5 `BIND_80_OK` inside `unshare -Urn` after setting the sysctl to 80 | kernel path **verified**; the observation is a direct consequence | as printed in §8.5 |
| §8.7 performance table (RootlessKit CI / passt CI figures) | figures are from published CI, not from `old-host`; not host-dependent | n/a — but the flagged 10× pasta discrepancy (0.24 vs 2.5 Gbps @1500) is still **unresolved**, as the document says |

§8.3 (passt/pasta architecture), §8.4 (RootlessKit port drivers), §8.6 (`lxc-user-nic`) are documentation-derived and read correctly against the cited man pages and READMEs; I found nothing to contradict them and nothing on this host that bears on them. The §8.3 correction — *"pasta **does** use a tap device; what it avoids is the tap for local traffic"* — is right, and is a genuine correction of a widespread misreading.

---

## PART H — LOW-confidence claims, and remaining spot checks

### H1. §1.1 "`ip_forward = 1` in a fresh netns is inherited, not the documented default 0" — **CONFIRMED, and the LOW-confidence mechanism is now settled**

The document marks the *observation* HIGH and the *mechanism* `[LOW]`: *"The likely mechanism is that new netns copy the `ipv4_devconf` template, which the initial netns has already mutated… What would settle it: `devinet_init_net()` in `net/ipv4/devinet.c` for this kernel, plus a repeat on a host where `ip_forward=0`."*

**Both halves are now settled, on a completely different host and kernel (6.11 vs 4.18):**
- `net.core.devconf_inherit_init_net = 0` on the host — the default. That sysctl is exactly the knob `devinet_init_net()` consults via `net_inherit_devconf()`: `0`/`1` copy `init_net`'s **live** `devconf_all`/`devconf_dflt`, `2` uses compiled-in defaults, `3` copies from the *current* netns.
- Host `net.ipv4.ip_forward = 1`. Every container netns on this host: **`ip_forward = 1`**, verified inside the nginx netns.

So the mechanism is confirmed and namable: **a new netns inherits `ipv4_devconf` from `init_net`'s current values, governed by `net.core.devconf_inherit_init_net` (default 0)**. The documented "default 0" describes a freshly booted `init_net`, not a fresh child netns. **Upgrade §1.1's mechanism from `[LOW]` to `[F] HIGH`, and cite `net.core.devconf_inherit_init_net`.** The design advice — *"do not assume a fresh netns has forwarding off; read it, don't guess"* — is correct and now well-founded.

### H2. §12.2 "`rust-netlink` is maintained by the nmstate/Red Hat networking people" — **UNSUPPORTED**, and the contrast it draws is weaker than presented

The document tags this `[LOW]` and says it is *"inferred from the maintainer set and downstream consumers, not from a governance document."* That hedge is correct and I could not improve on it: the `rust-netlink` GitHub organisation has **no governance document, no README, and no organisation description mentioning Red Hat or nmstate**. Its self-description is *"building blocks for netlink"*. The visible members are **@cathay4t and @JohnTitor**. The inference is defensible (@cathay4t is the nmstate lead) but it is an inference about one person's employer, not about project governance.

**The comparison it supports is the part that should be corrected.** §12.2 contrasts `neli`'s *"bus factor of one"* against `rust-netlink`, *"which is a GitHub **organisation**"*. Two visible members, one of whom does the bulk of the work, is **not a materially better bus factor** — it is the same shape with a different noun. The genuine advantages of `rust-netlink` over `neli` are the two technical ones the document already gives (a 293-file rtnetlink attribute surface including `InfoVeth::Peer`, and consumption by netavark), and those stand on their own. **Drop the "organisation vs one person" argument; it does not survive looking at the org page.**

### H3. §5.3's netavark iptables-removal timeline `[LOW]` — **OUTDATED**, see D3. The removal has already happened on `main`.

### H4. §6.1's embedded-resolver port rationale `[MED]` — **UNSUPPORTED (correctly self-flagged)**, see E1. No comment in `daemon/libnetwork/resolver.go` explains the choice; the document's hedge and "what would settle it" stand verbatim.

### H5. §9.7's boot-time GC unit `[R] LOW` — **reasoning CONFIRMED**

The stated reasoning is *"a reboot clears every class-A and class-C kernel resource but leaves our on-disk records claiming they exist."* Correct: a reboot destroys netns, veths, bridges, nft rules and every network sysctl. It also clears `/run` (tmpfs), which means netns bind mounts under `/run/mydocker/netns/` are **self-clearing at boot** — the class-C leak the document worries about most (§1.6, §9.1) does not survive a reboot. What *does* survive is the lease directory under `/var/lib/mydocker`, so a boot-time GC pass is exactly right and its scope is exactly "leases and container state records", not kernel resources. The `[R] LOW` tag is honest about the unknown (how boot identity is represented in the records), and the reasoning underneath it is sound. **No correction; the confidence tag can rise to MED once D2 §6.4's `boot_id` check is specified.**

### H6. Remaining spot checks against the live host — all **CONFIRMED**

| §  | Claim | Live result |
|---|---|---|
| 1.2 | `ip netns list` reports none of Docker's namespaces | Empty, exit 0. `/var/run/netns/` is an empty directory. Five nsfs binds under `/run/docker/netns/`. **CONFIRMED** — the cleanest possible proof that the path is a userspace convention, corroborated by `ip-netns(8)`: *"By convention a named network namespace is an object at /var/run/netns/NAME that can be opened."* |
| 1.2 | `readlink /proc/2588141/ns/net` → `net:[4026532504]` | **Identical**, 3 days later |
| 1.2 | nsfs bind mounts | `462 29 0:4 net:[4026532363] /run/docker/netns/232afd7802e3 … nsfs nsfs rw` — mount type `nsfs`, source `net:[…]` |
| 1.6 | `network_namespaces(7)`: physical devices migrate back, veth devices are destroyed | Both quoted **verbatim correctly** from the man page |
| 3.1 | bridge FDB: learned vs `permanent` | Confirmed. Learned (non-`permanent`, no `vlan`) entries present: `02:00:00:dd:79:00 dev veth28c3b49`, `02:00:00:8b:f6:75 dev vethd754244`. Minor drift: `ip -d link` reports `fdb_n_learned 3` while `bridge fdb` currently shows 2 learned — an entry aged out between commands. Immaterial. |
| 3.1 | bridge defaults Docker relies on | `stp_state 0`, `forward_delay 1500`, `vlan_filtering 0`, `mcast_snooping 1`, `nf_call_iptables 0`, `ageing_time 30000` — **all exact** |
| 3.1 | `docker0` is `NO-CARRIER`/`state DOWN` with a `linkdown` route because it has zero ports | Confirmed, and so is `br-ac0ab0eb6175` |
| 3.2 | the single MASQUERADE rule and its `! -o <bridge>` exclusion | Confirmed; counter now `51517 pkts / 3388K bytes` |
| 3.4 | global `bridge-nf-call-iptables=1` but per-bridge `nf_call_iptables 0` | **Confirmed on all three bridges.** The two-independent-controls story is right. |
| 3.4 | `IFLA_BRPORT_ISOLATED` is exposed on this kernel | `isolated off` present on all five veths in `ip -d link` |
| 3.5 | complete container-side state: address, kernel connected route, default route, nothing else | Confirmed verbatim inside the nginx netns |
| 3.5 | every container veth reports `hairpin off` | Confirmed on all five |
| 2.5 | Docker's MTU: bridge, veth and container `eth0` all 1500, matching `eth0` | Confirmed. `com.docker.network.driver.mtu: "1500"` on the default bridge. (Note the *user-defined* network's `Options` is `{}` — the document's `[O]` is from `docker network inspect bridge`, which is what it says.) |
| 10.1 | first-fit allocation `172.17` → `172.18` → `172.19` from the built-in local pool, no `daemon.json` | Confirmed, including the orphan holding `172.19` |
| 10.1 | the host's on-link routes that `InferReservedNetworks` would reserve | `10.0.0.0/16 dev eth0 scope link`, `10.1.0.0/20 dev eth1 scope link`, `198.51.100.0/20 dev eth0 scope link` — **exactly as printed** |
| 10.2 | boltdb key space; addresses only in endpoint records; no IPAM bitmap | Confirmed by parsing the file: `network/` ×4, `endpoint/` ×5 (all under network `5e08a4608e57…`), `endpoint_count/` ×4, `bridge/` ×2, `bridge-endpoint/` ×5, `sandbox/` ×6. **No `ipam/` subtree, no `bitseq`.** |
| 7.2 | `com.docker.network.bridge.enable_icc: "true"` on the default bridge | Confirmed verbatim |
| 4.3 | `nf_conntrack_count` / `max` | `35 / 262144` now (document saw `133 / 262144`) — same order, normal variance |

**§2 (rtnetlink wire format) is not host-checkable and I did not re-derive it.** The kernel-source citations I did spot-check — `netns_install()`, `rtnl_link_get_net_capable()`, `rtnl_dev_combine_flags()`'s `ifi_change == 0` ⇒ `~0` comment, `__inet_bind`'s port check — all reproduced verbatim at v6.11. On that sample the section's citation discipline is good. The one thing I would flag as **unverified rather than wrong**: §2.2's claim that `IFLA_NET_NS_FD` *inside* the `VETH_INFO_PEER` nest creates the peer directly in the target netns with *"Tradeoff: none that I found."* That is a strong "no downside" claim about a non-obvious kernel path; it deserves a round-trip test against a real kernel before it is relied on, which §2.6 already budgets for.

---
## PART I — Verdict summary

**Scope actually examined:** ≈105 discrete claims across §0–§11 and §13–§15, plus §12.1/§12.3's crate-adjacent architectural conclusions. §12's per-crate metrics (LOC, `unsafe` counts, download figures) belong to another verifier and I did not audit them, except `rustables`' licence and the three dependency trees the async question turns on.

### Not-CONFIRMED findings, in order of consequence

| # | § | Claim | Verdict | The correction |
|---|---|---|---|---|
| 1 | 1.1, 9.8, 11.2 | Writing `net.ipv4.ip_forward` *"resets all configuration parameters to their default state"*, blowing away `rp_filter`/`accept_local`/`route_localnet`/`arp_ignore` | **REFUTED** | `inet_forward_change()` (v6.11) sets only `all.accept_redirects = !on`, `default.forwarding = on`, `<iface>.forwarding = on` for every device, and disables LRO on every netdev. Nothing else. An unchanged write is a no-op. Live: `rp_filter=2` survives on a host with `ip_forward=1`. |
| 2 | 11.2 | *"The single most dangerous operation in the ladder is stage 3's `ip_forward` write"* | **REFUTED** (follows from #1) | The most dangerous operation is stage 4's first `-p` publish on a host with a foreign firewall manager (§4.6) — verified, exploitable by accident, and **no ufw command can undo it**, not even `ufw route deny`. |
| 3 | 11.1, 11.2 | Stage 2 (veth + manual routes) is *"not user-useful, a test fixture rather than a feature"*; the first useful rung is bridge+NAT | **REFUTED** | The bridge is not what makes it useful. veth + `ip_forward` + MASQUERADE + `resolv.conf` = full `apt-get`-capable egress **with no bridge** — this is the CNI `ptp` plugin and Docker 28's `gateway_mode: routed`. Split stage 3 into **3a routed NAT egress** (first useful rung, no class-C bridge, L3 policy for free) and **3b bridge** (buys L2 adjacency + a shared gateway, and brings the whole §3.4/§3.5/§7.2/§9 bridge tax). |
| 4 | 4.4 | *"every IPv6 connection to this production web server is being carried by `docker-proxy`… this host proves it"*, tagged `[O] HIGH` | **MISCLASSIFIED** | `ip -6 addr show scope global` is **empty**; only `fe80::/64`. The host has no IPv6 connectivity and carries ~zero IPv6 connections (the `[::]` proxies show the same 3 s CPU as the idle v4 ones). The address-family argument is a **fact about NAT** (`[F]`), not an observation. What this host *does* prove: Docker starts the v6→v4 helper **unconditionally, on a host with no IPv6** — so gate our helper on the host actually having an address in that family. |
| 5 | 5.3 | *"netavark has both an iptables and an nftables firewall driver… the iptables driver is deprecated and slated for removal"* (`[LOW]` on timeline) | **OUTDATED** | Already removed. `src/firewall/` on `main` = `firewalld.rs`, `fwnone.rs`, `mod.rs`, `nft.rs`, `state.rs` — **no `iptables.rs`**; `mod.rs` defines only `FIREWALLD`/`NFTABLES`/`NONE`. The cited Fedora change (F41) only ever proposed switching the default. Net effect: §5.4's "direction of travel" argument is *stronger* than stated. |
| 6 | 9.3 | *"Docker's reconciliation runs at daemon start and this daemon has not restarted since March"* — offered as the cause of the leak | **CORRECTED** | A restart would **not** have cleaned it. `removeIPChains()` removes eleven Docker-owned chains and never touches built-in `nat/POSTROUTING` or `raw/PREROUTING`, where all three leaked rules live; nothing enumerates orphan `br-*` devices. The real lesson — *"the engine wrote state into chains it does not own and lost the key to find it again"* — is a **stronger** argument for §5.4's own table and §9.7's live-set GC than the one made. |
| 7 | 8.5 | *"nginx binds `:80` with no sysctl change at all"* in a rootless container netns | **CORRECTED** | True only while the process retains `CAP_NET_BIND_SERVICE`. For the very common non-root `USER` image it fails. Live: **all five containers have `ip_unprivileged_port_start = 0` with `HostConfig.Sysctls = null`** — Docker sets it per-netns, unasked, and that is the half that covers the case the capability does not. Add the per-netns sysctl as remedy 0. |
| 8 | 11.1 | `--network=host` is *"1 line"* / *"omit `CLONE_NEWNET`"* | **CORRECTED** | Also requires short-circuiting the entire §4 publish path (`-p` is meaningless in host netns) and a second `resolv.conf` branch (the host stub *is* reachable here and nowhere else). And the cost column should name that **the abstract UNIX socket namespace is per-netns** (`network_namespaces(7)`), so host mode exposes every `@`-prefixed host listener. The *substance* — permanent orthogonal mode, not a rung — is CONFIRMED; the "correction" framing is mildly inflated (the original ladder plausibly ordered by effort, not by supersession). |
| 9 | 5.3 | Docker docs quoted as *"For bridge networks, iptables and nftables have the same functionality"* | **CORRECTED** | The same docs also say the nftables backend is **experimental** and that *"With the experimental nftables backend, Docker does not enable IP forwarding itself."* Those are not the same functionality. Also missing: the operator-facing `--firewall-backend {iptables|nftables}` daemon flag. |
| 10 | 12.2 | `rust-netlink` is *"a GitHub **organisation** maintained by the nmstate/Red Hat networking people"*, contrasted with `neli`'s *"bus factor of one"* (`[LOW]`) | **UNSUPPORTED** | No governance document, README, or org description names Red Hat or nmstate; self-description is *"building blocks for netlink"*; **two** visible members (@cathay4t, @JohnTitor). Two members with one doing the bulk is the same bus-factor shape as `neli`. Drop the organisation argument; the two technical reasons (attribute surface, netavark consumption) carry the recommendation on their own. |
| 11 | 6.1 | The random-high-port resolver design exists for conflict avoidance with a container binding `0.0.0.0:53` (`[MED]`, self-flagged) | **UNSUPPORTED (correctly self-flagged)** | `daemon/libnetwork/resolver.go` on `master` has **no comment explaining the choice**. It defines `dnsPort = "53"`, takes the port as a parameter to `SetupFunc(port uint16)`, and calls `setupNAT()` with no rationale. The hedge and the "what would settle it" stand verbatim. |
| 12 | 2.2 | `IFLA_NET_NS_FD` inside `VETH_INFO_PEER` — *"Tradeoff: none that I found"* | **UNSUPPORTED** | A strong no-downside claim about a non-obvious kernel path, not host-checkable and not verified here. §2.6 already budgets round-trip tests against a real kernel; make this one of them before the design leans on it (and it leans hard — §9.2 item 5 claims it eliminates a whole failure class). |
| 13 | 8.1–8.4, 8.6; 1.1 sysctl table; 1.3; 1.6 empirical; 2.2 ifindex ordering; 2.5 MTU experiment; 2.6(4) grammar drift | Every `old-host` observation | **UNSUPPORTED — pending host** | `old-host.example.edu` → NXDOMAIN. Kernel-source halves verified at v6.11 (`netns_install`, `rtnl_link_get_net_capable`, `__inet_bind`, `rtnl_dev_combine_flags`) — all reproduced verbatim, so the section's citation discipline is good. Exact re-run commands are in **G2**. One `old-host` claim (§1.1 `ip_forward` inheritance) is now **independently confirmed** on the host — see H1. |

### Upgrades (claims the document under-rated)

| § | Claim | Was | Now |
|---|---|---|---|
| 1.1 | Fresh netns inherits `ip_forward` from `init_net` rather than getting the documented default 0 | mechanism `[LOW]` | **`[F]` HIGH** — `devinet_init_net()` / `net_inherit_devconf()`, governed by `net.core.devconf_inherit_init_net` (= 0 on the host). Confirmed live on a second host and a different kernel: host `ip_forward=1`, every container netns `ip_forward=1`. |
| 8.2 | Widen `ping_group_range` inside the container netns; it is per-netns and free | `[R]` from libslirp source + `old-host` | **Confirmed in Docker's production behaviour**: host `1 0`, every container netns `0 2147483647`. |
| 4.6 | `ufw deny 80` does not stop `-p 80:80` | `[F]` HIGH | Stronger: **`ufw route deny 80` also fails** (it lands in `ufw-user-forward`, still after `DOCKER-FORWARD`'s terminal ACCEPT). There is no ufw command that closes a Docker-published port. `DOCKER-USER` is the only lever. |
| 9.3 | The live leak | `[O]` HIGH, cause undetermined | Observation fully re-verified independently; six candidate innocent explanations each ruled out; and the *mechanism* identified (built-in chains are outside `removeIPChains`'s reach). |
| 5.3 | netavark's direction of travel | "deprecated and slated for removal" | **already removed** |

### Fully confirmed, spot-checked against the live host or primary source

**≈92 of ≈105 claims examined verified without correction**, including the entire §4.1 ruleset byte-for-byte, the §4.2/§4.3 hop-by-hop trace, §4.5's Docker-28 `raw/PREROUTING` hardening, §4.6's two independent ufw-bypass mechanisms, §5.1's dual-view and the `grep -ci f2b → 0` invisibility proof, §5.2's `priority filter - 1` ordering hazard, the whole of §6.1–§6.3 (identical ports, fds and pid three days later), §1.2's `ip netns list`-sees-nothing proof, §3.1/§3.4/§3.5's bridge and per-port state, §10.1/§10.2's pools and boltdb key space, §8.5's kernel citations verbatim, and the entire §12.1 async adjudication.

### The three answers the prompt asked for, in one line each

1. **`ip_forward`** — **REFUTED.** Stale kernel documentation faithfully quoted; the kernel touches `accept_redirects`, `forwarding` fan-out, and LRO, and nothing else.
2. **The live leak** — **CONFIRMED**, no innocent explanation survives, and the diagnosis is corrected in the direction that makes §9's conclusion stronger, not weaker.
3. **Async / netlink** — **CONFIRMED, all three legs. Async is not forced into the runtime core.** `rtnetlink` has no sync constructor and hard-depends on `netlink-proto` + `futures-*`; `netlink-packet-route`/`-core` have zero async deps and `netlink-sys`'s are all optional behind a default-empty feature set; netavark takes exactly that route and does not depend on `rtnetlink`. Caveat: netavark never calls `clone()`, so it witnesses the *crate* claim, not D2 §8's thread-purity claim.

### Still unsettled

- The **cause** of the §9.3 leak (needs the dev-VM kill-point experiment; add "restart dockerd and re-diff" as a step, to demonstrate the built-in-chain residue survives reconciliation).
- Every `old-host` experiment (**G2** has the commands).
- The §8.7 pasta throughput discrepancy (RootlessKit 0.24 Gbps vs passt CI 2.5 Gbps at MTU 1500) — the document flags it and it remains open.
- §2's wire-format details beyond the four kernel snippets I re-derived; in particular §2.2's `VETH_INFO_PEER` + `IFLA_NET_NS_FD` "no tradeoff" claim.
