# V5 — Adversarial verification: container networking (D4)

Verifier: **V5**. Target: `research/D4-networking.md` (2455 lines, written 2026-09-04).
Cross-file adjudication: `D5-resources-security.md` §17–§18 only.
Date of this pass: **2026-10-23** (seven weeks of upstream drift since D4).

---

## Method and evidence base

**Read:** all of D4 §0–§14; D5 §17–§18 and D5's "To D4" handoff block (lines 3213–3400, 3644–3665).
Nothing else in D5 — that is V6's.

**Ran, on `old-host`** (`OLD-HOST`, Rocky 8.8, kernel `4.18.0-425.3.1.el8.x86_64`, unprivileged uid
900000000, `iproute2-6.2.0`, `nftables v1.0.4`, `slirp4netns 1.2.3`/`libslirp 4.4.0`, no
`/etc/subuid` entry, no `pasta`, no `lxc-user-nic`). Everything under `/tmp`, everything inside
`unshare -Urn`, cleaned up. Scripts: `/tmp/v5_rootless_net.sh`, `/tmp/v5_rootless_b.sh`,
`/tmp/v5_slirp.sh`, `/tmp/v5_d.sh`, `/tmp/v5_e.sh`.

**Ran, on `host-a`** (`HOST-A`, Ubuntu 24.10, kernel `6.11.0-29-generic`, Docker
`28.3.1`, `iptables v1.8.10 (nf_tables)`, `nft v1.1.0`): **read-only only** —
`iptables -S` / `ip6tables -S` per table, `nft list tables`, `nft list table inet f2b-table`,
`ip -d link show`, `ip route`, `ip -o addr`, `ip netns list`, `lsns -t net`, `ls /run/docker/netns`,
`ss -ltnp`, `ps -eo pid,user,args`, `docker network ls` / `network inspect -f` / `info` / `version`,
and `cat` under `/proc/sys`. **No mutating command of any kind was issued on that host.**

**Web, primary sources:** nftables man page (netfilter.org), Docker nftables backend docs,
netavark release feed, Fedora change tracker, plus the sources named inline.

**Verdict counts (distinct adjudicated claims):** CONFIRMED 84 · CORRECTED 18 · DISPUTED 4 ·
WEAK-EVIDENCE 4 · UNVERIFIABLE-HERE 7.

---

## Part 1 — The D4-vs-D5 rootless networking conflict — **D4 CONFIRMED on substance; both files CORRECTED on the gate; D4's slirp4netns observation CORRECTED (not reproducible)**

This was the decisive experiment and it was run first.

### 1.1 There is no substantive conflict. Both files say the same thing; D4 says it better.

The brief anticipated a disagreement. There isn't one. Side by side:

| | D4 §8.1 (line ~1255) | D5 §17 table + "To D4" §1 |
|---|---|---|
| veth/bridge **inside** the rootless netns | works | works |
| veth end **out** to the host netns | fails, `EPERM` | fails, `EPERM` |
| bind :80 **inside** the netns | works (§8.5) | works |
| publish host port <1024 | fails | fails |

**Verdict: CONFIRMED** that the two documents agree. The adjudication that is actually needed is
not "which file is right" but "is the shared claim right, and is the gate stated correctly". It is
right; the gate is stated wrongly in both, and the wrong statement is load-bearing.

### 1.2 The full inside-the-namespace set, run on `old-host`, kernel 4.18 — **CONFIRMED**

D4 §8.1: *"The entire bridge+NAT model works rootless — bridges, veths, addresses, enslaving,
nftables nat chains, network sysctls — as long as it all stays inside the namespace."*

`[O]` **HIGH** — inside `unshare -Urn` (`CapEff: 000001ffffffffff`, `uid_map: 0 900000000 1`):

```
bridge add br0                      OK      nft add table ip mynat              OK
veth pair create (v0/v1)            OK      nat/postrouting chain (srcnat)      OK
addr add 10.77.0.1/24 br0           OK      masquerade rule                     OK
enslave v0 to br0 (IFLA_MASTER)     OK      nat/prerouting chain (dstnat)       OK
br0 / v0 / v1 up                    OK      dnat to 10.77.0.2:80 rule           OK
addr add 10.77.0.2/24 v1            OK      filter chain @ priority 0 (DROP)    OK
route add 10.88.0.0/24 via ...      OK      filter chain @ priority -300 (raw)  OK
dummy device add                    OK      named set (ipv4_addr) + elements    OK
iptables -t nat -A POSTROUTING -j MASQUERADE  OK   set-matching drop rule       OK
                                            nft -f <file>  (atomic replace)     OK
```

D4's claim is **CONFIRMED and then some**: it understated the case. The things D4 did *not* test
and which also work rootless on a 4.18 kernel are the ones §5.4's whole recommendation rests on —
**named sets, set elements, set-matching rules, base chains at arbitrary priorities including raw
(-300), and whole-table atomic replacement via `nft -f`.** That materially strengthens §5.4: the
nftables design is not a root-only design that we then have to retrofit for rootless.

`[O]` **HIGH, new** — two caveats on the sysctls that D4's blanket "network sysctls work" hides:

- `route_localnet` 0→1: **OK**. `ip_unprivileged_port_start` 1024→1: **OK**. `rp_filter`: **OK**.
  `ip_forward`: **OK** (already 1 in the fresh netns, as D4 §1.1 observed).
- **`ping_group_range` cannot be widened** in a single-uid/gid userns. See §1.6 below — this is a
  correction to an `[R]` HIGH.
- `/proc/sys/net/bridge` is **ENOENT** inside the fresh netns, confirming D4 §1.1 exactly.

### 1.3 The wall, and the sharper framing — **D4 CONFIRMED, D5 CORRECTED**

`[O]` **HIGH** — four probes on `old-host`, all `EPERM` / `RTNETLINK answers: Operation not permitted`
except where noted:

| probe | result |
|---|---|
| **B1** host netns, unprivileged: `ip link add … type dummy` / `type veth` / `ip addr add` | **EPERM** (all three) |
| **B2** inside `unshare -Urn`: `ip link set vout netns 1` (pid 1 = init netns) | **EPERM** |
| **B2** inside `unshare -Urn`: `ip link set vout netns <my own login shell pid>` | **EPERM** |
| **B3** inside `unshare -Urn`: `ip link set vout netns <child netns of the SAME userns>` | **rc=0, OK** |
| **B4** inside `unshare -Urn`: `ip link set vout netns <a SIBLING `unshare -Urn` of mine>` | **EPERM** |

**B4 is the probe that settles the framing, and neither D4 nor D5 ran it.** The sibling netns is
*not* the host netns. It is mine, it is unprivileged, it was created by my own uid seconds earlier —
and the move still fails. So:

> `[F]` **HIGH — CORRECTED.** The gate is **not** "CAP_NET_ADMIN in the host netns" (D5's wording)
> and it is not specifically about the host at all. **There is no such thing as a capability "in a
> network namespace": capabilities live in *user* namespaces.** The rule the kernel actually
> enforces is `rtnl_link_get_net_capable()` → `netlink_ns_capable(skb, net->user_ns, CAP_NET_ADMIN)`:
> **CAP_NET_ADMIN in the user namespace that *owns* the target network namespace.** The host netns
> is merely the common case of a netns owned by `init_user_ns`, where an unprivileged process has
> nothing. A sibling rootless netns owned by a *different* userns is equally unreachable — B4.

D4 §2.3 and §8.1 both state the rule correctly in the kernel-source quote but then paraphrase it as
"the initial netns" in prose; **D5 §17's table cell states it wrongly** (*"needs `CAP_NET_ADMIN` in
the host netns"*) and cites it as `[F] HIGH` **from `capabilities(7)` / `ip-link(8)`, neither of
which says this** — and marks it `not tested`.

- **D5 §17 "move one veth end to the host netns": WEAK-EVIDENCE → now CONFIRMED-as-outcome,
  CORRECTED-as-mechanism.** The outcome is right (I tested it; D5 did not). The cited basis does
  not support it and the mechanism as stated is wrong. The governing statement is D4's kernel quote.

**Why the precision matters for our design, concretely:** the wrong framing suggests a rootless
engine could attach containers to a bridge it created in *some other* namespace it owns. B4 says no.
The only netns a rootless process can push an interface into is one owned by a userns it holds
CAP_NET_ADMIN in — i.e. its own userns or a descendant. That is exactly why `rootlesskit` creates a
*single* "rootless netns" owned by the engine's own userns and puts every container's bridge inside
*that*, and why the host boundary is then crossed by a userspace stack and not by a veth.

### 1.4 The part that actually matters for our design — **CONFIRMED, with a new negative control**

D4 frames the wall as "there is no way to connect an unprivileged netns to the host's network stack
with a veth". `[O]` **HIGH, new negative control** — a rootless netns with a bridge, an address and
a route, and *no helper at all*:

```
$ unshare -Urn sh -c 'ip link set lo up; ip link add br0 type bridge;
    ip addr add 10.77.0.1/24 dev br0; ip link set br0 up; ip route add default via 10.77.0.1
    getent hosts example.com ; exec 3<>/dev/tcp/1.1.1.1/443'
dns  -> timeout (rc=124)
tcp  -> connect: No route to host  (rc=1)
```

Everything D4 said works, works — and the namespace is still a sealed box. **The bridge+NAT model
being fully constructible rootless is not the same as it being useful rootless**, and D4 §8.1's
sentence can be read as implying it is. That is the one place the D4 wording invites a wrong
conclusion, and the fix is one clause.

**CORRECTED phrasing for §8.1 (replacing the sentence at ~line 1255):**

> The entire bridge+NAT model is **constructible** rootless — bridges, veths, addresses, enslaving,
> nftables nat chains *including named sets and atomic transactions*, and most network sysctls — as
> long as every object stays inside namespaces owned by our own user namespace. It is **not
> reachable**: no veth end can enter a netns owned by a user namespace we lack `CAP_NET_ADMIN` in,
> which includes the host's. A rootless container's path to the outside therefore cannot be a veth
> at all; it must be a userspace stack (`pasta`/`slirp4netns`) or a setuid helper.

### 1.5 The helper menu, and what is present on `old-host` — **CONFIRMED**

| helper | what it buys | status on `old-host` |
|---|---|---|
| **`slirp4netns`** | usermode TCP/IP stack; tap inside the netns, ordinary sockets outside | **present, 1.2.3, and it works** (§1.7) |
| **`pasta`/`passt`** | same seam, `splice()` bypass for local traffic, no NAT, 65520 MTU | **absent** |
| **`rootlesskit`** | the netns + port-forward orchestration layer (its `builtin` port driver) | absent |
| **`newuidmap`/`newgidmap`** | multi-uid maps — **not a networking helper at all** | **binaries present, but no `/etc/subuid` entry for this user → unusable** |
| **`lxc-user-nic`** | the setuid-root escape hatch: creates the veth in `init_net` for you | **absent** |

`[F]` **HIGH, clarification worth making because the brief groups them** — `newuidmap`/`newgidmap`
are **not** part of the answer to the networking wall. They are setuid-root helpers for writing
`uid_map`/`gid_map` from `/etc/subuid` ranges; they do not touch netlink and cannot move an
interface. The only setuid helper on the list that addresses *this* wall is `lxc-user-nic`.
D4 §8.6 has this right; the brief's grouping does not. The real menu is: **a userspace stack
(`pasta`, `slirp4netns`), or a setuid-root NIC helper (`lxc-user-nic`), or nothing.**

`[R]` **CONFIRMED** — D4 §8.7's "shell out to `pasta`, do not write a usermode stack, do not build a
setuid helper" survives intact. **DISPUTED on one sub-point:** D4 §8.7 says "`pasta` … the ecosystem
has converged", but `pasta` is **absent on `old-host`**, the degradation probe, while `slirp4netns`
is present and working. On RHEL-family 8.x, `passt` is not in the base repos. **Our rootless driver
must therefore probe for both and prefer `pasta`, falling back to `slirp4netns`** — which is a
two-binary dependency, not one. That shifts §8.7's `[R]` HIGH from "shell out to `pasta`" to
"shell out to `pasta`, with `slirp4netns` as the documented fallback", and it is cheap because both
take a pid and a tap name.

### 1.6 **CORRECTED — `ping_group_range` is per-netns but it is NOT free rootless**

D4 §8.5 `[R]` **HIGH**: *"and set `ping_group_range` inside the container netns (§8.2) so `ping`
works. It is per-netns and free."* D4 §8.2 repeats it: *"we can fix this ourselves inside the
container netns — no host change needed."*

`[O]` **HIGH, new** — on `old-host`, inside `unshare -Urn` with `gid_map: 0 709200513 1`:

```
ns value:  65534  65534          host value: 0  2147483647
write "0 2147483647"  ->  EINVAL
write "0 65534"       ->  EINVAL
write "65534 65534"   ->  EINVAL
write "0 0"           ->  OK
```

**Mechanism** `[F]` **HIGH** — `net/ipv4/sysctl_net_ipv4.c::ipv4_ping_group_range()` does
`make_kgid(current_user_ns(), urange[0])` / `[1]` and returns `-EINVAL` unless **both** bounds are
gids *mappable in the caller's user namespace*. With a single-gid map, the only writable value is
`0 0`. With a full `/etc/subuid`-backed 65536-gid map it would be `0 65535`, never the documented
`0 2147483647`.

**Second, sharper correction — D4 §1.1's reading of `65534 65534` is incomplete.** D4 records the
fresh-netns value as `65534 65534` and notes "upstream default is `1 0`, i.e. *nobody*". Those are
**the same value**. `inet_init_net()` sets `make_kgid(&init_user_ns, 1)` / `make_kgid(&init_user_ns, 0)`;
read from inside a userns where neither init-gid 1 nor init-gid 0 is mapped, both render as
`overflowgid` = 65534. So `65534 65534` **is** `1 0` seen through the userns — an *empty* range, not
a one-group range. An implementation that reads `65534 65534` and concludes "a group range is
already configured, nothing to do" would be wrong, and would be wrong **silently**.

**Which `[R]` this shifts:** §8.5's "set `ping_group_range`… it is free" becomes:
> Rootful: set it, it is free. **Rootless: attempt it, accept `EINVAL`, and do not treat it as a
> setup failure.** The value must be a pair of gids mapped in our own userns; `0 0` is the only
> value guaranteed to work under a single-gid map. And never parse the current value to decide
> whether to write — `65534 65534` is the *empty* range rendered through the map, not a populated one.

`[O]` **HIGH, incidental but useful** — `ping -c1 127.0.0.1` **succeeded** inside the rootless netns
anyway, despite the empty `ping_group_range`, because `ns_capable(net->user_ns, CAP_NET_RAW)` holds
for userns-root and `ping` falls back to `SOCK_RAW`. So for *our* rootless containers the sysctl is
mostly moot; it matters only for a container that has dropped `CAP_NET_RAW`.

### 1.7 **CORRECTED — D4 §1.4 and §8.2's `slirp4netns` failure does not reproduce**

D4 §1.4 and §8.2 both carry this as `[O]` **HIGH**:

```
$ slirp4netns --configure --mtu=65520 $CPID tap0
setns(CLONE_NEWNET): Operation not permitted
$ slirp4netns --configure --mtu=65520 --userns-path=/proc/$CPID/ns/user $CPID tap0
setns(CLONE_NEWNET): Operation not permitted     # still fails
```

and D4 §8.2 builds an `[R]` **HIGH** on top of it: *"if we ever shell out to a network helper for a
rootless container, **the helper must be launched from a process already inside the container's
userns**, not from the daemon. That is a process-topology constraint → Handoff to D2."*

`[O]` **HIGH — same host, same binary, same invocations, 2026-10-23: it works.**

```
$ unshare -Urn sh -c 'ip link set lo up; sleep 10' &  ; CPID=$!
$ slirp4netns --configure --mtu=65520 $CPID tap0
sent tapfd=5 for tap0
received tapfd=5
$ nsenter -t $CPID -n -U --preserve-credentials ip -o addr show
2: tap0  inet 10.0.2.100/24 brd 10.0.2.255 scope global tap0
$ nsenter ... ip route show
default via 10.0.2.2 dev tap0
10.0.2.0/24 dev tap0 proto kernel scope link src 10.0.2.100
$ nsenter ... ping -c2 10.0.2.2          -> 2 received, 0% loss
$ nsenter ... exec 3<>/dev/tcp/1.1.1.1/443  -> TCP-CONNECT-OK
```

End-to-end, with and without `--userns-path`, from a **plain unprivileged login shell in the initial
userns** — exactly the topology D4 says cannot work.

**Why it works, and why D4's mechanism explanation is right but mis-applied** `[F]` **HIGH** —
`netns_install()` does require `CAP_SYS_ADMIN` in *both* the target netns's userns and the caller's
own; D4 §1.4's kernel quote is correct and CONFIRMED. But slirp4netns ≥1.0 **satisfies it itself**:
its `nsenter()` opens `/proc/<pid>/ns/user` *automatically* when `--userns-path` is not given, calls
`setns(usernsfd, CLONE_NEWUSER)` first — which grants `CAP_FULL_SET` in that userns via
`set_cred_user_ns()` — and only then `setns(netnsfd, CLONE_NEWNET)`. Both checks then pass. D4 §8.2
item 1 **documents this exact sequence from `main.c`** and then, two pages later, asserts the tool
fails for want of it. The document contradicts itself, and the empirical half is the wrong half.

Most likely cause of the original observation: `$CPID` captured from `$!` after
`unshare -Urn sh -c '…' &` under a shell that `exec`s the final `sleep`, leaving the recorded pid
transiently wrong, or the probe racing the child's `unshare` — both produce a `setns` on the
*initial* netns's own user namespace, which is precisely `EPERM`. I could not reproduce it under any
of three timings.

**Verdicts:**
- D4 §1.4 `[F]` **HIGH** on `netns_install()`'s two `ns_capable` checks: **CONFIRMED**, kernel source quote is accurate.
- D4 §1.4 `[R]` **HIGH** "always `setns(userns)` first, then `setns(netns)`": **CONFIRMED** — still exactly right, and it is *why* slirp4netns works.
- D4 §1.4 / §8.2 `[O]` **HIGH** "slirp4netns fails on `old-host`": **CORRECTED — not reproducible. It succeeds, end to end.**
- D4 §8.2 `[R]` **HIGH** "the helper must be launched from a process already inside the container's userns": **CORRECTED to `[R]` LOW / delete.** Its sole evidential basis was the failure above. A helper that opens the target's `/proc/<pid>/ns/user` and `setns`es it first can be launched from anywhere — which is what the reference implementation does. **This removes a claimed process-topology constraint on D2.** D2 should not restructure its daemon around it.

### 1.8 Which document was right

**D4.** On every substantive claim about what rootless networking can and cannot do, D4 is correct
and better-sourced than D5, which marks its one networking row `not tested` and cites two man pages
that do not contain the rule. D4's self-inflicted damage is in two places only: the `slirp4netns`
observation (§1.7), which is wrong and carries a recommendation with it, and the "free sysctl"
claim (§1.6).

---

## Part 2 — §4, the `docker run -p 8080:80 nginx` trace — **CONFIRMED verbatim against live production, with four additions and one structural gap**

Re-collected read-only on `host-a` on 2026-10-23, seven weeks after D4. The containers have
since restarted (netns bind mounts `d3ac0efae40f` / `48ca02d7270a` replace D4's
`27f65303b172` / `a3a7dfcf557d`; nginx is now pid 3382686, container id `919e37255290`).

### 2.1 Chain structure and jump order — **CONFIRMED, byte-identical**

`[O]` **HIGH** — `iptables -t nat -S`, `-t raw -S`, `-t filter -S` today reproduce D4 §4.1
**exactly**, rule for rule, including rule order, with one cosmetic difference: `raw/PREROUTING`
lists the five live `172.18.x` rules in a different order than D4 recorded (`.2 .5 .6 .4 .3` vs
`.2 .3 .4 .5 .6`), which is just insertion order after restarts. Also `-P INPUT DROP` is in effect
and D4's §4.1 excerpt omitted it — immaterial to the trace (published-port traffic is FORWARDed,
never INPUT), but worth noting since it is the thing that makes §4.6's "ufw is the security
boundary" belief *partly* true for host services and entirely false for containers.

CONFIRMED element by element:

| element | verdict | live evidence |
|---|---|---|
| `FORWARD → DOCKER-USER` first, `DOCKER-FORWARD` second, ufw chains third | **CONFIRMED** | `-A FORWARD -j DOCKER-USER` / `-j DOCKER-FORWARD` / `-j ufw-before-logging-forward` in that order |
| `DOCKER-FORWARD → DOCKER-CT → DOCKER-ISOLATION-STAGE-1 → DOCKER-BRIDGE → per-bridge ACCEPT` | **CONFIRMED** | exact |
| `DOCKER-ISOLATION-STAGE-1` matches `-i <br> ! -o <br>`, `STAGE-2` DROPs `-o <other br>` | **CONFIRMED** | exact |
| `nat/PREROUTING -m addrtype --dst-type LOCAL -j DOCKER` | **CONFIRMED** | exact; nft view `fib daddr type local` |
| `-A DOCKER ! -i <bridge> -p tcp --dport 80 -j DNAT`, **no `-d`** | **CONFIRMED** | exact. The publish really is "every host address" |
| `-A DOCKER -i <bridge> -j RETURN` before the DNAT rules | **CONFIRMED** | exact, and see §2.4 — it does more than D4 says |
| `POSTROUTING -s 172.18.0.0/16 ! -o br-… -j MASQUERADE` | **CONFIRMED** | exact |
| `raw/PREROUTING -d <container ip> ! -i <its bridge> -j DROP`, one per container IP | **CONFIRMED** | 7 rules, 5 live + 2 orphaned |
| `net.ipv4.ip_forward = 1` | **CONFIRMED** | `1`; `conf/all/forwarding = 1`; `net.ipv6.conf.all.forwarding = 0` |
| `DOCKER-USER` present and empty | **CONFIRMED** | still empty |

### 2.2 **New, live:** the counters, and what they calibrate

`[O]` **HIGH** — 32 days of uptime on this rule set (`docker-proxy` elapsed `32-09:30:19`):

```
nat/DOCKER      DNAT :80   33,695 pkts / 1,811 KB        ("packets" here ≈ connections, §4.2)
nat/DOCKER      DNAT :443  1,738,000 pkts / 100 MB
filter/DOCKER   ACCEPT :80   33,695 / 1,811 KB           <-- identical to the DNAT counter
filter/DOCKER   ACCEPT :443  1,738,000 / 100 MB          <-- identical
nat/OUTPUT      -> DOCKER       36 pkts / 2,160 B
raw/PREROUTING  all 7 DROP rules    0 pkts / 0 B
docker-proxy    4 processes, 00:00:04 CPU each over 32 days
```

Three things fall out that D4 could not have stated:

1. **`nat/DOCKER` and `filter/DOCKER` counters match exactly**, per port. That is a clean empirical
   proof of D4 §4.2's claim that the DNAT rule is *translation* and the filter rule is
   *authorization* for the same first-packet-of-flow, and that `DOCKER-CT` absorbs everything after.
2. **All seven `raw/PREROUTING` anti-spoof rules have fired zero times in 32 days.** D4 §4.5's
   `[R]` HIGH ("implement this") is **CONFIRMED and re-weighted**: it is zero-cost insurance against
   a precondition (someone routes to your container subnet) that is rare but catastrophic. It is not
   a hot path and should not be optimised as one — which weakens, slightly, §5.4's "the rule set
   grows linearly" urgency argument for named sets. The named-set argument survives on *update*
   cost (one insert vs one rule per container start/stop) and on atomicity, not on evaluation cost.
3. **`docker-proxy` is not idle — it is at 4 s CPU over 32 days.** D4 recorded "0:01 over 11 days".
   This is the IPv6 path doing real work (§2.5), not noise.

### 2.3 **The structural gap: D4 §4 never traces the host-originated path.** `nat/OUTPUT` is listed in §4.1 and explained nowhere.

`[O]` **HIGH, and it is the rule that answers "why does `curl <my-ip>:8080` work from the host"**:

```
-A OUTPUT ! -d 127.0.0.0/8 -m addrtype --dst-type LOCAL -j DOCKER      [36 pkts]
```

`[F]` **HIGH** — Locally-generated packets never traverse `PREROUTING`; they go
`OUTPUT (raw) → conntrack → OUTPUT (nat) → routing → POSTROUTING`. So without this rule, a published
port would be reachable from every other machine on earth and **not from the host itself** — one of
the more confusing failure modes a container engine can ship. §4.2's hop-by-hop trace covers only
the inbound external SYN; the host-local case is a genuinely different path and D4 does not walk it.

`[F]` **HIGH** — **The `! -d 127.0.0.0/8` exclusion is the load-bearing part and D4 never mentions
it.** It means:

- `curl 198.51.100.10:80` from the host → `dst-type LOCAL`, not 127/8 → **jumps to DOCKER → DNAT →
  reaches the container**. This is the 36 packets.
- `curl 127.0.0.1:80` from the host → **excluded, no DNAT at all**. It is delivered locally to
  whatever is bound to `0.0.0.0:80`, which is `docker-proxy`. **So on a default Docker host,
  `curl localhost:<published>` is served by the userland proxy and nothing else.**

That exclusion exists because DNAT-ing a `127.0.0.1` destination to a routable address is exactly
the martian case `route_localnet` exists to permit — D4 §4.4 has the `route_localnet` discussion
right but never connects it to the `! -d 127.0.0.0/8` match that is its visible consequence.
`[O]` **HIGH** — `route_localnet = 0` on all 12 interfaces, re-verified today. **CONFIRMED.**

`[R]` **CORRECTED/EXTENDED for us** — §4.4's recommendation ("DNAT only for the 95% case, helper for
loopback and v6") is right, but it must be stated with the OUTPUT rule attached, or we will ship a
host that cannot reach its own containers:

> Three nat rules per published port, not two: `PREROUTING → ours` (external), `OUTPUT → ours` with
> `! -d 127.0.0.0/8` (host-originated via a routable local address), and the DNAT itself. The
> loopback case is *deliberately excluded* from DNAT and is covered by the helper's `bind()`.

### 2.4 `-A DOCKER -i <bridge> -j RETURN` does two jobs, and D4 names only one — **CORRECTED**

D4 §4.2: *"This early-RETURN per bridge is how Docker prevents traffic originating in a container
from being DNAT'd by another container's publish rule."* True, and incomplete.

`[F]` **HIGH** — The same rule also makes **the hairpin case structurally unreachable by DNAT**. A
container connecting to the host's address on a published port sends a packet that arrives
`-i br-9d68a0cc943b`, hits the RETURN, and is **never DNAT'd**. So Docker's same-host
container-to-published-port case is blocked *before* the L2 hairpin restriction is ever consulted.
D4 §3.5 attributes the blockage to the bridge's 802.1D port-isolation rule; on a stock Docker host
that rule never gets a chance to apply.

The packet is instead delivered locally (destination is a LOCAL address, nothing translated it) to
whatever holds `0.0.0.0:<port>` — **`docker-proxy`**. That is the real mechanism of D4 §4.4's
reason #4, and it is a *different* mechanism from the one §3.5 describes.

### 2.5 `docker-proxy`: all four roles re-adjudicated

| D4's claim | verdict | evidence |
|---|---|---|
| Still spawned **by default** in Docker 28 | **CONFIRMED** | no `/etc/docker/daemon.json` on this host; `dockerd` cmdline is `-H unix://… --containerd=…` and nothing else; 4 proxies running ⇒ `userland-proxy` default is still `true` in 28.3.1 |
| `-use-listen-fd`: daemon binds, passes the fd | **CONFIRMED** | verbatim in all four cmdlines |
| Role 1 — **v6→v4 address-family bridging**, structurally impossible with NAT | **CONFIRMED, and it is the only role carrying real traffic** | `ip6tables -t nat -S` DOCKER chain is still **empty**; `net.ipv6.conf.all.forwarding = 0`; network has `EnableIPv6: false`; yet `[::]:80` and `[::]:443` are listening. 4 s CPU over 32 days is this. |
| Role 2 — holds the port / makes it visible to `ss` | **CONFIRMED** | `ss -ltnp` shows only docker-proxy on :80/:443 |
| Role 3 — loopback publishing without `route_localnet` | **CONFIRMED and sharpened** | §2.3: the `! -d 127.0.0.0/8` exclusion means loopback is *always* the proxy's job when the proxy exists |
| Role 4 — hairpin | **CONFIRMED, mechanism CORRECTED** | §2.4: the blocker is the `-i <bridge> RETURN` rule, not L2 hairpin |
| "a fallback for when DNAT cannot apply" | **CONFIRMED as the right one-line summary** | of the four roles, three are "DNAT cannot apply here" and one is bookkeeping |

**What breaks if we simply never write one** — the honest answer, now fully grounded:

1. **IPv6 clients cannot reach IPv4-only containers.** On this production host that is *the whole
   IPv6 service*. Not a corner case.
2. **`-p 127.0.0.1:8080:80` does not work**, because the `OUTPUT` DNAT rule excludes 127/8. Our
   choices are a helper `bind()` or `route_localnet=1` — and §4.4 is right that `route_localnet` is
   a security downgrade and a persistent host mutation. **Helper.**
3. **A container cannot reach its own published port via the host address** (§2.4). Rare; D4 §3.5 is
   right to defer it.
4. **`ss -ltnp` shows nothing** for published ports, and a conflicting `bind()` by an unrelated
   process **succeeds and silently shadows nothing** — the port is still DNAT'd away before the
   local listener sees it, so the admin gets a listener that never receives a packet. This is the
   quietest of the four failures and the one most worth a warning.

### 2.6 `--iptables=false` — **D4 does not cover this at all; filling the gap**

`[C]` **HIGH** — Docker's own docs: setting `iptables`/`ip6tables` to `false` *"will prevent Docker
from creating most of its `iptables` or `nftables` rules"*, it *"is likely to break container
networking"*, *"containers in bridge networks will not be able to access internet hosts by
masquerading"*, and — the part that matters — **"all of their ports will be accessible to hosts on
the local network."**
Source: <https://docs.docker.com/engine/network/packet-filtering-firewalls/>

So what it *leaves behind* is: the bridge, the bridge address and its on-link route, the veths, the
container addresses and routes, `net.ipv4.ip_forward=1`, and `br_netfilter` if the packaging loaded
it. What it removes is every rule — **including the `raw/PREROUTING` anti-spoof set**, which is why
"all of their ports will be accessible". `--iptables=false` is not "no firewalling", it is
**"pre-28.0 exposure, deliberately"**.

`[R]` **HIGH, new** — If we offer an equivalent flag, it must not be spelled as a negation of one
subsystem. The thing a user who reaches for it actually wants is *"do not touch my host firewall"*,
and the honest implementation of that is **also refuse to enable `ip_forward` and refuse to bring up
the bridge's routed path** — i.e. degrade to a strictly L2/isolated network — rather than silently
building a routed, forwarding, unfiltered path to the containers. Docker's flag builds the road and
removes the guardrails.

### 2.7 The sysctls Docker changes on your host without telling you — **CONFIRMED + one D4 missed**

`[C]` **HIGH** — Docker's docs, verbatim: it *"enables the `sysctl` settings `net.ipv4.ip_forward`
and `net.ipv6.conf.all.forwarding`"* if not already enabled, and *"it also configures the firewall
to drop forwarded packets unless they are explicitly accepted"*, opt-out via `ip-forward-no-drop`.
D4 §3.3's `[R]` MED is **CONFIRMED** against the primary source.

`[O]` **HIGH, and D4 misses it** — the second host mutation, found today:

```
$ lsmod | grep br_netfilter
br_netfilter           32768  0           <-- refcount 0, nothing is using it
$ cat /sys/module/br_netfilter/refcnt      -> 0
$ cat /etc/modules-load.d/snap.docker.conf
arp_tables
br_netfilter
ip6table_filter
iptable_filter
overlay
$ cat /proc/sys/net/bridge/bridge-nf-call-iptables   -> 1   (global)
$ ip -d link show br-9d68a0cc943b | grep -o 'nf_call_iptables [0-9]' -> nf_call_iptables 0 (per-bridge)
```

**A Docker *packaging* artifact — a `modules-load.d` drop-in — loads `br_netfilter` at every boot.**
Loading the module sets `bridge-nf-call-iptables=1` globally, which changes packet processing for
**every bridge on the host**, libvirt's and the admin's included. Docker then sets the *per-bridge*
flag to `0` on its own bridges, i.e. it opts its own bridges out of the global change it caused.
Nothing currently holds a reference to the module (`refcnt 0`), and the drop-in is named for a
package (`snap.docker`) that is not what is running (`/usr/bin/dockerd`) — so this is itself a piece
of leaked host state, left by a package install, outliving the package.

**Verdicts:**
- D4 §3.4 `[F]` HIGH on what `bridge-nf-call-iptables` does, and on the two-knob global/per-bridge split: **CONFIRMED**.
- D4 §3.4 `[R]` HIGH "do not load or enable `br_netfilter`": **CONFIRMED**, and now better-supported than D4 knew — because the cost is demonstrably *permanent and package-level*, not a runtime toggle.
- D4's implied premise that the global `=1` is somebody else's doing: **CORRECTED.** On a packaged Docker install it is Docker's, it happens at boot before `dockerd` even starts, and it is not reverted by uninstalling anything. If we ship a package, **we must not add a `modules-load.d` drop-in**, and §9's "what outlives us" inventory should gain a row for *packaging-installed host configuration*, which is the one class of state that outlives even an uninstall.

### 2.8 Hairpin, and the reverse path

`[O]` **HIGH** — `bridge -d link show`: all five container ports are `hairpin off`, `isolated off`,
`learning on`, `flood on`, `state forwarding`, `neigh_suppress off`, `locked off`, `mab off`.
**CONFIRMED** against D4 §3.5 and §3.4, and `isolated` is confirmed present and settable on this
kernel, which is what D4's `--icc=false` recommendation depends on.

`[O]` **HIGH** — bridge FDB and container neighbour table re-confirm D4 §3.1/§3.5 exactly: learned
(non-`permanent`) container MACs on the right veth ports, the bridge's own MAC as the container's
gateway lladdr, peers resolved by ordinary ARP. **CONFIRMED.** The reverse-path trace (§4.3) is
derivable from the rule text and the counters and I found nothing to correct in it.

`[O]` **HIGH** — `nf_conntrack_count` 212–294 / `max` 262144. §4.3's `[R]` to surface this in
diagnostics is sound; at this ratio it is a non-issue here, which is itself the point — it is cheap
to check and the failure mode when it is *not* fine is bizarre.

---

### 2.9 **CORRECTED — the hairpin case: D4 §3.5 blames the wrong mechanism. Hairpin mode is neither necessary nor sufficient; the missing piece is a MASQUERADE.**

D4 §3.5 `[F]/[C]` **MED**: *"Packet path: container → bridge → host routing → `nat/PREROUTING` DNAT
to `172.18.0.4:80` → routing decides the output interface is the same bridge → **the frame must go
back out the port it came in on → blocked without hairpin**."*

This is testable, and it is the kind of destructive experiment that belongs on `old-host`. Built a
complete bridge+NAT+publish topology inside `unshare -Urn` (br0 `10.77.0.1/24`, container netns at
`10.77.0.2/24` with a default route, `ip_forward=1`, listener on `0.0.0.0:80`, nft dstnat
`8080 → 10.77.0.2:80`, srcnat masquerade) and probed container → `10.77.0.1:8080` under five
configurations.

`[O]` **HIGH** — `old-host`, kernel 4.18.0-425.3.1.el8, rootless:

| # | configuration | result |
|---|---|---|
| sanity | container → its own `10.77.0.2:80` directly | `OK peer=10.77.0.2` |
| **A** | DNAT on all input interfaces, **hairpin OFF** | **timeout** |
| **B** | DNAT on all input interfaces, **hairpin ON** | **timeout — still fails** |
| **C** | B **+ bridge-to-itself MASQUERADE** (`saddr 10.77/24 daddr 10.77/24 oif br0 masquerade`) | **`OK peer=10.77.0.1`** |
| **D** | Docker-shaped: DNAT only for `iifname != br0` (≡ `-A DOCKER -i <bridge> -j RETURN`), hairpin ON, masq present | **`Connection refused`**, 0 listeners on `:8080` |
| **E** | DNAT on all interfaces + the MASQUERADE, **hairpin OFF again** | **`OK peer=10.77.0.1`** |

**E is the falsification.** Hairpin is off and it works. **B is the other half:** hairpin is on and
it fails. So:

> `[F]` **HIGH — CORRECTED.** For the container-reaches-its-own-published-port case, bridge hairpin
> mode is **neither necessary (E) nor sufficient (B)**. The thing that is both is a
> **SNAT/MASQUERADE on the bridge-to-itself path**.

**Mechanism, and why D4's reasoning went wrong** `[F]` **HIGH** — the packet is **routed, not
switched**. `10.77.0.1` is a *local* address, so the frame is delivered up to the IP stack, DNAT'd
in `nat/PREROUTING`, and then **re-emitted by the bridge device from the local stack** via
`br_dev_xmit`. The 802.1D "never send a frame out the port it arrived on" check
(`should_deliver()`: `skb->dev != p->dev`) compares against the skb's *ingress* device, which for a
locally-generated skb is the bridge itself, not `hostv`. The check never fires, so hairpin has
nothing to disable. What actually breaks it is ordinary **hairpin NAT**: without SNAT the container
sees a SYN from `10.77.0.2` to `10.77.0.2`, replies *inside its own netns*, the reply never
traverses the root netns's conntrack, the DNAT is never reversed, and the client's socket waits
forever — exactly the observed **timeout**, not a refusal. With the MASQUERADE the container sees
`peer=10.77.0.1` and the reply is forced back through conntrack. The `peer=` value in C and E is
the direct proof.

**Caveat, stated honestly** `[O]` **MED** — this was tested with `br_netfilter` **inactive**
(`/proc/sys/net/bridge` is ENOENT in the test netns). With `br_netfilter` active *and the per-bridge
`nf_call_iptables` flag set*, bridged frames do enter the nat hooks while still in the bridge
forwarding path, and a DNAT there would produce a genuinely switched frame for which hairpin *is*
required. That is the configuration D4 §3.4 recommends against and that Docker disables per-bridge
(`nf_call_iptables 0`, `[O]` confirmed on `host-a`), so it is not our configuration. **This is
the one sub-case I could not test: it needs `br_netfilter` loadable in the test netns, which needs
real root.** See "Still unverifiable".

**D is the Docker case, confirmed empirically.** With the `-i <bridge> → RETURN` semantics in place,
no DNAT occurs, the packet is delivered locally to `:8080` where nothing listens, and the connection
is **refused** — the exact signature a stock Docker host would show if `docker-proxy` were not
holding that port. This is §2.4's claim, demonstrated.

**Verdicts:**
- D4 §3.5 `[F]/[C]` MED on the hairpin packet path: **CORRECTED.** The blocker is missing SNAT, not the L2 port-isolation rule.
- D4 §3.5 `[R]` MED "skip hairpin in the MVP": **CONFIRMED as a decision, CORRECTED in its reasoning.** Skip it because it is *the wrong knob*, not because the case is rare. If we later want the case to work it is **one MASQUERADE rule in our existing nft transaction**, not an `IFLA_BRPORT_HAIRPIN_MODE` attribute — strictly cheaper than D4 estimated, and it drops into the per-attachment rule set we already build.
- D4 §3.5's description of Docker's `userland-proxy=false` behaviour ("hairpin **plus** a MASQUERADE rule"): **CONFIRMED as a description of what Docker does**, but the hairpin half is cargo in the no-`br_netfilter` configuration.

---

## Part 3 — §5, iptables vs nftables

### 3.1 **CORRECTED — §5.2's nftables verdict bullet is wrong about `accept`, and the error hides §5.4's own decisive risk**

D4 §5.2, line ~1004, `[F]` **HIGH**:

> *"Worse, a terminal verdict in a lower-priority chain (`accept` in nftables means 'accept for this
> table/hook', `drop` is immediate and final) can end evaluation before the higher-priority table is
> consulted at all."*

Checked against the nftables man page, **OVERALL EVALUATION OF THE RULESET**
(<https://www.netfilter.org/projects/nftables/manpage.html>), verbatim:

> *"An **accept** verdict (including an implicit one via the base chain's policy) **ends the
> evaluation of the current base chain**. The packet advances to the next base chain. … Thus a
> packet is ultimately accepted **if and only if no (matching) rule or base chain policy issues a
> drop verdict**."*
>
> *"A **drop** verdict (including an implicit one via the base chain's policy) **immediately ends
> the evaluation of the whole ruleset**. No further chains of any hook are consulted."*
>
> *"A **return** verdict ends the evaluation of the current chain"* and pops the call stack; from a
> base chain it applies the base chain's policy.
>
> *"The ordering is ascending: for each hook, chains with lower priority values are evaluated before
> those with higher ones"*, **"regardless of the table to which they belong"**, and
> **"the ordering of chains with identical priority is undefined."** `nat`-type chains must have
> priority above `-200`.

Three distinct errors in D4's one sentence:

1. **Granularity.** `accept` ends the current **base chain**, not "this table/hook". A table may
   hold several base chains at one hook; `accept` in one does not skip the siblings.
2. **The claim is true of `drop` and false of `accept`**, and the sentence bundles them so that it
   reads as "a terminal verdict, including accept, can preempt". It cannot. Only `drop` preempts.
3. **Direction of "lower-priority".** D4 means lower *number* = evaluated earlier; the sentence can
   be read the other way. The man page's phrasing ("ascending") is the one to use.

**CORRECTED statement:**

> Across tables at one hook, **priority decides order (ascending, lower number first, regardless of
> table; ties are undefined)**. `accept` is **not** terminal for the ruleset — it ends only the
> current base chain, and every other base chain at that hook and every later hook still runs.
> `drop` **is** terminal and immediate: no further chain of any hook is consulted. Therefore a
> packet is accepted **iff nothing anywhere drops it**. A native-nft program can preempt
> iptables-nft's chains by choosing a lower priority number — but preemption only buys it the power
> to **drop**, never the power to **accept over** a later dropper.

### 3.2 What this means for coexisting with Docker on one host — **and it inverts the practical risk**

`[O]` **HIGH** — the live ordering problem on `host-a` is confirmed exactly as D4 describes it:
`table inet f2b-table` (fail2ban, native nft) sits at `type filter hook input priority filter - 1`
= **-1**, one below the `ip filter` INPUT chain that `iptables-nft` pins at 0. Three firewall
writers, two invisible to each other's tooling. **CONFIRMED.**

But the consequence D4 draws in §5.2 ("a terminal verdict … can end evaluation before the
higher-priority table is consulted") points at the *wrong hazard*. The real one is the one §5.4
states correctly in its tradeoff list and §5.2 obscures:

`[F]` **HIGH, decisive** — `iptables -t filter -P FORWARD DROP` is rendered by `iptables-nft` as a
base-chain **policy drop** on the `ip filter` FORWARD chain at priority 0. `[O]` HIGH — that is the
state on `host-a` right now (`-P FORWARD DROP`, set by ufw *and* by Docker). Per the man page,
a base-chain policy drop is terminal for the whole ruleset. So:

> **If we write our rules in `table inet mydocker` and `accept` forwarded container traffic there,
> and the host has a foreign `filter/FORWARD` policy of DROP with no matching accept, our traffic is
> dropped anyway — whatever priority we choose.** Priority buys ordering, not authority. There is no
> priority low enough to out-accept a later drop, because `accept` does not end the ruleset.

This is not hypothetical: it is the configuration of the production host in front of us, and it is
the configuration of every Ubuntu host that has ever run `ufw enable` or installed Docker.

**Verdicts:**
- D4 §5.2 `[F]` HIGH on base-chain priorities and the aliases (`raw=-300, mangle=-150, dstnat=-100, filter=0, security=50, srcnat=100`): **CONFIRMED verbatim** against the man page. Two additions D4 omits and we need: **ties are undefined**, and **`nat` chains must be > -200** — the second is a hard constraint on where we may put our dstnat chain, and it is below `raw` (-300), so our anti-spoof chain must be `type filter`, not `type nat`. (`[O]`: verified on `old-host` — `type filter hook prerouting priority -300` accepted; this is what our §4.5 equivalent must be.)
- D4 §5.2's terminal-verdict bullet: **CORRECTED** as above.
- D4 §5.4's tradeoff bullet ("our `accept` in *our* table does not override it — both hooks run, and a DROP anywhere wins… this is the strongest argument against native nft"): **CONFIRMED, and it should be promoted out of the tradeoff list into the design.** §5.2 and §5.4 contradict each other on this point in the same document; §5.4 is the correct half.

### 3.3 §5.4's `[R]` HIGH "write native nftables in our own table and never touch iptables" — **DISPUTED on the absolute form; the reasoning otherwise survives and has strengthened**

Auditing the six stated reasons against today's evidence:

| § 5.4 reason | verdict |
|---|---|
| 1. **Atomicity** | **CONFIRMED, and it is the strongest one.** `[O]` verified on `old-host`: `nft -f <file>` applies a whole-table replacement as one transaction, **including rootless inside `unshare -Urn` on kernel 4.18**. §11.4's security case — "filter ACCEPT installed, `raw` anti-spoof missing ⇒ the pre-28 hole is open" — is real and atomicity is the only clean answer. |
| 2. **Named sets and maps** | **CONFIRMED mechanically, re-weighted in importance.** `[O]` on `old-host`: `nft add set … type ipv4_addr`, `add element`, and `ip daddr @set … drop` all work, rootless, on 4.18. But §2.2 above: the per-container-IP `raw` rules have fired **0 times in 32 days**, so the gain is **not** evaluation cost. It is *update* cost (one set element per container start/stop vs one rule insert/delete into a shared chain) and rule-set legibility. Still a good argument; a weaker one than §5.4 implies. |
| 3. **Priority control** | **CONFIRMED but materially narrowed by §3.2.** Priority lets us run *before* Docker/ufw. It does not let us accept past them. Its real value is being able to place our anti-spoof `drop` at -300 and our filter chain wherever we choose — defensive placement, not offensive. |
| 4. **One namespace of our own / `nft delete table` as a total reset** | **CONFIRMED.** `[O]` on `old-host`: `nft delete table inet mydocker` → rc=0, complete. **With one trap D4 does not flag: `flush table` is NOT `delete table`.** Verified — after `flush table inet mydocker`, the named set and **its elements survived** (`elements = { 10.77.0.2, 10.77.0.3 }`), only the rules were cleared. §9.7's reconcile step 4 says "delete the table and re-add" — that is the correct verb and the document should say *why*, because `flush` is the one a reader would reach for and it silently leaks set elements. |
| 5. **`inet` family covers v4 + v6 in one table** | **CONFIRMED**, and §2.5 shows the gap it closes is real on this very host (empty `ip6` DOCKER chain, all IPv6 traffic on the userland proxy). |
| 6. **Direction of travel** | **CONFIRMED and upgraded from `[C] MED–HIGH / [LOW] on timeline` to `[C] HIGH`.** See §3.4. |

**The dispute.** "**Never touch iptables at all**" is not operationally achievable on the hosts we
will actually run on. §3.2 shows that on any host with a foreign `filter/FORWARD` policy DROP — ufw,
firewalld, or a previous Docker install — a pure-native-nft engine's forwarded traffic is dropped
regardless of what we write. The three available responses are:

1. **Detect and refuse/warn.** Read `nft list ruleset`, find a base chain at hook `forward` whose
   policy is `drop` and that is not ours, and refuse to create a routed network (or publish a port)
   without `--i-know-my-firewall-drops-forwarded-traffic`. Costs nothing, surfaces the problem at
   the right moment. **This is D4 §4.6's warning, promoted from courtesy to correctness.**
2. **Write exactly one `accept` into the foreign `ip filter` FORWARD chain** — i.e. touch iptables,
   once, for one rule, and own the fact.
3. **Set `filter/FORWARD` policy to ACCEPT ourselves** and rely on our own drop. This is a global
   host mutation of exactly the kind §9 says never to make silently, and it is strictly worse than 2.

**Recorded as DISPUTED with my weighting:** the facts under §5.4 survive; the recommendation is
mis-stated in its absolute form. The honest version:

> **Write every rule of ours natively, in `table inet mydocker`, and never express our policy in
> iptables syntax. Accept that on a host whose `filter/FORWARD` is foreign-dropped we have exactly
> two honest options — one `accept` rule in the foreign chain, or refusing to publish — and make
> that an explicit, logged, user-visible decision rather than a silent failure.** Shipping the
> absolute form means shipping an engine whose containers have no network on a stock `ufw`-enabled
> Ubuntu host, with no error anywhere.

### 3.4 Two `[C]` claims have moved, both in §5.4's favour — and one of them was already wrong when D4 was written

**netavark — `[C]` MED–HIGH with `[LOW]` on timeline → CONFIRMED, `[C]` HIGH.**
D4: *"nftables is now the default; the iptables driver is deprecated and slated for removal"*, with
`[LOW]` on the timeline because the claim came from a GitHub discussion.
`[C]` **HIGH** — **netavark v2.0.0 (released 10 Jun 2026) release notes: "Removed iptables
support."** It is a listed breaking change. Latest is **v2.1.0 (06 Aug 2026)**, which also made
*"the bridge network driver … default to strict isolation mode"*.
Sources: <https://github.com/containers/netavark/releases/tag/v2.0.0>, <https://github.com/containers/netavark/releases>
**The removal has happened.** D4's `[LOW]` can be retired. Podman's network backend is now
nftables-only, which is about as strong an endorsement of §5.4's direction as the ecosystem can give.

**Docker — `[C]` HIGH. CORRECTED — and this is a D4 *miss*, not drift.**
D4 §5.3: *"Docker 28 introduced a firewall backend abstraction… the abstraction exists and iptables
is still what ships/defaults here."*
`[C]` **HIGH** — **Docker Engine 29.0.0 shipped an actual nftables backend on 2025-11-10**, i.e.
**ten months before D4 was written**:
*"Experimental support for nftables can now be enabled by setting Docker daemon's `firewall-backend`
option to `nftables`."* Latest is **29.9.2 (2026-10-20)**. iptables remains the default; nftables is
documented as experimental, *"cannot be enabled when the Docker daemon is running in Swarm mode"*,
and *"the rules for overlay networks have not yet been migrated from iptables"*.
Sources: <https://docs.docker.com/engine/release-notes/29/>, <https://docs.docker.com/engine/network/firewall-nftables/>

`[O]` **HIGH** — `docker info` on `host-a` reports `{"Driver":"iptables"}`. **CONFIRMED as an
observation, but it is an observation about a host pinned at 28.3.1 — a major version behind even in
September 2026.** It is not evidence about what Docker does; it is evidence about what this host
does. **This is the methodological failure worth naming: D4 inferred the state of upstream from a
single un-upgraded production host and did not check the release feed.** The contract's "seven weeks
of drift is enough to move a `[C]` claim" understates it — this `[C]` HIGH was ten months stale on
the day it was written, and the same pattern will recur wherever `host-a` is the only source.

**Three details of Docker's nftables implementation that bear directly on our design:**

1. *"For bridge networks, Docker creates two tables, `ip docker-bridges`"* and `ip6 docker-bridges`,
   with base chains plus per-network chains, using *"well known priority values for each of its base
   chains"*. **Docker chose per-family tables; D4 §5.4 reason 5 argues for a single `inet` table.
   Ours is the better call** — the v4/v6 asymmetry on this very host (§2.5) is the bug `inet`
   prevents — but we should know we are diverging from the reference implementation, and that
   `inet` chains cannot use family-specific statements without an explicit `ip`/`ip6` qualifier.
2. *"In Docker's nftables implementation, **there is no `DOCKER-USER` chain**."* Instead the docs
   tell the admin to *"create your own table with base chains of type `filter` and hook `forward`,
   and set priorities relative to Docker's chains."* **That is exactly D4 §5.4's architecture,
   adopted by Docker independently.** It also means D4 §4.6's `[R]` HIGH ("create a `MYDOCKER-USER`
   equivalent") is **CORRECTED for the nftables world**: there is nothing to create. The admin's
   override mechanism *is* a table at a lower priority, and our job is to **document our priorities
   as a stable interface** — because once an admin writes a chain at "our priority minus one", those
   numbers are a public API we cannot change.
3. *"nftables: Docker will **not** enable IP forwarding on the host."* A deliberate behavioural
   divergence from the iptables backend (which does, §2.7). Worth matching: it is the
   least-surprising default and it removes §11.4's "largest irreversible global mutation" from the
   stage-4 risk list — at the cost of requiring the admin to enable forwarding themselves. `[R]`
   **MED** — follow Docker's nftables behaviour here, not its iptables behaviour: **detect
   `ip_forward=0`, fail the network create with a one-line explanation and the exact `sysctl`
   command, and do not write it ourselves.** That converts §1.1's "booby trap: writing `ip_forward`
   resets other config" from a hazard we must navigate into one we simply never trigger.

### 3.5 §5.1 — the dual-view claims — **CONFIRMED**

`[O]` **HIGH** — re-verified today: `nft list tables` →
`ip filter, ip6 filter, ip nat, ip6 nat, ip raw, inet f2b-table`. `iptables -t filter -S` shows
Docker's and ufw's chains and **does not mention `f2b-table`**, although fail2ban's rule is live and
currently rejecting one address (`elements = { 198.51.100.30 }`). D4 §5.1's three consequences are
all CONFIRMED, including the one that matters most: **an admin auditing with `iptables-save` would
conclude fail2ban is not running.** The `# Warning: table ip nat is managed by iptables-nft, do not
touch!` marker is present. The `counter` on every translated rule is present. Nothing to correct.

---

## Part 4 — §1 and §2, the primitives we will actually implement — **CONFIRMED, with one caution about the primary source**

Every claim below was re-tested on `old-host` inside `unshare -Urn` / `unshare -Urmn`.

### 4.1 Netns lifecycle — **CONFIRMED**

| claim | verdict | evidence |
|---|---|---|
| `unshare(CLONE_NEWNET)` moves the calling **thread**; `clone(CLONE_NEWNET)` starts the child in a new one | **CONFIRMED** | `unshare(2)`, `clone(2)` |
| A fresh netns contains exactly `lo`, DOWN, `qdisc noop`, **with no `127.0.0.1`** | **CONFIRMED** | reproduced: `1: lo: <LOOPBACK> … state DOWN` and no addresses until `ip link set lo up` |
| `/proc/<pid>/ns/net` is a magic symlink to `net:[<inode>]` on **nsfs**; same inode ⇒ same netns | **CONFIRMED** | `namespaces(7)`; `readlink` on both hosts |
| `ip netns add` is `mkdir` + self-bind + `--make-shared` + `O_CREAT\|O_EXCL` placeholder + `unshare` + `mount --bind /proc/self/ns/net` | **CONFIRMED behaviourally** | `grep nsfs /proc/self/mountinfo` → `3531 3530 0:4 net:[4026534952] /run/netns/p1 rw shared:1226 - nsfs nsfs rw` — an ordinary bind mount, source `net:[…]`, fstype `nsfs`, **and `shared:1226`, i.e. the `--make-shared` really is there** |
| **The mounted file's inode number *is* the namespace id** | **CONFIRMED exactly** | `stat -c '%i' /run/netns/p1` → `4026534952`, identical to the `net:[4026534952]` in mountinfo. So `fstat().st_ino` on a netns fd is the namespace identity — no string parsing. This is a genuinely useful implementation detail and D4 is right to call it out. |
| `ip netns list` is `readdir()` and nothing more; the path is a **convention** | **CONFIRMED** | `ip-netns(8)`; and live on `host-a`: `ip netns list` is **empty** while `lsns -t net` shows **five** container netns bind-mounted under `/run/docker/netns/` |
| A bind mount keeps a netns alive with **zero processes in it** | **CONFIRMED** | `ip netns add p1` then `ip netns exec p1 ip -o link show` → `lo` listed. No process ever lived in it. Deleting the mount → `grep -c nsfs /proc/self/mountinfo` → **0** |

> ⚠ **Caution about the primary source, and D4 is right to go beyond it.**
> `network_namespaces(7)` says *"When a network namespace is freed (**i.e., when the last process in
> the namespace terminates**)"*. That parenthetical is **an imprecise simplification**: it is
> exactly the belief that makes people think a netns leak is impossible. P3 above disproves it — a
> netns with no process ever in it, held alive indefinitely by a bind mount. D4 §1.2's
> "a process's `nsproxy`, an open fd, or a bind mount of the nsfs file" is the correct statement and
> it is **not** what the man page says. **Cite `namespaces(7)` + the empirical demonstration for
> this, not `network_namespaces(7)`.**

### 4.2 What happens to interfaces when a netns dies — **CONFIRMED, both halves**

`[F]` **HIGH** — `network_namespaces(7)`, re-read verbatim: *"its **physical** network devices are
moved back to the **initial** network namespace"* (not the parent's), and *"When a namespace is
freed, the **`veth(4)` devices that it contains are destroyed**."* D4 §1.6 quotes both accurately.
**The asymmetry D4 states is correct: physical devices migrate home and survive; virtual devices are
destroyed.**

`[O]` **HIGH** — and the composed consequence, independently reproduced today: created
`hostend`/`childend` with the child end in a separate netns, killed the child, waited 3 s →
`hostend` count in the parent went **1 → 0**, leaving only `lo`. **The host-side veth is
garbage-collected by the kernel with no action from us.** D4 §1.6's `[R]` HIGH ("do not write
cleanup code for either veth end") is **CONFIRMED** and is still the single best cost-saving
recommendation in the file.

`[O]` **HIGH, D4 does not record the errno** — `ip link set lo netns <pid>` fails with
**`EINVAL` ("Invalid argument")**, not `EPERM`. D4 §2.3/§8.1 describe the `netns_immutable` /
`NETIF_F_NETNS_LOCAL` check and quote the ext-ack string *"The interface netns is immutable"* —
**that string did not appear on this 4.18 kernel**, which predates the `NL_SET_ERR_MSG` for it.
Practical consequence for us: **branch on `EINVAL`, not on the message**, and do not rely on ext-ack
text existing for a given failure on older kernels. Same for the name-collision case, which returned
**`EEXIST` ("File exists")** with no ext-ack text on 4.18.

### 4.3 §2 — veth over rtnetlink, and the one-message claim — **CONFIRMED, and it is the most valuable `[R]` in §2**

D4 §2.2 `[F]`/`[R]` **HIGH**: put `IFLA_NET_NS_FD` *inside* the `VETH_INFO_PEER` nest and the peer is
created directly in the target netns — "one message", no create-then-move, **no window in which the
container-side veth exists in the host netns**.

`[O]` **HIGH — verified directly.** `ip link add hostv type veth peer name ceth netns $C` is
precisely this encoding (iproute2 puts `IFLA_NET_NS_FD`/`_PID` inside the peer nest). Result:

```
rc=0
parent netns:  1: lo    2: hostv@if2          <-- ceth NEVER appeared here
child netns:   1: lo    2: ceth@if2
```

The peer was never visible in the creating namespace at any point. **CONFIRMED: the single-message
form works, it is what `ip` already does, and it eliminates both the race and the leak window D4
names.** This is the cheapest correctness win in §2 and nothing I found argues against it.

`[O]` **MED → CONFIRMED** — the nsfs/procfs **path** form of the target also works on the create
path (`ip link add h2 type veth peer name c2 netns /proc/$C/ns/net` → rc=0), which is the
`IFLA_NET_NS_FD`-from-an-open-fd case D4 recommends over the pid form. **One iproute2 asymmetry
worth knowing:** the same path form on the *set* path (`ip link set X netns /proc/1/ns/net`) is
rejected by iproute2 6.2 with `argument "..." is wrong: Invalid "netns" value` — a userspace parser
limitation, not a kernel one. Irrelevant to us (we send the netlink ourselves) but it will confuse
anyone reproducing by hand.

`[O]` **MED → CONFIRMED** — ifindex ordering: `ip link add vA type veth peer name vB` produced
`4: vB@vA` and `5: vA@vB`. **The peer gets the lower index.** Both start
`<BROADCAST,MULTICAST,M-DOWN>`. Exactly as D4 recorded. The `[R]` HIGH that follows ("bring up
*both* ends; forgetting the host end presents as `LOWERLAYERDOWN`, not as an error") is sound and
was visible throughout my topology work.

**Message construction — CONFIRMED on documentary grounds, not re-derived.** `RTM_NEWLINK` with a
zeroed `ifinfomsg`, `IFLA_IFNAME`, `IFLA_LINKINFO{IFLA_INFO_KIND="veth", IFLA_INFO_DATA{VETH_INFO_PEER{
<embedded zeroed ifinfomsg>, IFLA_IFNAME, …}}}`; attribute numbers `IFLA_LINKINFO=18`,
`IFLA_INFO_KIND=1`, `IFLA_INFO_DATA=2`, `VETH_INFO_PEER=1`, `IFLA_NET_NS_PID=19`,
`IFLA_NET_NS_FD=28`. D4's trap — *"`VETH_INFO_PEER`'s payload begins with a bare `struct ifinfomsg`,
and only then does the attribute list start"*, with `veth_policy[VETH_INFO_PEER].len =
sizeof(struct ifinfomsg)` so a shorter payload returns `-ERANGE` — matches the kernel sources D4
cites and matches iproute2's `ip/link_veth.c`. I did not re-derive the byte layout; `netlink-packet-route`
encodes it for us (§12) and the one-message result above is behavioural confirmation that the shape
is right. **Nothing here needed correcting.**

`[R]` **CONFIRMED and worth restating** — §2.1's `NETLINK_EXT_ACK` recommendation survives, with the
§4.2 caveat attached: **turn it on, but do not make control flow depend on the *text*.** On 4.18 two
of the three failures D4 names produce no ext-ack at all. Branch on the errno; log the text.

### 4.4 §9.6's stale-placeholder findings — **CONFIRMED, independently reproduced verbatim**

D4 §9.6 simulated dying between `open(O_CREAT|O_EXCL)` and `mount --bind`. Reproduced today on the
same host, identical results:

```
ip netns list            -> "Error: Peer netns reference is invalid." (x2), then lists BOTH `crashed` and `p1`
ip netns add crashed     -> rc=1   Cannot create namespace file ".../crashed": File exists
ip netns exec crashed    -> rc=255 setting the network namespace "crashed" failed: Invalid argument
ip netns delete crashed  -> rc=0   (recovers)
```

All five of D4's readings hold, including the one that matters most for our recovery path:
**`setns()` returning `EINVAL` on a path we ourselves created is a positive identification of a
stale placeholder.** `[R]` **CONFIRMED** — "umount2 then unlink, and unlink must run even if umount
failed" is the correct teardown shape; `EINVAL`/`ENOENT` from `umount2` is the *expected* result on
a placeholder and must not abort.

---

## Part 5 — §6, DNS — **CONFIRMED in full, live, including the per-container randomness**

Re-verified read-only on `host-a` today, inside the running nginx container's net **and mount**
namespaces (`nsenter -t <pid> -m -n -- cat …`; read-only `cat` only).

### 5.1 The 127.0.0.11 mechanism — **CONFIRMED, every step**

```
container /etc/resolv.conf:
  # Generated by Docker Engine.
  nameserver 127.0.0.11
  search .
  options edns0 trust-ad ndots:0
  # Based on host file: '/etc/resolv.conf' (internal resolver)
  # ExtServers: [host(127.0.0.53)]

container-netns nat table:
  -A OUTPUT      -d 127.0.0.11/32 -j DOCKER_OUTPUT
  -A POSTROUTING -d 127.0.0.11/32 -j DOCKER_POSTROUTING
  -A DOCKER_OUTPUT      -d 127.0.0.11/32 -p tcp --dport 53 -j DNAT --to-destination 127.0.0.11:42041
  -A DOCKER_OUTPUT      -d 127.0.0.11/32 -p udp --dport 53 -j DNAT --to-destination 127.0.0.11:43967
  -A DOCKER_POSTROUTING -s 127.0.0.11/32 -p tcp --sport 42041 -j SNAT --to-source :53
  -A DOCKER_POSTROUTING -s 127.0.0.11/32 -p udp --sport 43967 -j SNAT --to-source :53

listeners, inside the container netns, owned by a host-netns process:
  UNCONN 127.0.0.11:43967  users:(("dockerd",pid=70234,fd=57))
  LISTEN 127.0.0.11:42041  users:(("dockerd",pid=70234,fd=60))
```

- **Which process listens, in which namespace: CONFIRMED.** `dockerd` (pid 70234, host netns) owns
  sockets that live in the container's netns — §1.5's socket-creation trick, demonstrated.
- **DNAT rules inside the container's netns: CONFIRMED**, they exist and point at the bound ports.
- **The SNAT-back-to-`:53` half: CONFIRMED**, and D4's reason for it (glibc/musl reject a reply
  whose source tuple differs from the query's) is correct.
- **The ports are random per container and re-rolled on restart: CONFIRMED by drift.** D4 recorded
  `39251`/`55634` on 2026-09-04; today the same service shows `42041`/`43967`, and the owning fds
  moved `52/54 → 57/60`. The containers restarted in between. **So the rules genuinely must be
  written per container netns *after* the socket is bound — D4's corollary is right and now has two
  independent samples.**
- D4 §6.1's `[MED]` on *why* a random high port rather than `:53` (conflict avoidance against a
  container binding `0.0.0.0:53`) remains **WEAK-EVIDENCE**: it is the only explanation consistent
  with the design, but I also found no maintainer statement. The test that settles it is unchanged
  (the libnetwork resolver commit message).

### 5.2 `/etc/resolv.conf` and `/etc/hosts` generation — **CONFIRMED**

`[O]` **HIGH** — container `/etc/hosts`, read today:

```
127.0.0.1	localhost
::1	localhost ip6-localhost ip6-loopback
fe00::	ip6-localnet
ff00::	ip6-mcastprefix
ff02::1	ip6-allnodes
ff02::2	ip6-allrouters
172.18.0.4	919e37255290
```

Six boilerplate lines + one meaningful line, and `/etc/hostname` = `919e37255290`, matching. **D4
§6.2 CONFIRMED exactly**, including the `[R]` HIGH that the last line matters (`gethostname()` then
resolve is a common pattern). Note the container id has changed since D4 (`301b2be6f7c8` →
`919e37255290`) while the IP has not — see §6.3.

### 5.3 systemd-resolved — **CONFIRMED, and the failure mode is real on this host**

`[O]` **HIGH** — host `/etc/resolv.conf` → `/run/systemd/resolve/stub-resolv.conf`,
`nameserver 127.0.0.53`; `systemd-resolve` listening on `127.0.0.53%lo:53`. The container's generated
file says `(internal resolver)` and `ExtServers: [host(127.0.0.53)]`. **D4 §6.3 CONFIRMED verbatim**,
including the reading of the `host(...)` annotation as "forward upstream queries to 127.0.0.53,
resolved from the host network namespace".

The underlying `[F]` HIGH — *"`127.0.0.53` is meaningless inside a container netns; each netns has
its own loopback, so naively copying the host's resolv.conf yields a container with no DNS"* — is
correct and is the single most load-bearing fact in §6. **CONFIRMED.**

D4 §6.3's `[F]` **MED** on Docker's precise decision tree (all-loopback ⇒ internal resolver;
otherwise filter and copy) remains **WEAK-EVIDENCE** — inferred, not read. Unchanged; the test is
still `libnetwork/resolvconf` in moby.

### 5.4 User-defined vs default bridge — **CONFIRMED**

`[C]` **HIGH** — Docker's docs state the asymmetry (names resolve on user-defined networks, not on
the default bridge) and `[O]` the host is consistent: all five containers are on the user-defined
`webapp_default` and the compose app depends on name resolution. **CONFIRMED.** The mechanism is
`dockerd`'s embedded resolver answering from its own per-network records; the default bridge simply
has no such records. D4 §6.4's `[R]` HIGH "do not implement `--link`" is **CONFIRMED** — it is
deprecated upstream and strictly worse than the `/etc/hosts` injection it is built on.

### 5.5 What an MVP can honestly skip — **CONFIRMED, with one sharpening**

D4 §6.5's three-stage ladder survives review. Stage 1 (filter the host's `resolv.conf`, drop every
loopback nameserver, fall back to a configured default, generate `/etc/hosts`) is **~30 lines and
genuinely shippable**, and it works on systemd-resolved hosts precisely because it *drops*
`127.0.0.53` rather than understanding it. `[R]` **CONFIRMED**, including the refusal to hardcode
`8.8.8.8` silently.

**Sharpening, from §6.1's drift evidence:** D4 §6.5 stage 3 `[R]` MED suggests that when we do build
the embedded resolver we should **bind `127.0.0.11:53` directly and skip the DNAT/SNAT rules**,
accepting the risk of a container binding `0.0.0.0:53`. The §5.1 observation strengthens this: the
DNAT/SNAT pair exists *only* because the port is random, and the port is random *only* because of
the `0.0.0.0:53` conflict. Binding `:53` directly removes **four rules per container netns, the
ordering dependency between socket-bind and rule-write, and an entire class of restart bug** (the
rules must be rewritten on every start because the port changes). It converts a silent
misconfiguration into a loud `EADDRINUSE` in a rare case. **Agreed, and I would raise it from
`[R]` MED to `[R]` HIGH.**

**What to skip, stated plainly:** stage 3 entirely. An MVP needs working external DNS (stage 1) and
nothing else. Peer-name resolution via `/etc/hosts` injection (stage 2) is worth it only once
`mydocker` grows multi-container units; its known ceiling (a snapshot that goes stale on
restart-with-a-new-IP) is **largely removed by sticky IPAM** — and Part 7 below shows Docker's live sticky-IP
behaviour makes exactly that bet.

---

## Part 6 — §9, host state that outlives our process — **CONFIRMED, still live 7 weeks later, and the mechanism is sharper than D4 states**

### 6.1 The orphan is still there — **CONFIRMED, re-observed 2026-10-23**

`[O]` **HIGH** — `host-a`, read-only. Seven weeks after D4 recorded it, every element of the
orphan is unchanged:

```
docker network ls   -> 4 networks: webapp_default (9d68a0cc943b, 172.18.0.0/16),
                       bridge (f8b0597542e5, docker0, 172.17.0.0/16), host, none
                       NOTHING owns 172.19.0.0/16 or the id 0d4cf39c58f7

ip -d link show br-0d4cf39c58f7
  26: br-0d4cf39c58f7: <NO-CARRIER,BROADCAST,MULTICAST,UP> state DOWN  fdb_n_learned 0
ip -4 -o addr        -> 26: br-0d4cf39c58f7  inet 172.19.0.1/16
ip route             -> 172.19.0.0/16 dev br-0d4cf39c58f7 proto kernel scope link src 172.19.0.1 linkdown
ip -o link | grep master -> five veths, ALL on br-9d68a0cc943b, none on the orphan

iptables -t nat -S POSTROUTING
  -A POSTROUTING -s 172.19.0.0/16 ! -o br-0d4cf39c58f7 -j MASQUERADE        <-- ORPHAN, still here
iptables -t raw -S
  -A PREROUTING -d 172.19.0.2/32 ! -i br-0d4cf39c58f7 -j DROP               <-- ORPHAN
  -A PREROUTING -d 172.19.0.3/32 ! -i br-0d4cf39c58f7 -j DROP               <-- ORPHAN
iptables -t filter -S | grep -E '172\.19|0d4cf39c58f7'   -> (empty)
docker info -> LiveRestoreEnabled: false, DefaultAddressPools: null, ServerVersion 28.3.1
```

**CONFIRMED.** The bridge's ifindex is still 26 against the live bridge's 67, so it predates the
current network by a long way. Counters: all three orphan rules have fired **0 packets**.

### 6.2 **CORRECTED/SHARPENED — the mechanism D4 gives is right in outcome, imprecise in cause, and the precise version is a *better argument for §5.4*.**

D4 §9.2: *"Docker's filter chains are rebuilt from scratch at daemon start, so anything that is not
in the store disappears from them; the nat rules, the raw rules, and the bridge device are written
once at network-create time and are only removed by an explicit network-delete."*

`[O]` **HIGH, new detail D4 missed** — the `nat/DOCKER` chain contains
`-A DOCKER -i br-9d68a0cc943b -j RETURN` and `-A DOCKER -i docker0 -j RETURN`, and **no RETURN for
`br-0d4cf39c58f7`**. That rule is per-network and was written at network-create time, exactly like
the leaked MASQUERADE — yet it was cleaned. So the split is **not** filter-vs-nat and it is **not**
create-time-vs-start-time. The actual line is:

> `[F]` **HIGH — CORRECTED.** **Rules Docker writes into *its own* chains (`nat/DOCKER`,
> `filter/DOCKER`, `DOCKER-ISOLATION-STAGE-1/2`, `DOCKER-FORWARD`, `DOCKER-CT`, `DOCKER-BRIDGE`) are
> flushed and rebuilt at daemon start, so orphans there self-heal. Rules Docker writes into
> *built-in* chains it shares with other software (`nat/POSTROUTING`, `raw/PREROUTING`, and the
> jumps in `filter/FORWARD`) cannot be flushed — they can only be deleted by exact match
> reconstructed from a store record — so orphans there are permanent.** The device, the address and
> the route are in the same category: nothing can enumerate them as "ours" without the record.

This is a cleaner rule, it predicts exactly the observed state, and it carries a design consequence
D4 does not draw:

`[R]` **HIGH, new** — **This is the strongest form of §5.4 reason 4, and D4 under-sells it.** The
entire orphan class exists *because* Docker must put some of its rules in chains it does not own.
With `table inet mydocker` and nothing outside it, **100% of our firewall state is enumerable
without any record of ours surviving** — `nft list table inet mydocker` is a complete inventory, and
`nft delete table inet mydocker` is a complete, atomic reset. The permanent-orphan class
**disappears by construction**, not by better bookkeeping.

`[R]` **HIGH, and it is the sting in the tail** — §3.2 showed we may be forced to write **exactly
one** rule outside our table: an `accept` into a foreign `filter/FORWARD` whose policy is DROP.
**That one rule is therefore the only piece of our state that can become a permanent orphan.** It
must carry an unmistakable, greppable marker (an iptables `-m comment --comment "mydocker:<netid>"`,
or an nft `comment`) so that a sweep can find and remove it **with no record of ours intact** — and
our reconcile pass must look for it explicitly. One rule, one marker, one extra line in the sweep.

### 6.3 The harm the orphan can do, and the cleanup-order trap D4 does not state

`[F]` **HIGH** — D4 §10.3 is right that the orphan is currently **safe but permanent**: the bridge's
address creates an on-link route, Docker's `InferReservedNetworks` on-link-route heuristic sees it,
and `172.19.0.0/16` is therefore never re-allocated. Fine.

`[R]` **HIGH, new** — **But that safety is an artefact of the bridge still existing, and the obvious
manual cleanup destroys it.** An admin who finds this and runs the natural first command —
`ip link delete br-0d4cf39c58f7` — removes the address, removes the on-link route, and thereby makes
`172.19.0.0/16` **allocatable again**, while leaving behind:

- `-A POSTROUTING -s 172.19.0.0/16 ! -o br-0d4cf39c58f7 -j MASQUERADE`. The `! -o` names a device
  that no longer exists, so the exclusion can never match ⇒ **every packet from the new
  172.19.0.0/16, including container-to-container traffic on the same new bridge, gets
  masqueraded.** Every container sees every peer as coming from the gateway. §3.2 of D4 explains
  exactly why that exclusion is load-bearing; this is what its absence looks like.
- two `raw/PREROUTING` DROP rules that will **silently blackhole all externally-originated traffic
  to the new `.2` and `.3`** — before conntrack, before nat, with no log.

So the correct manual recovery order is **rules first, device last**, and the same ordering applies
to our own teardown. **This belongs in §9.7's ordering block**, which currently gets the netns part
right (unmount first, because it garbage-collects the veths) but says nothing about the
bridge-versus-rules ordering on network delete. Add: **delete every rule naming a subnet or a device
before deleting the device; a half-cleanup that removes the device first is strictly worse than no
cleanup at all.**

### 6.4 A second, different class of surviving state, found today — **NEW**

Already recorded in §2.7 and repeated here because it belongs in §9.1's inventory: a Docker
**packaging** artefact, `/etc/modules-load.d/snap.docker.conf`, loads `br_netfilter` at every boot;
the module is loaded with **refcount 0**; `net.bridge.bridge-nf-call-iptables` is **1 globally**,
altering packet processing for every bridge on the host; and the drop-in is named for a package that
is not the one running. **This outlives not just the process but the uninstall.**

`[R]` **HIGH** — §9.1's inventory table needs a row it does not have:

| Object | Freed when our process dies? | Freed by | Cost of leaking it |
|---|---|---|---|
| **packaging-installed host config** (`modules-load.d`, `sysctl.d`, systemd units) | **no — it outlives the package** | a human, reading a drop-in nobody remembers | global packet-processing change for software unrelated to us |

**And the derived rule: if we ever ship a package, it must not install a `modules-load.d` or
`sysctl.d` drop-in.** Everything we need, we do at runtime, in a namespace, where it dies with us.

### 6.5 The rest of §9 — **CONFIRMED, nothing to correct**

| claim | verdict |
|---|---|
| §9.1 inventory (veths free themselves, netns bind mount is *the* leak, bridge/rules/IPAM/helper/sysctls do not) | **CONFIRMED** — §4.2 reproduced the veth half directly; §4.4 reproduced the bind-mount half |
| §9.3 Docker's startup GC: `reservePools` → `sandboxRestore` → `cleanupLocalEndpoints` → `networkCleanup` → `setupPlatformFirewall` | **CONFIRMED** against the quoted moby sources; `[C]` HIGH |
| §9.3 "without live-restore, every sandbox is stale at daemon start and is force-deleted" | **CONFIRMED** — and `[O]`: `LiveRestoreEnabled: false` on this host, which is the default |
| §9.3 `[R]` HIGH "the record must outlive the host state it describes" | **CONFIRMED and strengthened by §6.2** — it is the single invariant that separates recoverable garbage from unrecoverable |
| §9.4 CNI's DEL contract (idempotent by specification) and the **GC** verb added in spec 1.1.0 | **CONFIRMED** against the SPEC quotes |
| §9.5 conntrack entries defeat correct rules; Docker flushes explicitly; UDP-only for the port-change case | **CONFIRMED** against the quoted moby comment. `[O]`: `nf_conntrack_count` 212–294 / 262144 here |
| §9.6 stale netns placeholder behaviour | **CONFIRMED — independently reproduced verbatim**, Part 4.4 |
| §9.7 ownership table, one idempotent setup path, intent-before-mutation, flock, self-describing naming, the reconcile sweep | **CONFIRMED** as reasoning; the netavark `firewall_reload.rs` / `state.rs` precedents are accurately quoted |
| §9.7 `IFNAMSIZ` = 16 including NUL ⇒ 15 usable, and `br-0d4cf39c58f7` is exactly 3+12 | **CONFIRMED** — counted; it is 15 characters |

One addition to §9.7's reconcile step 4: **use `nft delete table`, never `nft flush table`.** `[O]`
verified on `old-host` — after `flush table inet mydocker`, the named set **and its elements
survived**; only rules were cleared. A reconcile built on `flush` would silently carry stale
container IPs forward in the anti-spoof set, which is precisely the failure the reconcile exists to
prevent. D4 §9.7 says "delete"; it should say *why*.

---

## Part 7 — §10 IPAM and §11 the ladder

### 7.1 §10 — **CONFIRMED, with live confirmation of the sticky-address and random-MAC claims**

| claim | verdict |
|---|---|
| §10.1 Docker's default local pools, in list order, 15×/16 + 16×/20 | **CONFIRMED** against the quoted `ipamutils/utils.go`, and `[O]` the host's allocation history reads straight off it: `docker0`=172.17 (1st), `webapp_default`=172.18 (2nd), orphan=172.19 (3rd) |
| §10.1 IPAM reserves exactly network + broadcast; **the gateway is not special-cased**, it is just first-fit | **CONFIRMED** — `[O]` gateway 172.18.0.1, containers `.2 .3 .4 .5 .6` |
| §10.2 Docker's allocator is **in-memory only**, rebuilt by `reservePools()` replay; `RequiresRequestReplay: true` | **CONFIRMED** against the quoted sources |
| §10.2 `[R]` HIGH "IPAM is an index, not a database; the container record is the truth" | **CONFIRMED** as reasoning; the three arguments (no two-phase commit, free sweep, less code) all hold |
| §10.3 the on-link-route + resolv.conf exclusion heuristics, IPv4-only | **CONFIRMED** against `netutils/utils_linux.go`; and §6.3 above shows the on-link-route heuristic is what makes the orphan safe |
| §10.4 CNI host-local's `flock` + `O_EXCL`-per-IP + `last_reserved_ip` round-robin cursor | **CONFIRMED** against the quoted sources |
| §10.5 the concurrency option table; reject hash-derivation; reject probe-then-claim; `flock` + `O_EXCL` | **CONFIRMED.** The birthday argument is right: a /24 has 253 usable addresses, first collision at ~19 containers |
| §10.6 Docker 28 does **not** derive the MAC from the IP | **CONFIRMED, and now with a restart sample** — see below |

`[O]` **HIGH, new and directly on point** — the containers restarted between D4 and today. Comparing:

| container | IP then | IP now | MAC then (D4, 2026-09-04) | MAC now (2026-10-23) |
|---|---|---|---|---|
| nginx | 172.18.0.4 | **172.18.0.4** | `ee:47:c6:16:34:61` | **`8e:26:a2:33:ab:1e`** |
| frontend | 172.18.0.3 | **172.18.0.3** | `32:fa:e6:8a:a4:eb` | **`5e:f4:eb:4b:7a:9e`** |
| redis / certbot / web | .2 / .5 / .6 | **unchanged** | — | **unchanged** |

Two claims confirmed in one observation:

1. **Addresses are sticky across restart.** The container ids changed (`301b2be6f7c8` →
   `919e37255290`) and the netns bind mounts changed, but the IPs did not. §10.5's `[R]` MED on
   sticky addressing is **CONFIRMED as Docker's live behaviour**, and it is what makes §6.5's
   `/etc/hosts`-injection stage survivable.
2. **MACs are freshly randomised on restart while the IP is reused.** This is **exactly** the
   scenario §10.6's `[R]` HIGH argues for: an IP reused with a *different* MAC produces a clean ARP
   miss and immediate re-resolution, instead of a stale-but-syntactically-valid FDB/ARP entry
   pointing at the wrong port for up to `ageing_time` (300 s, `[O]` confirmed). **CONFIRMED with
   live evidence D4 did not have.**

`[O]` **MED** — one detail D4's §10.6 table got from the earlier sample is now stale in the
*values* but not the *claim*: none of the five MACs is `02:42:` + the IP bytes, then or now.

Nothing in §10 needed correcting.

### 7.2 §11 — **CORRECTED: the ladder has two ordering errors, both in the success tests, and D4 only half-notices one**

D4 §11's table, stages 1–6. Checking the brief's exact question — *does any stage need something a
later stage introduces?*

**Ordering error 1 — stage 4's acceptance test requires stage 6.** The table's stage-4 success test
is *"`apk add curl` succeeds inside"*. That needs **DNS**, which the ladder introduces at stage 6. A
container at stage 4 has an address, a route, working MASQUERADE and no nameserver at all — so the
test fails and the stage looks broken. D4 **half-notices this** in §11.3 (*"the pragmatic first
release is stage 4 + the DNS `resolv.conf` rewrite … pull it in"*) but **never fixes the table**, so
the ladder as written still says stage 4 is testable on its own. **CORRECTED: §6.5-stage-1
(`resolv.conf` + `/etc/hosts` generation, ~30 lines) is a *prerequisite* of stage 4, not a
successor.** It should be folded into stage 4 and stage 6 renamed to "peer-name resolution".
The other stage-4 criterion — *"two containers ping each other by IP"* — is satisfiable and should
be the one that gates the stage.

**Ordering error 2 — stage 5's acceptance test requires a component §4.4 defers.** The table's
stage-5 success test is *"`curl localhost:8080` from the host **and from another machine**"*. Per
§2.3 of this report, `curl localhost:8080` is **deliberately excluded from DNAT** by
`! -d 127.0.0.0/8` in `nat/OUTPUT`; the only things that can serve it are a **userland helper bound
to the loopback address** or `route_localnet=1`, which §4.4 rightly refuses. §4.4 treats the helper
as an optional extra for "the cases that need it". **CORRECTED: the loopback helper is not optional
at stage 5 — it is what makes stage 5's own acceptance test pass.** Either the helper moves into
stage 5's definition, or the test changes to `curl <host-ip>:8080`, which exercises the `nat/OUTPUT`
path instead and is satisfiable with rules alone.

**Everything else in the ordering is correct.** Stage 3 genuinely does not need `ip_forward`
(container↔host over a veth is local delivery, not forwarding) and genuinely does not need IPAM
(hardcode the pair). Stage 5 genuinely needs stage 4's bridge and forwarding. Stage 2 needs only
`lo` up, which it includes.

**Is "host netns only" correctly placed first? — DISPUTED, and §11.1 argues against itself.**

- D4's stage 1 is **`--network=host`** (join the host netns), *not* "no networking at all".
  `--network=none` — a fresh empty netns — is D4's **stage 2**. These are opposite things and the
  brief's phrasing conflates them; D4 does not.
- §11.1 is titled *"stage 1 is real work, and it is not 'no networking'"* and then argues
  *"it is **the absence of work**: no netns, no veth, no IPAM, no rules"*, with the table's host-state
  column reading **"none"**. Both cannot be true. The accurate statement is: **stage 1 is the
  absence of networking work and the presence of a great deal of *non*-networking value**, because
  it unblocks rootfs/exec/cgroups/signals/wait with networking removed as a variable. The
  conclusion (build it first) is right; the justification is muddled and should be restated.
- **The substantive objection D4 does not raise:** `--network=host` is the configuration D5 §16.3
  lists as dangerous shortcut #9 — *"the container reaches every host-local service — including
  `127.0.0.1:2375` …, cloud metadata at `169.254.169.254`, and any localhost-trusting database"*.
  D4 §11.3 then observes that *"anyone with real work to do runs stage 1 instead and gets no
  isolation at all"* — i.e. it predicts users will park on it. **Shipping it first is correct;
  shipping it as the default is not.** `[R]` **HIGH, new:** build stage 1 first, but the moment
  stage 2 exists, `--network=none` becomes the default and `--network=host` requires an explicit
  flag and a one-line warning. Otherwise the first release establishes a default that is a
  documented security anti-pattern, and defaults are the hardest thing to change later.

§11.3 (stage 4 is the first genuinely useful rung), §11.4 (stage 4 = largest irreversible mutation;
stage 5 = highest mutation rate and the only rung where partial state is a *security hole*), and
§11.5 (build networking in, not as an OCI hook; keep CNI's five-verb shape) are all **CONFIRMED**.
§11.5's four arguments against the hook boundary are sound, and the strongest — *"`poststop` only
runs if someone calls `delete`"* — is correct against the runtime-spec wording.

One refinement to §11.4 from Part 3.4: Docker's nftables backend *"will not enable IP forwarding on
the host"*. If we follow that (recommended, §3.4), **stage 4's "largest irreversible global
mutation" shrinks to the bridge + its route**, and `ip_forward` leaves the risk list entirely. That
materially changes §11.4's conclusion and is worth folding in.

---

## Part 8 — §12's crate claims, behaviour only

Scope per the brief: *what abstraction each provides and whether it hides the mechanism we are
trying to learn*. **Maintenance health, release cadence, download counts, RUSTSEC advisories and the
overall dependency set are V7's** — D4 §12.3/§12.4 carry a lot of that and I have deliberately not
re-checked any of it. Handing off: **V7 should re-verify `nftables` 0.6.3's and `netlink-packet-route`
0.33's current versions and advisory status, and adjudicate the `nftnl` RUSTSEC-2025-0126 /
`mnl` RUSTSEC-2025-0142 pair.**

### 8.1 `nftnl` vs the `nftables` crate vs hand-rolled — **DISPUTED within D4 itself; §12.4 is right and §5.4 is wrong**

D4 contradicts itself and flags it: **§5.4 `[R]` MED** recommends *"link `libnftnl` via the `nftnl`
crate rather than hand-rolling"*, treating `nft -f -` as *"an honest stopgap"*. **§12.4 `[R]` HIGH**
recommends the **`nftables` crate** (typed Rust → nft JSON → the `nft` binary) *permanently*,
observing it is what netavark actually ships, and §12.4 explicitly notes the contradiction.

**Adjudicated on the brief's criterion — what does each hide?**

| option | what it provides | what it hides | does it hide the mechanism we want to learn? |
|---|---|---|---|
| **hand-rolled nft netlink** | everything | nothing | **No — but it is the wrong thing to learn.** The nftables expression encoding is a large, evolving, bytecode-like IR. Owning it teaches us about *nftables' internals*, not about container engines. D4 §12.5's framing of this is exactly right. |
| **`nftnl` (FFI to `libnftnl`)** | C-library expression building | the netlink wire format | **No, and that is its only advantage.** In exchange it hides nothing we wanted hidden and adds a build-time/run-time ABI coupling: `nftnl-sys` ships 26 checked-in bindgen outputs selected by cumulative Cargo features, so *which `libnftnl.so` is on the host* becomes a build decision. For a binary meant to run on both Ubuntu 24.10 and Rocky 8.8 that is a distribution problem, not a footnote. |
| **`nftables` crate** | typed Rust structs → nft's JSON API → `nft -j -f -` | the expression encoding **and** the netlink transport | **It hides the encoding, not the semantics** — and the semantics are the whole of §4 and §5. Which chains, which hooks, which priorities, which sets, in what order and why: all of that stays ours, visible in our own source, and is the part the project exists to understand. |

**Verdict: DISPUTED, and §12.4 governs.** The `nftables` crate is the right choice and it is not a
stopgap. Three reasons, in order:

1. **It delivers the four properties §5.4 actually asked for** — atomicity (one `nft -j -f -` is one
   kernel transaction), typed construction, named sets/maps, and single-table `inet` — which is the
   entire content of the recommendation. The transport was never the point.
2. **`[O]` verified** on `old-host`: the underlying mechanism (`nft -f <file>` as an atomic
   whole-table apply) works, **including rootless on kernel 4.18**, which is the degradation probe.
   Whatever we build on top of it degrades gracefully.
3. **§12.5's deciding question is the right one** — *"is the thing I need back from this tool an
   error I have to branch on?"* — and it answers cleanly here. §5.4's design produces **one** nft
   operation per container (a whole-table transaction). A failed transaction means **nothing
   happened**, which is a complete and actionable answer with or without an errno. That is the exact
   opposite of `ip`, where ~12 per-operation failures each demand a *different* recovery decision.

`[R]` **CORRECTED** — §5.4's `[R]` MED should be struck and replaced by §12.4's. The `nft -f -` path
is not a shortcut with a known ceiling; **it is the design**, and `nftnl`/`rustables` are a possible
future optimisation behind the firewall trait that §5.4 correctly insists on either way.

One operational caveat the brief's criterion surfaces and D4 states only in passing: **a runtime
dependency on the `nft` binary.** `[O]` — present on both probe hosts (`v1.1.0` on `host-a`,
`v1.0.4` on `old-host`). `[R]` **HIGH** — probe for it **at startup**, not mid-setup, and fail with
the package name. And note the version spread: `nft` 1.0.4 on RHEL 8 **rejects some syntax 1.1.0
accepts** (`[O]`: `nft add chain … fwd {…}` fails on 1.0.4 because `fwd` is a reserved keyword
there). Using the crate's **JSON** API rather than generating nft script text sidesteps that class
of problem entirely — which is a concrete argument for `nftables` over `nft -f -` with hand-written
text that D4 does not make.

### 8.2 `rtnetlink` / `netlink-packet-route` vs writing the messages ourselves — **CONFIRMED, and this is the sharpest reasoning in §12**

| option | what it hides | verdict on the brief's criterion |
|---|---|---|
| `netlink-sys` | `socket()/bind()/sendto()` + `setsockopt` | **hides nothing we care about.** Take it — and note it is the crate that exposes `NETLINK_GET_STRICT_CHK`, which netavark sets and without which filtered dump requests silently return the whole table |
| `netlink-packet-core` | `nlmsghdr` as Rust types | **hides nothing.** It *is* the struct |
| `netlink-packet-route` | **the TLV byte layout** | **hides one thing, and it is the right one to hide.** ~47k lines of attribute tedium. D4's advice — read `link_info/veth.rs` once to see the embedded-`ifinfomsg` trick, then use the crate — is exactly right, and my Part 4.3 confirms the trick is the whole difficulty |
| `netlink-proto` | sequencing and demux | **hides what we want to own.** Avoid |
| **`rtnetlink`** | **message construction *and* the executor choice** | **avoid, and the second half is the real objection.** It has no `sync` feature; `netlink-proto` is an unconditional dependency; the only choice is tokio vs smol |

**CONFIRMED, and §12.1's constraint argument is the most valuable thing in §12.** Three facts from
earlier in D4 compose into it and all three survived verification:

1. a netlink socket is permanently bound to the netns it was created in;
2. `setns` is **per-thread** and there is no whole-process form (`[F]` CONFIRMED, `setns(2)`);
3. §1.5's trick requires creating a socket *inside* a netns and serving it from outside — `[O]`
   **re-confirmed live today**: `dockerd` pid 70234 owns fds 57/60 listening on `127.0.0.11` inside
   a container netns it does not live in (Part 5.1).

The primitive our domain needs is therefore **"run this closure on a dedicated OS thread pinned into
netns X, then restore"** — a *thread* primitive, hostile to a work-stealing executor that may move a
task between workers at any `.await`. `[R]` **CONFIRMED and I would strengthen it**: a tokio worker
thread that has been `setns`'d is a landmine for every unrelated task the scheduler later hands it,
and the bug it produces (a socket silently created in the wrong namespace) is invisible until
something fails to receive traffic. **Nothing in D4 requires async; D4's central primitive is
actively hostile to one. → stands as a constraint handed to D2, not a vote.**

`[F]` **CONFIRMED and worth repeating because it bears on §9.7** — `netlink-packet-route`'s own
source comment records that **you cannot read a veth's peer back out of `IFLA_LINKINFO`/`INFO_DATA`**
(`veth_link_ops` has no `fill_info`); the peer is discoverable only as `IFLA_LINK`, an ifindex.
`[O]` **confirmed empirically** in Part 4.3: the displayed form is `hostv@if2` / `ceth@if2` — an
ifindex in the *other* namespace, with the name not preserved. **So our reconcile sweep must match
host veths on the name prefix, never on peer information.** This is the one place where the
TLV-hiding crate's behaviour leaks into our architecture, and D4 catches it.

---

## Corrections that change a recommendation

- **§1.4 / §8.2 `[O]` HIGH "slirp4netns fails on `old-host` with `setns(CLONE_NEWNET): EPERM`"** → it succeeds, end to end, from a plain unprivileged login shell in the initial userns, with and without `--userns-path`, because slirp4netns ≥1.0 opens `/proc/<pid>/ns/user` itself and `setns`es it first → **invalidates §8.2's `[R]` HIGH "a rootless network helper must be launched from a process already inside the container's userns", and the process-topology constraint it hands to D2. Delete both.**
- **§8.5 `[R]` HIGH "set `ping_group_range` inside the container netns; it is per-netns and free"** → rootless it returns `EINVAL` unless **both** bounds are gids mapped in our own userns; under a single-gid map only `0 0` is writable → **shifts the `[R]` to "attempt it, tolerate `EINVAL`, never treat it as a setup failure".**
- **§1.1 `[O]` "fresh netns `ping_group_range = 65534 65534`, restrictive"** → that *is* the upstream empty range `1 0`, rendered through a userns where init-gids 0 and 1 are unmapped → **never read the current value to decide whether to write; an implementation that sees `65534 65534` and concludes "a range is configured" is silently wrong.**
- **§3.5 `[F]/[C]` MED "container → own published port is blocked without bridge hairpin mode"** → hairpin is **neither necessary nor sufficient** (verified both directions on `old-host`); the missing piece is a **bridge-to-itself MASQUERADE**, because the packet is *routed* by the local stack, not *switched*, so 802.1D port isolation never applies → **§3.5's `[R]` MED "skip hairpin" survives as a decision but its reasoning is wrong; when we do want the case, it is one MASQUERADE rule in the nft transaction we already build, not an `IFLA_BRPORT_HAIRPIN_MODE` attribute.**
- **§5.2 `[F]` HIGH "a terminal verdict … (`accept` … `drop` …) can end evaluation before the higher-priority table is consulted"** → only `drop` does; `accept` ends **the current base chain** only and every other base chain at that hook still runs; a packet is accepted iff nothing anywhere drops it → **this error conceals the decisive operational risk that §5.4 states correctly in its own tradeoff list.**
- **§5.4 `[R]` HIGH "write native nftables in our own table and never touch iptables at all"** → the absolute form is not achievable: on any host with a foreign `filter/FORWARD` **policy DROP** (ufw, firewalld, a prior Docker install — i.e. `host-a` right now) our `accept` in our own table cannot save the packet → **DISPUTED. Keep every rule of ours native, but ship an explicit, logged decision: detect the foreign DROP and refuse to publish, or write exactly one marked `accept` into the foreign chain. Shipping the absolute form means containers with no network on a stock `ufw` host and no error anywhere.**
- **§5.3 `[C]` HIGH "Docker 28 introduced a firewall backend abstraction; iptables is still what ships"** → **Docker 29.0.0 shipped an experimental nftables backend on 2025-11-10 — ten months *before* D4 was written** (`--firewall-backend=nftables`); 35 releases since, latest **29.9.2 (2026-10-20)**. This is a miss, not drift: D4 inferred upstream's state from one un-upgraded production host and never checked the release feed → and critically, **Docker's nftables mode has no `DOCKER-USER` chain at all; the admin override is "a table at a priority relative to Docker's" → invalidates §4.6's `[R]` HIGH "create a `MYDOCKER-USER` equivalent". There is nothing to create. Our chain priorities become a public API and must be documented and frozen.**
- **§5.3 `[C]` MED–HIGH with `[LOW]` on the removal timeline (netavark)** → **netavark v2.0.0 (10 Jun 2026) "Removed iptables support"**; v2.1.0 (06 Aug 2026) is current → **retire the `[LOW]`; §5.4's "direction of travel" reason is now `[C]` HIGH.**
- **§3.3 / §11.4 `ip_forward` as a mutation we must make carefully** → Docker's own nftables backend *"will not enable IP forwarding on the host"* → **`[R]` detect `ip_forward=0`, fail the network create with the exact `sysctl` command, and never write it. That deletes §1.1's "writing `ip_forward` resets all per-interface config" booby-trap from our code entirely and removes the largest irreversible mutation from §11.4's stage-4 risk list.**
- **§3.4's implied premise that the global `bridge-nf-call-iptables=1` is somebody else's doing** → on a packaged Docker install it is **Docker's**: `/etc/modules-load.d/snap.docker.conf` loads `br_netfilter` at every boot (refcount 0, drop-in named for a package that is not the one running) → **§9.1's inventory needs a "packaging-installed host config" row — the one class of state that outlives even an uninstall — and the rule "if we ship a package, it installs no `modules-load.d` or `sysctl.d` drop-in".**
- **§9.2's mechanism "filter chains are rebuilt at daemon start; nat/raw/device are written at create time and only removed by an explicit delete"** → the real split is **own chains vs built-in chains**: `nat/DOCKER`'s per-network RETURN rule is *also* create-time and *was* cleaned → **rules in chains we own self-heal by flush-and-rebuild; rules in chains we share are deletable only by exact match from a record, so they orphan permanently. This is the strongest form of §5.4 reason 4 and D4 does not make it: with `table inet mydocker` the permanent-orphan class disappears by construction — except for the one foreign `accept` rule above, which is therefore the only rule of ours that needs a greppable marker and an explicit line in the reconcile sweep.**
- **§9.7's teardown ordering block (netns first, record last) says nothing about bridge-vs-rules** → deleting the orphan bridge first makes its subnet allocatable again while leaving a MASQUERADE whose `! -o <dead device>` **can never match** (so every packet of the new subnet, including container-to-container, is masqueraded) and two `raw` DROPs that blackhole the new `.2`/`.3` before conntrack → **add: delete every rule naming a subnet or a device *before* deleting the device. A half-cleanup that removes the device first is strictly worse than no cleanup.**
- **§4 never traces the host-originated path, and `nat/OUTPUT`'s `! -d 127.0.0.0/8` is listed in §4.1 and explained nowhere** → **three nat rules per published port, not two**, and **loopback is deliberately excluded from DNAT** → **§4.4's `[R]` HIGH must say so explicitly, or we ship a host that cannot `curl localhost:<published>` and cannot explain why.**
- **§4.2 "the `-i <bridge> -j RETURN` rule prevents container-origin traffic being DNAT'd by another container's publish rule"** → it also makes the **hairpin case unreachable by DNAT before L2 is ever consulted** (verified: with that rule in place the probe gets `Connection refused`, not a timeout) → **this, not L2 hairpin, is the real mechanism behind §4.4's `docker-proxy` role 4.**
- **§11's ladder: stage 4's acceptance test is "`apk add curl` succeeds"** → that needs DNS, which the ladder introduces at stage 6; §11.3 half-notices this and never fixes the table → **fold §6.5-stage-1 (`resolv.conf` + `/etc/hosts`, ~30 lines) into stage 4 and gate stage 4 on "two containers ping each other by IP".**
- **§11's ladder: stage 5's acceptance test is "`curl localhost:8080` from the host"** → excluded from DNAT by `! -d 127.0.0.0/8`, so it is unsatisfiable without the loopback helper §4.4 treats as optional → **either move the helper into stage 5's definition or change the test to `curl <host-ip>:8080`.**
- **§11.1 "stage 1 is real work" while its own table says host state = none and its own text says "it is the absence of work"** → build `--network=host` first (correct), **but it must not be the default once stage 2 exists** — it is D5 §16.3's dangerous shortcut #9, and §11.3 itself predicts users will park on it → **`[R]` the moment `--network=none` exists it becomes the default; `--network=host` needs an explicit flag and a one-line warning.**
- **§5.4 `[R]` MED "link `libnftnl` via the `nftnl` crate; `nft -f -` is an honest stopgap" vs §12.4 `[R]` HIGH "take the `nftables` crate"** → D4 contradicts itself and flags it; **§12.4 governs. Strike §5.4's.** The crate's JSON API also sidesteps a real problem `nft -f -` with generated text has: `nft` 1.0.4 (RHEL 8) rejects syntax 1.1.0 accepts.
- **§9.7's reconcile step 4** → **use `nft delete table`, never `nft flush table`**: verified on `old-host` that after a `flush`, the named set **and its elements survive**, so a flush-based reconcile silently carries stale container IPs forward in the anti-spoof set.
- **§8.7 `[R]` HIGH "shell out to `pasta`; the ecosystem has converged"** → `pasta` is **absent** on `old-host`, the RHEL-8 degradation probe, while `slirp4netns` is present and working → **`[R]` probe for both; prefer `pasta`, fall back to `slirp4netns`. Two binaries, not one.**
- **D5 §17 "move one veth end to the host netns — needs `CAP_NET_ADMIN` in the host netns", `[F]` HIGH from `capabilities(7)`/`ip-link(8)`, `not tested`** → capabilities live in **user** namespaces, not network namespaces; the gate is `CAP_NET_ADMIN` in **the user namespace that owns the target netns**, and a *sibling rootless* netns — not the host's — is equally unreachable (verified) → **D4's framing governs; D5's cell and its cited basis are both wrong.**
- **§6.5 stage 3 `[R]` MED "prefer binding `127.0.0.11:53` directly over the DNAT/SNAT dance"** → the live evidence strengthens it: the DNAT ports were re-rolled on container restart (`39251/55634` → `42041/43967`), so the four per-netns rules must be rewritten on **every** start → **raise to `[R]` HIGH.**

## Still unverifiable, and the exact test

- **Does bridge hairpin mode become necessary when `br_netfilter` is active with the per-bridge `nf_call_iptables` flag set?** (My Part 2.9 matrix ran with `/proc/sys/net/bridge` absent.) → a disposable **root** VM: `modprobe br_netfilter; sysctl net.bridge.bridge-nf-call-iptables=1; ip link set br0 type bridge nf_call_iptables 1`, then re-run the five-row matrix. Not `host-a` (loading a module and flipping a global bridge sysctl on a live production host), not `old-host` (no root to `modprobe`).
- **The `NETLINK_EXT_ACK` strings D4 quotes** — *"The interface netns is immutable"*, *"An interface with the same name exists in the target netns"* — did not appear on `old-host`'s 4.18 kernel (bare `EINVAL` / `EEXIST`). → on a kernel ≥5.15: `unshare -Urn sh -c 'ip -N link set lo netns 1'` and the duplicate-name create, with `strace -e sendmsg,recvmsg` to read `NLMSGERR_ATTR_MSG`. Settles whether we may ever log the text (we still must branch on the errno).
- **Docker's stated rationale for the embedded resolver's random high port** (§6.1 `[MED]`: conflict avoidance against a container binding `0.0.0.0:53`) → the commit message / design note for libnetwork's resolver: `git -C moby log --follow -p daemon/libnetwork/resolver.go | grep -i -B20 'random\|ephemeral\|53'`.
- **Docker's precise `/etc/resolv.conf` decision tree** (§6.3 `[F]` MED: all-loopback ⇒ internal resolver, else filter and copy) → read `daemon/libnetwork/internal/resolvconf` at a pinned tag in moby.
- **Docker 29's nftables table structure and the actual numeric priorities of its base chains** — the docs say only *"well known priority values"*. We need the numbers before we can place ours relative to them. → a host running 29.x with `"firewall-backend": "nftables"`: `nft list table ip docker-bridges` and `nft list table ip6 docker-bridges`. Not possible on `host-a` (28.3.1, and changing the backend is a mutation).
- **The provenance of the `br-0d4cf39c58f7` orphan** (§9.2 `[LOW→MED]`: partial `network rm`, daemon kill mid-removal, or a compose race) → `journalctl -u docker --no-pager | grep -i 0d4cf39c58f7` on `host-a`. Read-only and permitted, but the bridge's ifindex (26 vs the live 67) says it predates the current journal; I expect the logs have rotated. The *state* is `[O]` HIGH regardless and the state is the point.
- **`--live-restore`'s actual effect on rule recreation** (§9.3 `[C]` LOW→MED, sourced to issue reports) → a disposable VM with 50 published ports: `systemctl restart docker` with and without `live-restore`, timing the outage and `diff`ing `iptables-save` before and after. `host-a` has `LiveRestoreEnabled: false` and restarting its daemon is out of the question.
