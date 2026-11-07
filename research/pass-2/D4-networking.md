# D4 — Container Networking & Host Integration

Research reference for a from-scratch Rust container engine. Root-first, Linux-only, x86_64.

**Claim tags:** `[F]` Fact (Linux behaviour, primary-sourced) · `[C]` Convention (what Docker/CNI/Podman happen to do) · `[R]` Recommendation (ours, with tradeoff) · `[O]` Observation (empirically verified, host+kernel named).
**Confidence:** HIGH / MED / LOW. Unsourced ⇒ LOW, with what would settle it.

**Empirical hosts referenced throughout:**
- `host-a` = `HOST-A`, Ubuntu 24.10, kernel `6.11.0-29-generic`, Docker `28.3.1`, `iptables v1.8.10 (nf_tables)`, `nftables v1.1.0`. Live production, read-only inspection only. Five running containers on a user-defined bridge with published :80/:443.
- `old-host` = `OLD-HOST`, Rocky Linux 8.8, kernel `4.18.0-425.3.1.el8.x86_64`, unprivileged (uid 900000000), unprivileged user namespaces enabled (`max_user_namespaces=1029387`), `slirp4netns 1.2.3` / `libslirp 4.4.0` present, no `passt`/`pasta`.

---

## 0. The OCI boundary: networking is deliberately out of scope

`[F]` **HIGH** — The OCI Runtime Spec does not define container networking. It defines a `linux.namespaces` array where an entry `{"type": "network"}` with no `path` means "create a new network namespace", and with a `path` means "join this existing one". That is the entire networking surface of the spec. There is no interface definition, no IPAM, no address, no route, no port mapping.
Source: <https://github.com/opencontainers/runtime-spec/blob/main/config-linux.md#namespaces>

`[F]` **HIGH** — The spec's ordering guarantee is what makes networking possible at all: `createRuntime` hooks run **after** the container namespaces are created but **before** `pivot_root`, and they run **in the runtime's namespace**, not the container's. So a hook receives the container PID on stdin (as part of the `state` JSON) and can therefore reach `/proc/<pid>/ns/net`. `createContainer` and `startContainer` hooks run *inside* the container namespaces. This is the seam every CNI-based stack uses.
Source: <https://github.com/opencontainers/runtime-spec/blob/main/config.md#posix-platform-hooks>

`[F]` **HIGH** — CNI, the de-facto standard that fills the gap, is explicit that the split is: *"The container runtime must create a new network namespace for the container before invoking any plugins."* Plugins configure the namespace; they never create it.
Source: <https://github.com/containernetworking/cni/blob/main/SPEC.md>

`[R]` **MED** — **Build networking in, not as a hook, but structure it behind an internal trait shaped like CNI's ADD/DEL/CHECK.** Reasoning: the hook boundary buys process isolation we do not want yet (a hook failure mid-setup is harder to reason about than an in-process error, and we would have to serialize state to JSON to cross it). But CNI's *shape* — "runtime owns the netns, the network plugin owns everything inside plus the host-side glue, DEL is idempotent and best-effort" — is a genuinely good factoring that we get for free by copying the interface. Tradeoff: if we later want third-party CNI plugin compatibility (a real ask for Kubernetes interop) we will need to shell out to plugin binaries anyway; keeping the internal trait CNI-shaped makes that a wrapper rather than a rewrite.

`[R]` **HIGH** — Do **not** try to be OCI-runtime-compatible *and* own networking in the same binary boundary at first. `runc` is not responsible for networking and neither should our runtime-core be. Put networking in the daemon/CLI layer that *calls* the runtime core. See Handoff to D2.

---

## 1. Network namespace lifecycle

### 1.1 What a fresh netns contains at birth

`[O]` **HIGH** — On `old-host` (Rocky 8.8, 4.18.0-425.3.1.el8):
```
$ unshare -Urn sh -c 'ip -o link show; echo ---; ip -o addr show; echo ---; ip route show'
1: lo: <LOOPBACK> mtu 65536 qdisc noop state DOWN mode DEFAULT group default qlen 1000
---
---            # NO ADDRESSES AT ALL
---            # NO ROUTES AT ALL (exit 0, empty output)
```
A newborn netns contains **exactly one device, `lo`, and it is DOWN with `qdisc noop` and — critically — has no `127.0.0.1` address.**

`[O]` **HIGH** — Same host, the address appears only after the link is brought up:
```
$ unshare -Urn sh -c 'ip link set lo up; ip -o addr show lo'
1: lo    inet 127.0.0.1/8 scope host lo
1: lo    inet6 ::1/128 scope host
```

`[F]` **MED** — The mechanism: `127.0.0.1/8` is added by the kernel's inetdev init path in response to the `NETDEV_UP` notification for a loopback device, not at namespace creation. (`net/ipv4/devinet.c`, `inetdev_event` → `inetdev_init`, which for `IFF_LOOPBACK` calls `inet_insert_ifa` with the loopback address.) **What would settle it definitively:** reading `inetdev_init` in the exact kernel version; I verified the observable behaviour but not the code path line-by-line.

`[R]` **HIGH** — **Always `ip link set lo up` (RTM_NEWLINK, `IFF_UP`) in a new container netns, even for `--network=none`.** Reasoning: an enormous amount of software assumes `127.0.0.1` works — Python's `multiprocessing`, JVM internals, anything binding a local listener for IPC. A container with a down loopback fails in confusing, non-obvious ways. This is one line and it is the single highest value-per-byte thing in this whole document. Docker does it; so should we.

`[O]` **MED** — Fresh-netns sysctl defaults on `old-host` (4.18):
```
ipv4/ip_forward              = 1     <-- INHERITED from the host's value, not 0
ipv4/conf/all/rp_filter      = 1
ipv4/conf/all/route_localnet = 0
ipv4/ping_group_range        = 65534 65534   (host: 0 2147483647)
ipv4/ip_local_port_range     = 32768 60999
ipv4/ip_unprivileged_port_start = 1024
/proc/sys/net/bridge         = ENOENT (does not exist until br_netfilter is active in that netns)
```
Two of these matter a lot:
- `ip_forward = 1` in a brand-new namespace was **surprising**. The upstream documented default is 0. The likely mechanism is that new netns copy the `ipv4_devconf` template, which the initial netns has already mutated. `[LOW]` on the mechanism. **Do not assume a fresh netns has forwarding off; read it, don't guess.** What would settle it: `devinet_init_net()` in `net/ipv4/devinet.c` for this kernel, plus a repeat on a host where `ip_forward=0`.
- `ping_group_range` is restrictive in a fresh netns (`65534 65534` here; upstream default is `1 0`, i.e. *nobody*). This is exactly why `ping` fails in fresh containers unless you either grant `CAP_NET_RAW` or widen `ping_group_range`. `[F]` on the semantics: *"Restrict ICMP_PROTO datagram sockets to users in the group range. The default is '1 0', meaning, that nobody (not even root) may create ping sockets."* — <https://www.kernel.org/doc/Documentation/networking/ip-sysctl.txt>

`[F]` **HIGH** — **Booby trap: writing `net.ipv4.ip_forward` resets other config.** From `ip-sysctl.txt` on `ip_forward`: *"This variable is special, its change resets all configuration parameters to their default state (RFC1122 for hosts, RFC1812 for routers)."* So if we set `ip_forward=1` *after* configuring per-interface knobs (`rp_filter`, `accept_local`, `route_localnet`, `arp_ignore`…), we silently blow them away.
`[R]` **HIGH** — **Set `ip_forward` first, before any per-interface sysctl, and only if it is not already 1.** Read-then-write, never blind-write: a blind write on a host that already had it enabled is a gratuitous reset of the admin's tuning. This is a real production-breaking footgun.

`[F]` **HIGH** — The entirety of `/proc/sys/net` is per-netns. `[O]` **HIGH** verified on `old-host`: writing `1` to `/proc/sys/net/ipv4/ip_forward` inside `unshare -Urn` succeeded and the host value was unchanged. This is why network sysctls are safe to tune per-container and why `--sysctl net.*` works in Docker.

### 1.2 nsfs, and why `/var/run/netns/<name>` is a *convention*

`[F]` **HIGH** — Namespaces are exposed as files in a special filesystem, `nsfs`. `/proc/<pid>/ns/net` is a magic symlink whose target text is `net:[<inode>]`; opening it yields an fd referring to the nsfs inode. Two processes are in the same netns iff those inodes match.
Source: `namespaces(7)`, <https://man7.org/linux/man-pages/man7/namespaces.7.html>

`[O]` **HIGH** — On `host-a`: `readlink /proc/2588141/ns/net` → `net:[4026532504]`.

`[F]` **HIGH** — A namespace lives as long as *something* references it: a process's `nsproxy`, an open fd, or **a bind mount of the nsfs file**. The bind mount is the trick that lets a netns outlive every process in it.

`[O]` **HIGH** — Verified the mechanism directly on `old-host`, inside `unshare -Urmn` with a private tmpfs on `/var/run`:
```
$ ip netns add p1
$ ls -la /var/run/netns/
-r--r--r-- 1 nobody nobody  0 Aug 23 13:05 p1
$ grep nsfs /proc/self/mountinfo
3044 3043 0:4 net:[4026534758] /run/netns/p1 rw shared:1385 - nsfs nsfs rw
$ stat -c '%i %F %s %A' /var/run/netns/p1
4026534758 regular empty file 0 -r--r--r--
```
Three things to read off this:
1. It is an **ordinary bind mount** whose source is `net:[4026534758]` on filesystem `nsfs` and whose mount type is `nsfs`. Nothing kernel-special about the path.
2. The **inode number of the mounted file equals the namespace id** (`4026534758` in both `stat -c %i` and the `net:[...]` root). So `stat().st_ino` on a netns fd *is* the namespace identity — useful for cheap identity comparison without string parsing.
3. Size 0, mode `-r--r--r--`. It looks like an empty regular file to userspace.

`[F]` **HIGH** — `ip netns list` does nothing but `readdir("/var/run/netns")`. `ip-netns(8)` states the design outright: *"By convention a named network namespace is an object at /var/run/netns/NAME that can be opened. The file descriptor resulting from opening /var/run/netns/NAME refers to the specified network namespace."* The word is *convention*.
Source: <https://man7.org/linux/man-pages/man8/ip-netns.8.html>

`[O]` **HIGH** — The convention's non-universality, demonstrated live on `host-a`:
```
$ ip netns list          # → EMPTY, exit 0
$ ls /var/run/netns/     # → empty directory
$ lsns -t net
4026531840 net 141       1 root  unassigned
4026532363 net   1   82271 do-agent  0 /run/docker/netns/01bf8848396b   redis-server
4026532437 net  12 2590614 root      4 /run/docker/netns/226fec94f918   gunicorn...
4026532504 net   2 2588141 root      2 /run/docker/netns/27f65303b172   nginx
4026532578 net   2   82533 root      3 /run/docker/netns/0d9ef409f77b   certbot loop
4026532709 net   5 2585529 root      1 /run/docker/netns/a3a7dfcf557d   nginx
$ ls -la /run/docker/netns/
-r--r--r-- 1 root root 0 Mar  2 21:50 01bf8848396b
-r--r--r-- 1 root root 0 Aug 12 00:46 226fec94f918
... (5 files)
```
Five live container network namespaces, and **`ip netns list` reports none of them**, because Docker bind-mounts into `/run/docker/netns/` instead. This is the cleanest possible proof that the directory is a userspace convention, and it is also the reason "`ip netns exec`" advice on the internet doesn't work for Docker containers.

`[R]` **MED** — **Use our own directory, `/run/mydocker/netns/<container-id>`, and do the bind mount ourselves.** Reasoning: (a) we do not want `ip netns delete` from a passing admin to nuke our state; (b) it keeps our state self-describing and greppable; (c) it costs three syscalls. Tradeoff: `ip netns exec <name>` won't work for debugging, which is a genuine ergonomic loss — mitigate by shipping `mydocker exec` and by printing the nsfs path in `mydocker inspect`. Docker made exactly this call.

`[R]` **LOW→MED** — Consider *additionally* symlinking or bind-mounting into `/var/run/netns/` for debuggability. Docker doesn't. The reason not to: two mounts of the same nsfs inode means two things to leak, and `ip netns delete` on the symlink would unmount one of them and confuse our own bookkeeping. Probably skip. What would settle it: whether `mydocker exec` lands early enough that nobody misses `ip netns exec`.

### 1.3 Creating and persisting a netns: the actual sequence

`[F]` **HIGH** — Two ways to get a netns:
1. `unshare(CLONE_NEWNET)` — the *calling thread* moves into a new netns. Requires `CAP_SYS_ADMIN` in the current userns. (`unshare(2)`)
2. `clone(..., CLONE_NEWNET, ...)` — the *child* starts in a new netns.
Source: <https://man7.org/linux/man-pages/man2/unshare.2.html>

`[F]` **MED** — The `ip netns add NAME` sequence, from `ip/ipnetns.c` in iproute2, is essentially:
```
mkdir -p  /var/run/netns                      (once)
# make /var/run/netns a mount point so it survives and is shared:
mount --bind /var/run/netns /var/run/netns ; mount --make-shared /var/run/netns
fd = open("/var/run/netns/NAME", O_RDONLY|O_CREAT|O_EXCL, 0)   # empty placeholder file
unshare(CLONE_NEWNET)                                          # we are now in the new netns
mount --bind /proc/self/ns/net /var/run/netns/NAME              # pin it
# (iproute2 then setns() back / exits; the bind mount holds the ns alive)
```
The `--make-shared` self-bind exists so the mounts propagate to other mount namespaces (so a container in a private mount ns can still see them). **`[MED]`** — I read this from the documented behaviour and secondary descriptions rather than the file itself. What would settle it: read `netns_add()` in `iproute2/ip/ipnetns.c`.

`[R]` **HIGH** — **Our sequence should be: fork a helper (or use a thread), `unshare(CLONE_NEWNET)` in it, bind-mount `/proc/self/ns/net` onto our placeholder file, then let the helper exit.** Notes:
- Do this in a *thread or forked child*, never on the main thread of a multi-threaded daemon — `unshare(CLONE_NEWNET)` affects only the calling thread, and having one thread of the daemon silently in a different netns than its siblings is a spectacular source of "why did this socket end up in the wrong namespace" bugs. This is a hard constraint on our threading model → **Handoff to D2.**
- Alternatively: never unshare in the daemon at all. Let the runtime core create the netns as part of container creation (per the OCI `namespaces` array), then bind-mount `/proc/<container-pid>/ns/net`. This is cleaner and it is what the OCI split implies.

`[R]` **MED** — Prefer the second: **let container creation make the netns, then pin it by bind-mounting `/proc/<pid>/ns/net`.** Tradeoff: it means networking setup happens *after* the container process exists, so the container's first instructions run before `eth0` appears. Docker/CNI solve this by having the container process block (on a socket / a `createRuntime` hook completing) before `execve`ing the user's entrypoint. We need that synchronization point anyway for other reasons → **Handoff to D2.**

### 1.4 Entering a netns: `setns(2)` — and a check that will bite us

`[F]` **HIGH** — `setns(fd, CLONE_NEWNET)` moves the calling thread into the netns referred to by `fd`. `fd` can come from `/proc/<pid>/ns/net` or from a bind-mounted nsfs path.
Source: <https://man7.org/linux/man-pages/man2/setns.2.html>

`[F]` **HIGH** — **`setns` into a netns requires `CAP_SYS_ADMIN` in *two* user namespaces, not one.** Exact kernel source, `net/core/net_namespace.c`, Linux v6.11:
```c
static int netns_install(struct nsset *nsset, struct ns_common *ns)
{
	struct nsproxy *nsproxy = nsset->nsproxy;
	struct net *net = to_net_ns(ns);

	if (!ns_capable(net->user_ns, CAP_SYS_ADMIN) ||
	    !ns_capable(nsset->cred->user_ns, CAP_SYS_ADMIN))
		return -EPERM;

	put_net(nsproxy->net_ns);
	nsproxy->net_ns = get_net(net);
	return 0;
}
```
Source: <https://raw.githubusercontent.com/torvalds/linux/v6.11/net/core/net_namespace.c>
Read it carefully: `CAP_SYS_ADMIN` in the **target netns's owning userns** *and* `CAP_SYS_ADMIN` in the **caller's own current userns**.

`[O]` **HIGH** — This is not academic; it is exactly why my `slirp4netns` attempt failed on `old-host`:
```
$ unshare -Urn sh -c 'ip link set lo up; sleep 8' &
$ slirp4netns --configure --mtu=65520 $CPID tap0
setns(CLONE_NEWNET): Operation not permitted
child failed(1)
$ slirp4netns --configure --mtu=65520 --userns-path=/proc/$CPID/ns/user $CPID tap0
setns(CLONE_NEWNET): Operation not permitted     # still fails
```
`slirp4netns` runs as my unprivileged uid in the **initial** userns. The first check passes (I am the owner of the child userns, so `ns_capable` grants via the `ns->parent == cred->user_ns && ns->owner == cred->euid` branch of `cap_capable()`), but the **second** check — `CAP_SYS_ADMIN` in my *own* current userns, the initial one — fails, because I am a normal user there.

`[R]` **HIGH** — **The correct order is always: `setns(userns)` first, then `setns(netns)`.** Joining the userns gives you a full capability set in it, which makes `nsset->cred->user_ns` the container's userns where you *do* have `CAP_SYS_ADMIN`, and both checks then pass. Getting this backwards produces an `EPERM` that looks like a permissions bug in the wrong place. This is the single most confusing failure I hit in this whole investigation and it will bite us the first time we write a rootless helper.

`[F]` **HIGH** — `setns` for a netns must be called from a single-threaded process, or more precisely: it affects only the calling thread, so in a multithreaded Rust program you must `setns` on a dedicated thread and restore afterwards. There is no "setns the whole process" operation.

### 1.5 The socket-creation trick (this is how Docker's DNS works)

`[F]` **HIGH** — **A socket's network namespace is fixed at `socket(2)` time from the creating thread's netns; it does not follow the process afterwards.** So you can: `setns` into the container netns → `socket()` + `bind()` → `setns` back to the host netns → and keep serving that socket from a process that lives in the host netns.

`[O]` **HIGH** — Docker does exactly this, verified on `host-a`. `dockerd` is pid 70234 and lives in the host netns, yet:
```
$ nsenter -t 2588141 -n ss -lunp
UNCONN 0 0 127.0.0.11:55634 0.0.0.0:* users:(("dockerd",pid=70234,fd=52))
$ nsenter -t 2588141 -n ss -ltnp
LISTEN 0 4096 127.0.0.11:39251 0.0.0.0:* users:(("dockerd",pid=70234,fd=54))
```
The listening sockets are **inside the container's netns** (they appear in that netns's socket table) while the owning process is **outside** it. fd 52 (UDP) and fd 54 (TCP) were created while a `dockerd` thread was temporarily `setns`'d into the container netns.

`[R]` **HIGH** — **Use this pattern for our embedded DNS resolver and for anything else that needs a listener inside a container netns.** It is far better than the alternative (a helper process permanently living in the container netns), because: no extra process to supervise, no extra process to leak, the socket dies with the netns automatically, and our main event loop handles it like any other fd. Tradeoff: needs a dedicated OS thread for the setns dance and careful restore-on-panic. **Handoff to D2**: this forces "we must be able to run a closure on a pinned thread in another netns" into the runtime architecture.

### 1.6 Destruction semantics — and the veth peer question, settled

`[F]` **HIGH** — `network_namespaces(7)`, verbatim: *"When a network namespace is freed (i.e., when the last process in the namespace terminates), its physical network devices are moved back to the initial network namespace"* and *"When a namespace is freed, the `veth(4)` devices that it contains are destroyed."*
Source: <https://man7.org/linux/man-pages/man7/network_namespaces.7.html>

So the rule is asymmetric and it matters:
- **Physical** devices (a real NIC you moved in with `ip link set eth1 netns X`) **migrate back to the initial netns**. They survive.
- **Virtual** devices (veth, and by extension the whole class) **are destroyed**.

`[F]` **HIGH** — `veth(4)`: a veth pair is a single object with two ends. **Deleting either end deletes both.** *"If either device is moved to another network namespace, the tunnel keeps working... When one of the devices is deleted, the pair is deleted too."*
Source: <https://man7.org/linux/man-pages/man4/veth.4.html>

`[O]` **HIGH** — Composing the two: **when a container's netns dies, the host-side veth end disappears too, with no action from us.** Verified on `old-host`:
```
$ unshare -Urn sh -c '
    ip link add hostend type veth peer name childend
    unshare -n sleep 60 & CPID=$!
    ip link set childend netns $CPID          # exit 0 — same userns owns both
    echo ---PARENT_AFTER_MOVE---; ip -o link show
    echo ---CHILD---; nsenter -t $CPID -n ip -o link show
    kill $CPID; sleep 2
    echo ---PARENT_AFTER_CHILD_DEATH---; ip -o link show'

---PARENT_AFTER_MOVE---
1: lo: <LOOPBACK> ...
3: hostend@if2: <BROADCAST,MULTICAST> mtu 1500 ... link-netnsid 0
---CHILD---
1: lo: <LOOPBACK> ...
2: childend@if3: <BROADCAST,MULTICAST> mtu 1500 ... link-netnsid 0
---PARENT_AFTER_CHILD_DEATH---
1: lo: <LOOPBACK> ...            <-- hostend is GONE
```
`hostend` (ifindex 3, in the parent netns, enslaved to nothing) vanished entirely when the child netns was freed. **This is very good news for our cleanup story: veth leaks are self-healing.**

Two incidental details worth keeping:
- After the move, the peer is displayed as `hostend@if2` — the peer is identified by **ifindex in the other namespace**, not by name. The name is not preserved across the reference.
- Both sides report `link-netnsid 0`. These are **per-namespace-local ids**, independently assigned; "0" from the parent means the child, and "0" from the child means the parent. They are not a global identifier. `[O]` HIGH. On `host-a` the host sees `link-netnsid 0,1,2,3,4` across five containers, and each container sees `link-netnsid 0` for the host.

`[R]` **HIGH** — **Do not write cleanup code for the container-side veth, and do not write cleanup code for the host-side veth either.** Both are freed by the kernel when the netns is freed. What is *not* freed automatically, and therefore is what our cleanup must actually cover:
1. the **nsfs bind mount** (holds the netns alive forever — this is *the* leak),
2. **iptables/nft rules** referencing the container's IP,
3. the **IPAM allocation**,
4. the **bridge** itself (it has no owner and persists),
5. **`docker-proxy`-equivalent helper processes**.
See §10.

`[R]` **HIGH** — Corollary that inverts the usual instinct: **unmounting the nsfs bind mount is not just "cleaning up a file", it is the act that destroys the namespace and therefore garbage-collects the veth.** Our teardown's *first and most important* step is `umount2(path, MNT_DETACH)` + `unlink(path)`. If we get that right, most of the rest is best-effort.

---

## 2. veth pairs over rtnetlink

We intend to speak `NETLINK_ROUTE` directly rather than exec `ip`. This section assesses what that actually costs.

### 2.1 Socket and framing

`[F]` **HIGH** — `socket(AF_NETLINK, SOCK_RAW | SOCK_CLOEXEC, NETLINK_ROUTE)`. Then `bind()` a `struct sockaddr_nl { .nl_family = AF_NETLINK, .nl_pid = 0, .nl_groups = 0 }` — `nl_pid = 0` on bind asks the kernel to auto-assign a unique port id (do **not** hardcode `getpid()`; that breaks with threads and with two sockets in one process). Destination `nl_pid = 0` means "the kernel".
Source: `netlink(7)`, <https://man7.org/linux/man-pages/man7/netlink.7.html>

`[F]` **HIGH** — Every message starts with:
```c
struct nlmsghdr {
    __u32 nlmsg_len;    /* including this header */
    __u16 nlmsg_type;   /* RTM_NEWLINK etc. */
    __u16 nlmsg_flags;  /* NLM_F_REQUEST | NLM_F_ACK | ... */
    __u32 nlmsg_seq;
    __u32 nlmsg_pid;
};
```
Alignment is 4 bytes: `NLMSG_ALIGN(len) = ((len)+3) & ~3`. Payload attributes are TLVs:
```c
struct rtattr {
    unsigned short rta_len;   /* including this header */
    unsigned short rta_type;
};
#define RTA_ALIGNTO     4
#define RTA_LENGTH(len) (RTA_ALIGN(sizeof(struct rtattr)) + (len))
#define RTA_DATA(rta)   ((void*)(((char*)(rta)) + RTA_LENGTH(0)))
```
Nesting is just an `rtattr` whose payload is more `rtattr`s. Source: `rtnetlink(7)`, <https://man7.org/linux/man-pages/man7/rtnetlink.7.html>

`[F]` **HIGH** — **The encoding rule, exactly, because this is where hand-rolled codecs break.** From iproute2's `lib/libnetlink.c::addattr_l`:
```c
rta = (buf + NLMSG_ALIGN(nlh->nlmsg_len));
rta->rta_type = type;
rta->rta_len  = RTA_LENGTH(alen);          /* 4 + alen — UNALIGNED */
memcpy(RTA_DATA(rta), data, alen);
nlh->nlmsg_len = NLMSG_ALIGN(nlh->nlmsg_len) + RTA_ALIGN(rta->rta_len);
```
So: **`rta_len` holds the unaligned length, but the cursor advances by the aligned length, and the trailing pad bytes are counted in `nlmsg_len`.** Both `nlmsg_len` and `rta_len` are unaligned values; only the *positions* are aligned. Nesting works the same way: emit a header with `rta_len = RTA_LENGTH(0)`, remember the offset, append 4-aligned children, then back-patch `nest->rta_len = cursor - nest_offset`.
`[F]` **HIGH** — `struct nlattr` (`nla_len`/`nla_type`) is byte-identical to `struct rtattr`; treat them as one type. iproute2 does **not** set `NLA_F_NESTED (1<<15)` on container attributes (`IFLA_LINKINFO` goes out as bare type 18), and the kernel masks the flag off anyway — so matching iproute2 and omitting it is safest.
Struct sizes: `nlmsghdr` 16, `ifinfomsg` 16, `ifaddrmsg` 8, `rtmsg` 12 — all naturally 4-aligned, so no padding is needed between the family header and the first attribute.
Source: <https://git.kernel.org/pub/scm/network/iproute2/iproute2.git/plain/lib/libnetlink.c>

`[F]` **HIGH** — Request/ack protocol: set `NLM_F_REQUEST | NLM_F_ACK`. The kernel replies with `NLMSG_TYPE = NLMSG_ERROR` carrying `struct nlmsgerr { int error; struct nlmsghdr msg; }`. **`error == 0` means success** — the "ack" is an error message with errno 0. `error < 0` is `-errno`. This convention trips everyone up once.

`[F]` **HIGH** — **Turn on extended acks.** `setsockopt(fd, SOL_NETLINK /* 270 */, NETLINK_EXT_ACK /* 11 */, &one, 4)`. The kernel then sets `NLM_F_ACK_TLVS (0x200)` on the reply header and appends TLVs after the `nlmsgerr`:
```c
NLMSGERR_ATTR_MSG       = 1,  /* NUL-terminated human-readable string */
NLMSGERR_ATTR_OFFS      = 2,  /* u32 byte offset of the offending attribute */
NLMSGERR_ATTR_MISS_TYPE = 5,  /* u32 type of a missing required attribute */
```
The TLVs start at `sizeof(struct nlmsgerr)` if `NLM_F_CAPPED (0x100)` is set, else at `sizeof(struct nlmsgerr) + payload_len(err->msg)`.
Without it you get a bare `EINVAL`. With it you get `"The interface netns is immutable"` or `"An interface with the same name exists in the target netns"` — the actual answer, from the kernel, naming the attribute.
Source: `netlink(7)`; <https://git.kernel.org/pub/scm/linux/kernel/git/torvalds/linux.git/plain/include/uapi/linux/netlink.h>
`[R]` **HIGH** — **Enable `NETLINK_EXT_ACK` on day one.** Debugging hand-rolled netlink without it is genuinely miserable, and it is one `setsockopt`. This is the single highest-leverage line in our netlink code. Consider also `NETLINK_GET_STRICT_CHK (12)` for stricter kernel-side validation of our dump requests — it turns silently-ignored garbage into an error, which is what we want while developing.

`[F]` **HIGH** — Useful flag values: `NLM_F_REQUEST 0x01`, `NLM_F_MULTI 0x02`, `NLM_F_ACK 0x04`, `NLM_F_ECHO 0x08`, `NLM_F_REPLACE 0x100`, `NLM_F_EXCL 0x200`, `NLM_F_CREATE 0x400`, `NLM_F_APPEND 0x800`, `NLM_F_DUMP 0x300`. Idioms from the header's own comment: `CREATE|EXCL` = "add, fail if exists"; `CREATE|REPLACE` = "add or update"; `EXCL` alone = "check". Message types: `NLMSG_NOOP 1`, `NLMSG_ERROR 2`, `NLMSG_DONE 3`, `NLMSG_OVERRUN 4`, `NLMSG_MIN_TYPE 0x10`.
`[R]` **MED** — Use `NLM_F_ECHO` on creates: the kernel echoes the resulting object back, so we learn the new ifindex without a follow-up `RTM_GETLINK`. Saves a round trip and closes a TOCTOU window.

### 2.2 The veth-pair message — the one genuinely fiddly encoding

`[F]` **HIGH** — `ip link add veth0 type veth peer name veth1` is a single `RTM_NEWLINK` with `NLM_F_REQUEST|NLM_F_CREATE|NLM_F_EXCL|NLM_F_ACK`, payload `struct ifinfomsg` (all zeros for creation: `ifi_family=AF_UNSPEC`, `ifi_index=0`), then:
```
IFLA_IFNAME  = "veth0"
IFLA_LINKINFO (nested)
  └─ IFLA_INFO_KIND = "veth"
  └─ IFLA_INFO_DATA (nested)
       └─ VETH_INFO_PEER (nested)          [= 1, from <linux/veth.h>]
            ├─ struct ifinfomsg            <-- A SECOND, ZEROED ifinfomsg, INLINE
            └─ IFLA_IFNAME = "veth1"       <-- then normal attrs for the peer
```
The trap: **`VETH_INFO_PEER`'s payload is not a plain attribute list. It begins with a bare `struct ifinfomsg` struct, and only then does the attribute list start.** iproute2 zeroes that embedded struct and advances past it. In `ip/link_veth.c` (note: **not** `ip/iplink_veth.c` — that filename no longer exists in iproute2):
```c
data = addattr_nest(n, 1024, VETH_INFO_PEER);
n->nlmsg_len += sizeof(struct ifinfomsg);   /* reserve 16 zero bytes */
iplink_parse(argc - 1, argv + 1, (struct iplink_req *)n, &type);
peer_ifm = RTA_DATA(data);
peer_ifm->ifi_index  = ifm->ifi_index;
peer_ifm->ifi_flags  = ifm->ifi_flags;
peer_ifm->ifi_change = ifm->ifi_change;
addattr_nest_end(n, data);
```
The kernel side is explicit about the offset arithmetic — `net/core/rtnetlink.c`, `rtnl_nla_parse_ifinfomsg()`:
```c
attrs = nla_data(nla_peer) + sizeof(struct ifinfomsg);
len   = nla_len(nla_peer)  - sizeof(struct ifinfomsg);
return nla_parse_deprecated(tb, IFLA_MAX, attrs, len, ifla_policy, exterr);
```
and the policy enforces a minimum: `veth_policy[VETH_INFO_PEER] = { .len = sizeof(struct ifinfomsg) }`, so a `VETH_INFO_PEER` payload shorter than 16 bytes returns `-ERANGE`. Writing 16 zero bytes is sufficient and correct.
Attribute numbers: `IFLA_LINKINFO = 18`, `IFLA_INFO_KIND = 1`, `IFLA_INFO_DATA = 2`, `VETH_INFO_PEER = 1`.
Sources: <https://git.kernel.org/pub/scm/network/iproute2/iproute2.git/plain/ip/link_veth.c>, <https://git.kernel.org/pub/scm/linux/kernel/git/torvalds/linux.git/plain/net/core/rtnetlink.c>, <https://git.kernel.org/pub/scm/linux/kernel/git/torvalds/linux.git/plain/drivers/net/veth.c>

`[F]` **HIGH** — **The shortcut that removes a whole step: put `IFLA_NET_NS_FD` *inside* the `VETH_INFO_PEER` nest** and the peer is created directly in the container's netns — no separate move message at all. `veth_link_ops.peer_type = VETH_INFO_PEER` makes the generic rtnetlink path (`rtnl_get_peer_net()` → `rtnl_link_get_net_ifla(peer_tb)`) resolve the peer's netns *before* `veth_newlink()` runs. Attributes inside the nest (`IFLA_IFNAME`, `IFLA_MTU`, `IFLA_ADDRESS`, `IFLA_TXQLEN`) apply to the peer only; the same attributes at top level apply only to the near end. `IFF_UP` in the embedded `ifinfomsg` even brings the peer up at birth (`rtnl_configure_link(peer, ifmp, 0, NULL)`).
`[C]` **HIGH** — this is what current Docker/libnetwork uses (`veth.PeerNamespace`).
`[R]` **HIGH** — **Use this.** It collapses "create pair, look up peer ifindex, move peer, rename peer" into **one** message, and it eliminates the window in which the container-side veth exists in the host netns — which is both a race (another process could see or grab it) and a leak (if we die between create and move, a stray veth is left in the host netns; see §10). Strictly better on correctness *and* fewer lines. Tradeoff: none that I found.

`[O]` **MED** — Creation order detail worth knowing: on `old-host`, `ip link add v0 type veth peer name v1` produced `2: v1@v0` and `3: v0@v1` — **the peer gets the lower ifindex**. Don't assume the named device is created first. Also both ends start `<BROADCAST,MULTICAST,M-DOWN>`: DOWN, and `M-DOWN` because the peer is also down. A veth end only reports `LOWER_UP` when its peer is up — which is why a container's `eth0` shows `LOWERLAYERDOWN` if you forget to bring the host end up. `[O]` HIGH on that last point, seen in E12/E15.

`[R]` **HIGH** — **Bring up *both* ends. Forgetting the host end is the classic "container has an IP and a route and still can't ping the gateway" bug**, and it presents as `LOWERLAYERDOWN`/`NO-CARRIER`, not as an error from any command you ran.

### 2.3 Moving one end into the netns

`[F]` **HIGH** — `RTM_NEWLINK` (or `RTM_SETLINK`) with `ifinfomsg.ifi_index = <ifindex>` and one of:
- `IFLA_NET_NS_FD` (u32, an open fd to an nsfs file or `/proc/<pid>/ns/net`)
- `IFLA_NET_NS_PID` (u32, a pid)
- `IFLA_NET_NS_ID` (s32, a netnsid previously assigned via `RTM_NEWNSID`)

`[F]` **HIGH** — The capability gate, `net/core/rtnetlink.c` v6.11:
```c
net = rtnl_link_get_net_capable(skb, dev_net(dev), tb, CAP_NET_ADMIN);
...
err = __dev_change_net_namespace(dev, net, pat, new_ifindex);
```
and inside `rtnl_link_get_net_capable`:
```c
if (!netlink_ns_capable(skb, net->user_ns, cap)) {
    put_net(net);
    return ERR_PTR(-EPERM);
}
```
Source: <https://raw.githubusercontent.com/torvalds/linux/v6.11/net/core/rtnetlink.c>
So: **`CAP_NET_ADMIN` in the user namespace that owns the *target* netns.** That is the whole rootless story, and §8 has the empirical confirmation.

`[R]` **HIGH** — **Use `IFLA_NET_NS_FD`, not `IFLA_NET_NS_PID`.** Reasoning: a pid is a racy identifier — the container process can die and the pid be recycled between our `RTM_NEWLINK` and the kernel's lookup, and we would then move a veth into a *stranger's* netns. An fd is a stable reference to the namespace object itself; if the container dies, the netns stays alive because we hold the fd, and the operation is still correct. Cost: one `open()`. This is a correctness argument, not a style preference.

`[F]` **HIGH** — **Renaming during the move: `IFLA_IFNAME` serves two different purposes in the same message, and only one of them is unconditional.**

1. **Collision fallback.** `do_setlink()` passes `ifname` as the `pat` argument to `__dev_change_net_namespace()`, which uses it **only if the device's current name is already taken in the target netns**:
```c
if (netdev_name_in_use(net, dev->name)) {
    if (!pat) { NL_SET_ERR_MSG(extack,
        "An interface with the same name exists in the target netns"); goto out; }  /* -EEXIST */
    err = dev_prep_valid_name(net, dev, pat, new_name, EEXIST);
}
```
`pat` may contain `%d` and will be expanded. **No collision ⇒ the name is kept and `pat` is ignored.**
2. **Explicit rename**, later in the same `do_setlink()`:
```c
if (ifm->ifi_index > 0 && ifname[0]) { err = netif_change_name(dev, ifname); ... }
```
`[F]` **HIGH** — So the rename does happen in the same message, **but only if you identify the device by `ifi_index > 0`.** If you identify it by `IFLA_IFNAME` alone (with `ifi_index == 0`), the name is the *lookup key* and no rename occurs. This is a genuinely surprising asymmetry and it is why "I sent IFLA_IFNAME and it didn't rename" is a common complaint.
`[R]` **HIGH** — **Always identify links by `ifi_index`, never by name.** Two reasons that compound: it makes rename-on-move work, and names are not unique across netns while indexes are unambiguous within one. Capture the ifindex from the creation reply (use `NLM_F_ECHO` on the create, or read it from the ack) rather than doing a `RTM_GETLINK` lookup by name — that saves a round trip *and* closes a TOCTOU window.

`[F]` **MED** — Other move failure modes worth surfacing through `NETLINK_EXT_ACK`: `dev->netns_immutable` (formerly `NETIF_F_NETNS_LOCAL`) → `"The interface netns is immutable"`, which is why you can never move `lo` or many physical drivers; conflicting altnames → `-EEXIST`. `IFLA_NEW_IFINDEX` (49, s32) pins the ifindex in the target netns; otherwise the kernel reuses the old index if free and allocates a fresh one if not.

Attribute numbers: `IFLA_NET_NS_PID = 19`, `IFLA_NET_NS_FD = 28`, `IFLA_TARGET_NETNSID = 46`. They are **mutually exclusive** — the kernel rejects combinations.

### 2.4 Up, addresses, routes, bridge

`[F]` **HIGH** — **Bringing a link up** is `RTM_NEWLINK`/`RTM_SETLINK` with:
```c
ifi.ifi_index  = ifindex;
ifi.ifi_flags  = IFF_UP;      /* desired value of the bits */
ifi.ifi_change = IFF_UP;      /* MASK of which bits we are setting */
```
`ifi_change` is a **change mask**, and its zero case is the opposite of what you would guess. `net/core/rtnetlink.c`:
```c
static unsigned int rtnl_dev_combine_flags(const struct net_device *dev,
                                           const struct ifinfomsg *ifm)
{
	unsigned int flags = ifm->ifi_flags;

	/* bugwards compatibility: ifi_change == 0 is treated as ~0 */
	if (ifm->ifi_change)
		flags = (flags & ifm->ifi_change) |
		        (rtnl_dev_get_flags(dev) & ~ifm->ifi_change);
	return flags;
}
```
`[F]` **HIGH** — **`ifi_change == 0` is silently reinterpreted as `~0`, i.e. "replace the entire flag word".** So sending `flags = IFF_UP, change = 0` does **not** mean "set UP" — it means "set the flags to *exactly* `IFF_UP`", clearing `IFF_BROADCAST`, `IFF_MULTICAST`, `IFF_NOARP`, `IFF_PROMISC` and everything else on the device. That is a much nastier failure than a silent no-op: the link comes up and then multicast/broadcast is broken, so ARP and NDP quietly stop working and you debug it for an afternoon.
The gate for touching flags at all is `if (ifm->ifi_flags || ifm->ifi_change)`, so both-zero is the no-op. Consequently "bring the link **down**" cannot be expressed as `flags=0, change=0`; it must be `flags = 0, change = IFF_UP`.
`[R]` **HIGH** — **Always set `ifi_change` explicitly, to exactly the bits you are touching, on every single link message.** Up = `flags=IFF_UP, change=IFF_UP`. Down = `flags=0, change=IFF_UP`. Never leave the mask at its `Default::default()` — which in Rust is exactly the trap, because a zeroed struct is the dangerous case, not the safe one. Worth a newtype or a builder that makes the mask non-optional.
Source: <https://git.kernel.org/pub/scm/linux/kernel/git/torvalds/linux.git/plain/net/core/rtnetlink.c>

`[F]` **HIGH** — **Addresses**: `RTM_NEWADDR`, payload `struct ifaddrmsg { __u8 ifa_family; __u8 ifa_prefixlen; __u8 ifa_flags; __u8 ifa_scope; __u32 ifa_index; }`, plus:
- `IFA_LOCAL` — the local address
- `IFA_ADDRESS` — for broadcast interfaces, the same as `IFA_LOCAL`; for point-to-point, the *peer* address. Getting these confused on a ptp link is a classic bug.
- `IFA_BROADCAST` — optional, and **iproute2 does not send it unless you explicitly write `brd`**. `[F]` HIGH.
`[F]` **HIGH** — For IPv4 the kernel requires `IFA_LOCAL` and defaults `IFA_ADDRESS` to it, `net/ipv4/devinet.c`:
```c
if (!tb[IFA_LOCAL]) { NL_SET_ERR_MSG(extack, "ipv4: Local address is not supplied"); return -EINVAL; }
if (!tb[IFA_ADDRESS]) tb[IFA_ADDRESS] = tb[IFA_LOCAL];
```
So the complete, minimal wire form of `ip addr add 172.17.0.2/16 dev eth0` is:
```
RTM_NEWADDR, flags = NLM_F_REQUEST|NLM_F_ACK|NLM_F_CREATE|NLM_F_EXCL
ifaddrmsg { family=AF_INET, prefixlen=16, flags=0, scope=RT_SCOPE_UNIVERSE(0), index=<idx> }
  IFA_LOCAL(2)    4 bytes  172.17.0.2
  IFA_ADDRESS(1)  4 bytes  172.17.0.2
```
That is all. No `IFA_BROADCAST`, no `IFA_LABEL`, no `IFA_FLAGS`. `ifa_scope` comes from iproute2's `default_scope()`, which returns `RT_SCOPE_HOST(254)` only for `127.x.x.x` and `0` otherwise.
Sources: `rtnetlink(7)`; <https://git.kernel.org/pub/scm/linux/kernel/git/torvalds/linux.git/plain/net/ipv4/devinet.c>; <https://git.kernel.org/pub/scm/network/iproute2/iproute2.git/plain/ip/ipaddress.c>

`[F]` **HIGH** — **Adding an address auto-creates the connected route.** `RTM_NEWADDR` with prefixlen 16 gives you `172.17.0.0/16 dev eth0 proto kernel scope link src 172.17.0.2` for free. `[O]` HIGH — visible on `host-a` in every container: `172.18.0.0/16 dev eth0 proto kernel scope link src 172.18.0.4` with `proto kernel` marking it kernel-generated. **So we do not add the on-link route ourselves; only the default route.**

`[F]` **HIGH** — **Routes**: `RTM_NEWROUTE`, payload `struct rtmsg { rtm_family; rtm_dst_len; rtm_src_len; rtm_tos; rtm_table; rtm_protocol; rtm_scope; rtm_type; rtm_flags; }`. For `ip route add default via 172.18.0.1`:
```
rtm_family   = AF_INET
rtm_dst_len  = 0                 /* default route: prefix length zero  */
rtm_table    = RT_TABLE_MAIN (254)
rtm_protocol = RTPROT_BOOT (3)   /* what iproute2 uses when unspecified */
rtm_scope    = RT_SCOPE_UNIVERSE (0)
rtm_type     = RTN_UNICAST (1)
attrs: RTA_GATEWAY = 172.18.0.1
       (NO RTA_DST at all — absent, not zero-length)
       RTA_OIF = <ifindex>       /* optional if the gateway is unambiguous */
```
For an on-link/scope-link route: `rtm_scope = RT_SCOPE_LINK(253)`, `RTA_DST` set, `RTA_OIF` set, no `RTA_GATEWAY`.

`[F]` **HIGH** — **The scope rule, which is the part that silently bites:** iproute2 auto-selects `rtm_scope` for a unicast route based on whether a gateway was given (`ip/iproute.c`):
```c
else if (req.r.rtm_type == RTN_UNICAST || req.r.rtm_type == RTN_UNSPEC) {
    if (cmd == RTM_DELROUTE)              req.r.rtm_scope = RT_SCOPE_NOWHERE;
    else if (!gw_ok && !nhs_ok && !nhid)  req.r.rtm_scope = RT_SCOPE_LINK;
}
```
**Gateway present ⇒ `RT_SCOPE_UNIVERSE(0)`. No gateway ⇒ `RT_SCOPE_LINK(253)`.** Get this wrong and the kernel returns `EINVAL` or `ENETUNREACH` with no useful detail (unless ext-ack is on).

`[F]` **HIGH** — **Ordering constraint that will look like a bug:** `ip route add default via 172.18.0.1` sends **no `RTA_OIF`** — the kernel resolves the output device by looking the gateway up (`fib_check_nh`), which means **a scope-LINK route covering the gateway must already exist.** In our sequence that route comes for free from `RTM_NEWADDR` (§ above), so the order *must* be: address the interface, **then** add the default route. Reversed, the default route fails with `ENETUNREACH`. If you genuinely need the gateway before the prefix route, set `rtm_flags |= RTNH_F_ONLINK (4)` — that is what `ip route add … via X dev Y onlink` does.
Source: <https://git.kernel.org/pub/scm/network/iproute2/iproute2.git/plain/ip/iproute.c>

`[F]` **HIGH** — **Bridge creation**: `RTM_NEWLINK`, `IFLA_IFNAME="br0"`, `IFLA_LINKINFO{ IFLA_INFO_KIND="bridge" }`. Optional `IFLA_INFO_DATA{ IFLA_BR_* }` for STP (`IFLA_BR_STP_STATE=5`), ageing time (`4`), vlan filtering (`7`), etc.

`[F]` **HIGH** — **Enslaving**: plain `RTM_NEWLINK` (no `NLM_F_CREATE`) with `ifi_index = <veth host end>` and `IFLA_MASTER = <bridge ifindex>` (u32, attr **10**). Handled by `do_set_master()` → `ndo_add_slave`. **`IFLA_MASTER = 0` un-enslaves.**

`[F]` **HIGH** — **Per-port bridge options have two encodings, and the name everyone reaches for does not exist.** There is **no `IFLA_BRPORT_HAIRPIN_MODE` symbol.** Hairpin is `IFLA_BRPORT_MODE = 4`, a u8. Confirmed three ways:
- `net/bridge/br_netlink.c`: `br_set_port_flag(&set, &clear, tb, IFLA_BRPORT_MODE, BR_HAIRPIN_MODE);`
- `net/bridge/br_netlink.c`: `u8 mode = !!(flags & BR_HAIRPIN_MODE); nla_put_u8(skb, IFLA_BRPORT_MODE, mode);`
- `ip/iplink_bridge_slave.c`: `else if (matches(*argv, "hairpin") == 0) bridge_slave_parse_on_off("hairpin", *argv, n, IFLA_BRPORT_MODE);`

The two paths:
- **Path A** (what `ip link set … type bridge_slave` uses): `RTM_NEWLINK` on the port with `IFLA_LINKINFO{ IFLA_INFO_SLAVE_KIND(4)="bridge", IFLA_INFO_SLAVE_DATA(5){ IFLA_BRPORT_* } }`.
- **Path B** (what `bridge link set` uses): `RTM_SETLINK` with `ifi_family = AF_BRIDGE` and `IFLA_PROTINFO(12)` as the nest holding the same `IFLA_BRPORT_*` attributes.

Useful `IFLA_BRPORT_*` numbers: `STATE 1`, `PRIORITY 2`, `COST 3`, `MODE 4` (hairpin), `GUARD 5`, `PROTECT 6`, `FAST_LEAVE 7`, `LEARNING 8`, `UNICAST_FLOOD 9`, `PROXYARP 10`, `MCAST_FLOOD 27`, **`ISOLATED 33`**, `LOCKED 39`. All u8 booleans unless noted.
`[R]` **MED** — Use **Path A**; it is a normal `RTM_NEWLINK` and needs no `AF_BRIDGE` special-casing in our codec.
`[C]` **HIGH** — Docker sets hairpin on the host veth exactly when it is *not* using the userland proxy: `moby/daemon/libnetwork/drivers/bridge/bridge_linux.go` has `Hairpin: !config.EnableProxy`. That confirms §3.5's reading: hairpin and `docker-proxy` are two solutions to the same problem, and Docker picks one.

`[O]` **HIGH** — Confirmed the whole chain works with these primitives on `old-host`, inside an unprivileged userns netns:
```
$ unshare -Urn sh -c 'ip link add br0 type bridge && echo BRIDGE_OK;
    ip link add v0 type veth peer name v1;
    ip link set v0 master br0 && echo ENSLAVE_OK;
    ip addr add 10.99.0.1/24 dev br0 && echo ADDR_OK;
    ip link set br0 up && ip link set v0 up && echo UP_OK; ip -br addr show'
BRIDGE_OK / ENSLAVE_OK / ADDR_OK / UP_OK
lo    DOWN
br0   UNKNOWN  10.99.0.1/24
v1@v0 DOWN
v0@v1 LOWERLAYERDOWN
```
(`v0` is `LOWERLAYERDOWN` because `v1` is still down — the point from §2.2.)

### 2.5 MTU

`[F]` **HIGH** — `IFLA_MTU` (u32) on `RTM_NEWLINK`/`RTM_SETLINK`.

`[O]` **HIGH** — **veth does NOT inherit its bridge's MTU.** On `old-host`:
```
$ unshare -Urn sh -c 'ip link add br0 type bridge && ip link set br0 mtu 1400
    ip link add v0 type veth peer name v1
    ip link set v0 master br0; ip link set br0 up; ip link set v0 up; ip link set v1 up
    ip -d link show br0 | head -2; ip link show v0 | head -1; ip link show v1 | head -1'
2: br0: <...,UP,LOWER_UP> mtu 1400 ...
4: v0@v1: <...,UP,LOWER_UP> mtu 1500 ... master br0 ...
3: v1@v0: <...,UP,LOWER_UP> mtu 1500 ...
```
A 1500-MTU veth enslaved to a 1400-MTU bridge stays at 1500, on both ends. **The MTU must be set explicitly, on the bridge and on both veth ends.**

`[C]` **HIGH** — Docker exposes this as `com.docker.network.driver.mtu`. `[O]` on `host-a`: `docker network inspect bridge` → `"com.docker.network.driver.mtu": "1500"`, and every bridge, veth, and container `eth0` is 1500, matching `eth0`'s 1500.

**The classic MTU-mismatch failure, spelled out** `[F]` **HIGH**: if the host's uplink has a reduced MTU (1450 on many clouds, 1400 over WireGuard, 1476 over a GRE tunnel) but the bridge and veths are left at 1500, then:
- Small packets work. DNS works. `ping` works. TCP handshakes work.
- The connection then hangs the moment a full-size segment is sent — because the container negotiated MSS 1460 from its 1500 MTU, the host forwards a 1500-byte frame out a 1450-MTU interface, and the kernel either fragments it or sends `ICMP Fragmentation Needed`.
- Path MTU Discovery *should* fix it, **except** the ICMP "frag needed" has to get back to the container, and a huge fraction of the internet and of overzealous host firewalls drop ICMP type 3. So PMTUD blackholes.
- Symptom: "curl hangs at exactly the same byte count every time", "`apt update` works but `apt install` hangs", "TLS handshake completes then stalls on the certificate".
`[R]` **HIGH** — **Default the bridge MTU to the MTU of the interface carrying the host's default route, not to 1500.** Read it with `RTM_GETROUTE` for the default route → `RTA_OIF` → `RTM_GETLINK` → `IFLA_MTU`. Then apply it to the bridge and both veth ends. Also expose a per-network override. Tradeoff: the default route can change at runtime (WireGuard coming up) and we won't re-tune existing containers; accept that, document it, and add `--mtu`. This is ~20 lines and it prevents the single most-reported and least-diagnosable container networking bug.

### 2.6 Minimum message set, and the honest effort estimate

To do "create netns, veth pair, move one end in, address both, up both, default route inside, enslave host end to bridge":

| # | Op | Message | Key attributes |
|---|----|---------|----------------|
| 1 | create bridge (once per network) | `RTM_NEWLINK` | `IFLA_IFNAME`, `IFLA_LINKINFO{INFO_KIND="bridge"}`, `IFLA_MTU` |
| 2 | address the bridge (once) | `RTM_NEWADDR` | `ifaddrmsg`, `IFA_LOCAL`, `IFA_ADDRESS` |
| 3 | bridge up (once) | `RTM_NEWLINK` | `ifi_flags=IFF_UP`, `ifi_change=IFF_UP` |
| 4 | **create veth pair with peer already in the container netns** | `RTM_NEWLINK` | `IFLA_IFNAME`, `IFLA_MTU`, `IFLA_LINKINFO{"veth", INFO_DATA{VETH_INFO_PEER{ifinfomsg(0), IFLA_IFNAME="eth0", IFLA_MTU, IFLA_NET_NS_FD}}}` — **replaces the old create+lookup+move+rename sequence** |
| 5 | enslave host end | `RTM_NEWLINK` | `ifi_index`, `IFLA_MASTER` |
| 6 | host end up | `RTM_NEWLINK` | `IFF_UP` / change mask |
| 7 | *(setns into container)* — lo up | `RTM_NEWLINK` | `IFF_UP` |
| 8 | *(in container)* address eth0 | `RTM_NEWADDR` | `IFA_LOCAL`, `IFA_ADDRESS`, prefixlen |
| 9 | *(in container)* eth0 up | `RTM_NEWLINK` | `IFF_UP` |
| 10 | *(in container)* default route | `RTM_NEWROUTE` | `rtm_dst_len=0`, `RTA_GATEWAY`, scope UNIVERSE — **after** op 8 |

**Distinct message types: 3 for the happy path** (`RTM_NEWLINK`, `RTM_NEWADDR`, `RTM_NEWROUTE`), plus `RTM_GETLINK`/`RTM_GETADDR`/`RTM_GETROUTE` dumps and `RTM_DELLINK`/`RTM_DELADDR` for reconciliation and teardown. **Distinct attributes: ~14.** **Payload structs: 3** (`ifinfomsg`, `ifaddrmsg`, `rtmsg`).
Note `RTM_SETLINK` is not needed at all — iproute2 itself uses `RTM_NEWLINK` without `NLM_F_CREATE` for every `ip link set`, and so should we; one fewer code path.
Also note netns creation is **not netlink**: it is `unshare(CLONE_NEWNET)` / `setns(2)` plus a bind mount. `RTM_NEWNSID` only *labels* an existing netns with an nsid; it does not create one.

`[R]` **HIGH** — **Honest estimate: a weekend for the happy path, ~a week to be solid.** The genuinely hard parts are exactly four, and they are all "get it wrong once, then never again":
1. `rta_len` is **unaligned** but the write cursor advances by the **aligned** amount, and the pad bytes count toward `nlmsg_len`. This is where hand-rolled encoders break first.
2. The embedded `ifinfomsg` inside `VETH_INFO_PEER`.
3. `ifi_change`, whose zero value is the dangerous one.
4. **A netlink socket is permanently bound to the netns it was *created* in.** So you need one socket per namespace, opened *after* `setns`, and since `setns` is per-thread that means a dedicated OS thread. This is an architectural consequence, not a detail → **Handoff to D2.**

The long tail beyond that week: sequence/port demultiplexing if concurrent requests share a socket, `ENOBUFS` and `NLM_F_DUMP_INTR` restart on dumps, and mapping `EEXIST`/`EBUSY`/`EOPNOTSUPP`/`EPERM` into errors a caller can act on. None is hard; all of it is necessary before we trust it.

`[R]` **HIGH** — **Write it ourselves.** Reasoning: (1) it is a weekend, not a month; (2) it is the *learning* part of the project — netlink is the actual interface between a container engine and the kernel's networking, and abstracting it away on day one means never understanding what we built; (3) the alternative libraries all drag in tokio (§12); (4) the message set is tiny and *frozen* — `RTM_NEWLINK` has not changed in twenty years, so there is no maintenance treadmill. Tradeoff: we own the bugs, and hand-rolled TLV encoding in Rust wants a tiny well-tested builder (`push_attr(ty, bytes)` / `nest(ty, |b| …)`) plus round-trip tests against a real kernel. Budget for that builder; it is the whole abstraction we need.

`[R]` **HIGH** — **Do not shell out to `ip`.** This is a trap, not a shortcut. Specific reasons, in order of how much they will hurt: (1) `ip` reports failures as English on stderr with exit code 1/2/255, so we cannot distinguish "already exists" from "permission denied" from "no such device" without parsing prose — and *idempotent, resumable* setup (§10) depends entirely on telling those apart; (2) `ip link set X netns <pid>` takes a pid, and we established in §2.3 that pids are racy; there is no way to pass an fd through the CLI; (3) `ip netns exec` requires the `/var/run/netns` convention we deliberately are not using (§1.2); (4) `[O]` MED — behaviour differs across versions: on `old-host`'s iproute2, `ip link set v1 netns /proc/<pid>/ns/net` failed with `Error: argument "/proc/1074772/ns/net" is wrong: Invalid "netns" value` while the pid form was accepted, so even the argument grammar is not stable; (5) fork+exec per operation, ~12 execs per container start. The one legitimate use of `ip` is as an oracle while developing — run `ip -d link show` to check our work, and `strace -e sendmsg ip link add ...` to compare our bytes against iproute2's.

---

## 3. The bridge model

### 3.1 What a Linux bridge actually is

`[F]` **HIGH** — A `bridge` device is an **in-kernel software IEEE 802.1D learning Ethernet switch**. It maintains a forwarding database (FDB) mapping MAC → port, learned from source addresses of received frames, aged out after `ageing_time` (default 300s; Docker leaves it at 30000 centiseconds = 300s). Frames to a known unicast MAC go to that one port; unknown-unicast, broadcast, and multicast are flooded to all forwarding ports. It operates at L2 and does not need an IP address to switch.
Source: <https://www.kernel.org/doc/html/latest/networking/bridge.html>, `ip-link(8)` bridge section.

`[O]` **HIGH** — Live on `host-a`, the FDB shows exactly this, learned vs static:
```
$ bridge fdb show br br-9d68a0cc943b
9a:82:0c:4d:af:e1 dev br-9d68a0cc943b vlan 1 master br-9d68a0cc943b permanent   <- bridge's own MAC
a2:80:cf:ef:e6:c2 dev veth1aa0cfa vlan 1 master br-9d68a0cc943b permanent       <- the veth's own MAC
6e:ee:20:71:68:37 dev veth1aa0cfa master br-9d68a0cc943b                        <- LEARNED: container MAC
```
No `permanent` ⇒ dynamically learned. And `ip -d link show br-9d68a0cc943b` reports `fdb_n_learned 3` — three container MACs learned across five ports.

`[F]` **HIGH** — Giving the bridge an IP address turns it into **a switch plus a router interface on that switch**. This is the key conceptual move: `172.18.0.1/16` on `br-9d68a0cc943b` means (a) the host has an interface on the container L2 segment, (b) the kernel installs a connected route `172.18.0.0/16 dev br-…`, and (c) the address can serve as the containers' default gateway. Everything else follows from ordinary IP routing.

`[O]` **HIGH** — On `host-a`, the three bridges and their state:
```
4:  docker0            172.17.0.1/16   <NO-CARRIER,...,UP> state DOWN   (route: linkdown)
26: br-0d4cf39c58f7    172.19.0.1/16   <NO-CARRIER,...,UP> state DOWN   (route: linkdown)
67: br-9d68a0cc943b    172.18.0.1/16   <BROADCAST,...,UP,LOWER_UP> UP   5 veth ports
```
`docker0` exists with an address and a route but is `NO-CARRIER` / `state DOWN` with `linkdown` on its route — **because it has no enslaved ports.** A bridge with zero ports has no carrier. It is administratively UP but operationally down, and the connected route is marked `linkdown`. This is normal, not a fault, and it is why `docker0` looks broken on hosts where nobody uses the default bridge.

`[O]` **HIGH** — Bridge defaults Docker relies on, from `ip -d link show`: `stp_state 0` (STP **off**), `forward_delay 1500` (irrelevant with STP off), `vlan_filtering 0`, `mcast_snooping 1`, `nf_call_iptables 0` **(per-bridge!)**, `ageing_time 30000`.
`[R]` **HIGH** — **Leave STP off.** A container bridge is a star with no possible loop; STP's only effect would be a 15–30s forwarding delay on every single container start, during which the container has a link but no connectivity. This is a well-known Kubernetes/CNI gotcha. Zero benefit, large cost.

### 3.2 Why bridge + NAT and not routed or macvlan

`[F]/[R]` **HIGH** — The four realistic topologies and the actual reason bridge+NAT won:

| Model | Container gets | Needs | Fails when |
|---|---|---|---|
| **bridge + NAT** | private RFC1918 addr | nothing from the network | inbound requires explicit DNAT; conntrack table pressure |
| **routed bridge** (no NAT) | private addr, globally routed | the upstream router must have a route to your /16 via your host | you don't control the router — i.e. almost always |
| **macvlan / ipvlan** | address on the *host's* L2 | a free address on the LAN per container, DHCP or IPAM cooperation, and the switch must tolerate many MACs per port | cloud NICs (port security / MAC filtering drop it), Wi-Fi (802.11 has no multi-MAC), address exhaustion |
| **host netns** | the host's addresses | nothing | zero isolation, port collisions |

The decisive property of bridge+NAT is **it requires nothing whatsoever from the network the host sits on.** `docker run` works identically on a laptop behind Wi-Fi NAT, a cloud VM, a CI runner, and inside another container. Every other model requires cooperation from infrastructure the user does not control. That, not performance or elegance, is why it is the default.
`[R]` **HIGH** — **bridge + NAT is our only stage-4 model.** Ship macvlan never, or much later, and only as an explicit opt-in driver.

`[O]` **HIGH** — The single rule that makes egress work, live on `host-a`:
```
$ iptables -t nat -S POSTROUTING
-A POSTROUTING -s 172.18.0.0/16 ! -o br-9d68a0cc943b -j MASQUERADE
```
Read as: *any packet whose source is in the container subnet and which is leaving via any interface other than the bridge itself gets its source address rewritten to the address of the outgoing interface.* The `! -o <bridge>` exclusion is essential — without it, container-to-container traffic on the same bridge would also be masqueraded, so every container would see every peer as coming from `172.18.0.1` and container-to-container source IPs would be destroyed.

**Why this rule is load-bearing** `[F]` **HIGH**: the rest of the internet has no route to `172.18.0.0/16` (it is RFC 1918 private space — <https://www.rfc-editor.org/rfc/rfc1918>). A container's SYN would arrive at the destination with source `172.18.0.4`, and the SYN-ACK would be dropped or blackholed because there is no path back. MASQUERADE rewrites the source to the host's public `198.51.100.10`, conntrack remembers the mapping, and the reply is un-rewritten on the way back in. **Without this one rule, containers can send packets and never receive a reply — the failure looks like "DNS times out, everything times out", not like a permission error.**

`[O]` **HIGH** — The counters prove it is carrying the traffic: `ip saddr 172.18.0.0/16 oifname != "br-9d68a0cc943b" counter packets 51369 bytes 3378872 masquerade`.

`[F]` **MED** — `MASQUERADE` vs `SNAT`: `MASQUERADE` looks up the outgoing interface's primary address *per connection*, so it survives a changing host address (DHCP, failover); `SNAT --to-source X` is static and marginally cheaper. Docker uses MASQUERADE.
`[R]` **HIGH** — Use `MASQUERADE`. On a laptop or a DHCP cloud VM, static SNAT breaks the moment the address changes, and we have no code watching for that.

### 3.3 `net.ipv4.ip_forward` — required, and what breaks without it

`[F]` **HIGH** — `ip-sysctl.txt`: `ip_forward`, *"0 - disabled (default); not 0 - enabled. Forward Packets between interfaces."* A packet arriving on the bridge destined for a non-local address is **dropped in the routing decision** if forwarding is off — before `filter/FORWARD`, before any of our rules. It is not a firewall drop; the kernel simply refuses to be a router.

`[O]` **HIGH** — On `host-a`: `net.ipv4.ip_forward = 1`, `net.ipv4.conf.all.forwarding = 1`, and `net.ipv6.conf.all.forwarding = 0`.

**Symptom without it** `[F]` **HIGH**: container-to-container on the same bridge **still works** (that is L2 switching, no routing involved), but everything else dies — egress, published ports, container-to-host-other-interface. So the failure mode is "containers can talk to each other but nothing reaches the internet", which people misdiagnose as DNS.

`[R]` **HIGH** — Read it, and only write if 0. Combined with §1.1's warning: **`ip_forward` must be the first sysctl we touch**, because writing it resets per-interface config. And note we must set it in the **host** netns (that is where forwarding between bridge and uplink happens), which means this is a genuinely global, persistent host mutation → §10.

`[R]` **MED** — Docker 28 added `ip-forward-no-drop` and pairs enabling forwarding with setting `filter/FORWARD` policy to DROP, precisely because flipping a host into a router silently is a security change the admin did not ask for. **We should do the same: if we enable `ip_forward` ourselves, we must also install a default-deny on FORWARD, or we have just made the user's host a promiscuous router.** Tradeoff: setting `FORWARD` policy to DROP can break an existing router/VPN setup on that host, which is why Docker made it opt-out-able. Log loudly when we change either.

### 3.4 `bridge-nf-call-iptables` — what it changes, and why it is contentious

`[F]` **HIGH** — Normally, frames **switched** by a bridge between two of its own ports never enter the IP layer, so `iptables` (an L3 hook set) never sees them. The `br_netfilter` module changes this: with `net.bridge.bridge-nf-call-iptables=1`, bridged frames are pushed through the **`iptables` `FORWARD`/`PREROUTING`/`POSTROUTING` hooks** as if they were routed.
Source: <https://www.kernel.org/doc/html/latest/networking/bridge.html> ("Bridge + netfilter"), and `br_netfilter` docs.

`[O]` **HIGH** — On `host-a`, *both* knobs exist and disagree, which is the whole story:
```
$ sysctl net.bridge.bridge-nf-call-iptables
net.bridge.bridge-nf-call-iptables = 1                 <- GLOBAL: on
$ ip -d link show br-9d68a0cc943b | grep -o 'nf_call_iptables [0-9]'
nf_call_iptables 0                                      <- PER-BRIDGE: off
```
There are two independent controls: the global `br_netfilter` sysctl and a per-bridge `IFLA_BR_NF_CALL_IPTABLES` flag. Docker leaves the per-bridge flag at 0 on its own bridges.

**Why we need it at all** `[F]` **HIGH**: `--icc=false` (blocking container↔container on the same bridge) is an L2-switched path. Without `br_netfilter`, no iptables rule can ever see that traffic, so `--icc=false` is unenforceable in iptables. Same for any per-container L3 policy on intra-bridge traffic. This is the legitimate use.

**Why it is contentious** `[F]/[C]` **MED–HIGH**:
1. **It is global and it is a module.** Loading `br_netfilter` changes packet processing for **every bridge on the host**, including libvirt's, including the user's. A container engine flipping it is a system-wide behaviour change.
2. **It breaks conntrack assumptions.** Bridged frames now create conntrack entries. On a busy L2 segment this inflates the conntrack table, and asymmetric bridged traffic produces `INVALID` state entries that a `-m conntrack --ctstate INVALID -j DROP` rule (which `ufw` has by default, `[O]` seen on `host-a`: `-A ufw-before-input -m conntrack --ctstate INVALID -j DROP`) will then silently eat.
3. **The historic Kubernetes footgun.** kube-proxy *requires* `bridge-nf-call-iptables=1` for Service traffic from a pod to its own node to work; the widely-copied `sysctl net.bridge.bridge-nf-call-iptables=1` in every k8s install guide is why everyone thinks it's mandatory. It is not — it is mandatory *for kube-proxy's design*.
4. **It confuses the layering.** People expect a bridge to be a switch. Making iptables filter L2 forwarding means a rule with `-i eth0 -o eth1` semantics now applies to same-segment traffic, which surprises everyone once.

`[R]` **HIGH** — **Do not load or enable `br_netfilter`, and do not implement `--icc=false` in the MVP.** Reasoning: it is a global host mutation with system-wide side effects, in exchange for a feature (intra-bridge isolation) that has a strictly better answer: `IFLA_BRPORT_ISOLATED`. Setting the bridge-port `isolated` flag on every container's veth makes the *kernel bridge itself* refuse to forward between isolated ports — no netfilter, no conntrack, no global sysctl, no module. `[O]` HIGH: `ip -d link show` on `host-a`'s veths reports `isolated off`, i.e. the flag exists and is exposed on this kernel. `[R]` **MED** — when we do want `--icc=false`, implement it as `IFLA_BRPORT_ISOLATED=1` per port, not as iptables rules. Tradeoff: `isolated` is all-or-nothing per port (no "allow these two containers to talk"), whereas iptables could express a policy. Accept that; anyone needing real policy should use separate networks.

### 3.5 Container-side configuration, ARP, and hairpin

`[O]` **HIGH** — The complete container-side state on `host-a` (nginx, pid 2588141, ip `172.18.0.4`):
```
$ nsenter -t 2588141 -n ip addr
1: lo:  inet 127.0.0.1/8 scope host lo
2: eth0@if162: <BROADCAST,MULTICAST,UP,LOWER_UP> mtu 1500 link-netnsid 0
    link/ether ee:47:c6:16:34:61
    inet 172.18.0.4/16 brd 172.18.255.255 scope global eth0
$ nsenter -t 2588141 -n ip route
default via 172.18.0.1 dev eth0
172.18.0.0/16 dev eth0 proto kernel scope link src 172.18.0.4
$ nsenter -t 2588141 -n ip neigh
172.18.0.1 dev eth0 lladdr 9a:82:0c:4d:af:e1 STALE     <- the bridge's own MAC
172.18.0.6 dev eth0 lladdr ca:be:0b:2b:4b:76 STALE     <- a peer container
172.18.0.3 dev eth0 lladdr 32:fa:e6:8a:a4:eb STALE
```
That is the *entire* container network configuration. Three items: an address with a prefix, a kernel-generated connected route, and a default route. Nothing else.

`[F]` **HIGH** — **ARP/neighbour behaviour is completely ordinary and requires nothing from us.** The container ARPs for `172.18.0.1`; the request is broadcast, floods the bridge, and the *bridge device itself* answers because it owns that address. For a peer container, the request floods, the peer answers, and the bridge learns both MACs. No proxy ARP, no static neighbour entries, no `IFLA_BRPORT_PROXYARP` needed. `[O]` confirms: the container's neighbour table holds the bridge's real MAC `9a:82:0c:4d:af:e1`, matching `ip link show br-9d68a0cc943b`.

`[F]` **HIGH** — **Hairpin mode** (`IFLA_BRPORT_HAIRPIN_MODE`, `ip link set <port> type bridge_slave hairpin on`): by default a bridge will **never** send a frame back out the port it arrived on (802.1D). Hairpin disables that check for a port.
`[O]` **HIGH** — On `host-a`, every container veth reports `hairpin off`.

**When hairpin is needed** `[F]/[C]` **MED** — the case is a container reaching *its own* published port via the host's address. Packet path: container → bridge → host routing → `nat/PREROUTING` DNAT to `172.18.0.4:80` → routing decides the output interface is the same bridge → the frame must go back out the port it came in on → **blocked without hairpin**. Docker's answer historically was not hairpin but `docker-proxy`: the proxy's IP-layer connection sidesteps the L2 restriction entirely. With `userland-proxy=false`, Docker enables hairpin plus a MASQUERADE rule for the bridge-to-itself case.
`[R]` **MED** — **Skip hairpin in the MVP.** "Container connects to its own published port on the host IP" is a rare, confused thing for a container to do (it should use the peer's name or `localhost`). Add it when someone reports it, as one `IFLA_BRPORT_HAIRPIN_MODE` attribute. Note this interacts with the `docker-proxy` decision (§4.4).

---

## 4. The full `docker run -p 8080:80 nginx` trace

This is the central trace. Everything below is grounded in the **live** ruleset on `host-a`, where ports 80 and 443 are genuinely published to a container at `172.18.0.4` and four `docker-proxy` processes are running.

### 4.1 The live ruleset

`[O]` **HIGH** — `iptables -t nat -S` on `host-a` (Docker 28.3.1), verbatim:
```
-P PREROUTING ACCEPT
-P POSTROUTING ACCEPT
-N DOCKER
-A PREROUTING  -m addrtype --dst-type LOCAL -j DOCKER
-A OUTPUT ! -d 127.0.0.0/8 -m addrtype --dst-type LOCAL -j DOCKER
-A POSTROUTING -s 172.18.0.0/16 ! -o br-9d68a0cc943b -j MASQUERADE
-A POSTROUTING -s 172.17.0.0/16 ! -o docker0          -j MASQUERADE
-A POSTROUTING -s 172.19.0.0/16 ! -o br-0d4cf39c58f7  -j MASQUERADE
-A DOCKER -i br-9d68a0cc943b -j RETURN
-A DOCKER -i docker0 -j RETURN
-A DOCKER ! -i br-9d68a0cc943b -p tcp -m tcp --dport 80  -j DNAT --to-destination 172.18.0.4:80
-A DOCKER ! -i br-9d68a0cc943b -p tcp -m tcp --dport 443 -j DNAT --to-destination 172.18.0.4:443
```
`[O]` **HIGH** — `iptables -t filter -S`, Docker's chains only (the host also runs `ufw`; see §4.6):
```
-P FORWARD DROP
-A FORWARD -j DOCKER-USER
-A FORWARD -j DOCKER-FORWARD
-A FORWARD -j ufw-before-logging-forward
... (ufw chains follow) ...

-A DOCKER-FORWARD -j DOCKER-CT
-A DOCKER-FORWARD -j DOCKER-ISOLATION-STAGE-1
-A DOCKER-FORWARD -j DOCKER-BRIDGE
-A DOCKER-FORWARD -i docker0 -j ACCEPT
-A DOCKER-FORWARD -i br-9d68a0cc943b -j ACCEPT

-A DOCKER-CT -o docker0          -m conntrack --ctstate RELATED,ESTABLISHED -j ACCEPT
-A DOCKER-CT -o br-9d68a0cc943b  -m conntrack --ctstate RELATED,ESTABLISHED -j ACCEPT

-A DOCKER-BRIDGE -o docker0         -j DOCKER
-A DOCKER-BRIDGE -o br-9d68a0cc943b -j DOCKER

-A DOCKER -d 172.18.0.4/32 ! -i br-9d68a0cc943b -o br-9d68a0cc943b -p tcp --dport 443 -j ACCEPT
-A DOCKER -d 172.18.0.4/32 ! -i br-9d68a0cc943b -o br-9d68a0cc943b -p tcp --dport 80  -j ACCEPT
-A DOCKER ! -i docker0          -o docker0          -j DROP
-A DOCKER ! -i br-9d68a0cc943b  -o br-9d68a0cc943b  -j DROP

-A DOCKER-ISOLATION-STAGE-1 -i docker0         ! -o docker0         -j DOCKER-ISOLATION-STAGE-2
-A DOCKER-ISOLATION-STAGE-1 -i br-9d68a0cc943b ! -o br-9d68a0cc943b -j DOCKER-ISOLATION-STAGE-2
-A DOCKER-ISOLATION-STAGE-2 -o br-9d68a0cc943b -j DROP
-A DOCKER-ISOLATION-STAGE-2 -o docker0         -j DROP
```
`[O]` **HIGH** — and the one most people have never seen, `iptables -t raw -S`:
```
-A PREROUTING -d 172.19.0.2/32 ! -i br-0d4cf39c58f7 -j DROP
-A PREROUTING -d 172.19.0.3/32 ! -i br-0d4cf39c58f7 -j DROP
-A PREROUTING -d 172.18.0.2/32 ! -i br-9d68a0cc943b -j DROP
-A PREROUTING -d 172.18.0.3/32 ! -i br-9d68a0cc943b -j DROP
-A PREROUTING -d 172.18.0.4/32 ! -i br-9d68a0cc943b -j DROP
-A PREROUTING -d 172.18.0.5/32 ! -i br-9d68a0cc943b -j DROP
-A PREROUTING -d 172.18.0.6/32 ! -i br-9d68a0cc943b -j DROP
```
One rule **per container IP** — this is Docker 28's anti-direct-routing hardening; see §4.5.

### 4.2 Inbound: the packet, hop by hop

A SYN arrives on `eth0` for `198.51.100.10:80`.

**1. `raw/PREROUTING`** (netfilter priority `raw` = -300, the first IPv4 hook).
Checked against the seven per-container DROP rules. Destination is the host's public address, not a container IP, so no match. **Passes.**
(If someone on the LAN had a route to `172.18.0.0/16` and sent a packet directly to `172.18.0.4:3306`, *this* is where it dies.)

**2. conntrack** (`nf_conntrack_in`, priority -200). New flow ⇒ state `NEW`, a tuple `(tcp, 203.0.113.9:54321 → 198.51.100.10:80)` is created but not yet confirmed.

**3. `nat/PREROUTING`** (priority `dstnat` = -100).
```
-A PREROUTING -m addrtype --dst-type LOCAL -j DOCKER
```
`--dst-type LOCAL` means "the destination address is configured on this host" — it matches for `198.51.100.10`, and equally for `127.0.0.1` and for `172.18.0.1`. It is a cheap way to say "traffic aimed at us" without enumerating addresses. In nft this compiles to `fib daddr type local`. Jump to `DOCKER`.

Inside `DOCKER`:
- `-A DOCKER -i br-9d68a0cc943b -j RETURN` — **not** taken; we came in on `eth0`. This early-RETURN per bridge is how Docker prevents traffic *originating in a container* from being DNAT'd by another container's publish rule.
- `-A DOCKER ! -i br-9d68a0cc943b -p tcp --dport 80 -j DNAT --to-destination 172.18.0.4:80` — **MATCH.**

**Note what this rule does *not* say: there is no `-d`.** It matches port 80 arriving on *any* interface that isn't the bridge. So the publish is genuinely "all host addresses", including `10.10.0.5` (the private VPC address) and `10.116.0.2` (the second NIC) — a broader exposure than most people picture when they type `-p 80:80`. `[O]` HIGH — verified: the rule text has no destination match.

DNAT rewrites the destination to `172.18.0.4:80` and records the translation in the conntrack entry. `[F]` **HIGH** — a DNAT in `PREROUTING` applies **only to the first packet of a flow**; every subsequent packet is translated by conntrack directly, never re-traversing the nat table. That is why `nat` rules are cheap and why the DNAT counters (`11878` packets on :80) are far lower than the byte counts would suggest for a busy web server — they count *connections*, roughly, not packets.

**4. Routing decision.** Destination is now `172.18.0.4`. `ip route` gives `172.18.0.0/16 dev br-9d68a0cc943b proto kernel scope link src 172.18.0.1`. Not a local address ⇒ **this must be forwarded**, which requires `net.ipv4.ip_forward=1` (`[O]`: it is 1). Output interface: `br-9d68a0cc943b`.

**5. `filter/FORWARD`.** Policy is `DROP` (`[O]`: `-P FORWARD DROP`), so the packet must be explicitly accepted.
- `-A FORWARD -j DOCKER-USER` — empty on this host, falls through. **This is the user's hook and it is first for a reason** (§4.6).
- `-A FORWARD -j DOCKER-FORWARD`:
  - `-j DOCKER-CT`: `-o br-… -m conntrack --ctstate RELATED,ESTABLISHED -j ACCEPT`. Our packet is `NEW`, so no match. (This chain exists as a fast path: for every packet after the first, the flow is ESTABLISHED and gets accepted here, skipping all the per-container rules below. A real optimisation once you have hundreds of published ports.)
  - `-j DOCKER-ISOLATION-STAGE-1`: matches on `-i <bridge> ! -o <same bridge>`, i.e. traffic *leaving* a Docker bridge for elsewhere. We are *entering* a bridge from `eth0`, so no match.
  - `-j DOCKER-BRIDGE`: `-o br-9d68a0cc943b -j DOCKER`. Match. Jump to `filter/DOCKER`:
    - `-A DOCKER -d 172.18.0.4/32 ! -i br-9d68a0cc943b -o br-9d68a0cc943b -p tcp --dport 80 -j ACCEPT` — **MATCH. ACCEPTED.**
    Note this rule is far more specific than the nat one: it pins the destination IP *and* the input-interface-is-not-the-bridge *and* the output-interface-is-the-bridge. It is the actual authorization; the DNAT rule was only the translation.
    - Had it not matched, `-A DOCKER ! -i br-9d68a0cc943b -o br-9d68a0cc943b -j DROP` is the catch-all: **anything from outside aimed at this bridge that isn't an explicitly published port is dropped.** This is the "unpublished ports are not reachable" guarantee at L3.
- `ufw`'s FORWARD chains are **never reached** for this packet, because `DOCKER-FORWARD` already ACCEPTed. §4.6.

**6. `nat/POSTROUTING`.** `-s 172.18.0.0/16` — our source is still the *client's* `203.0.113.9`, not a container address, so the MASQUERADE rules do not match. **Important and frequently misunderstood: inbound published-port traffic is not masqueraded. The container sees the real client IP.** `[O]` HIGH — derivable directly from the rule's `-s` match.

**7. The bridge.** Frame is built with destination MAC `ee:47:c6:16:34:61` (from the host's neighbour table for `172.18.0.4`), sent out `br-9d68a0cc943b`. The bridge looks up the FDB, finds that MAC on port `veth2b1ca76`, forwards it.

**8. veth → container.** `veth2b1ca76` (host ifindex 162) transmits; its peer `eth0` (container ifindex 2) receives. `[F]` HIGH — a veth transmit is a direct `netif_rx` into the peer in the same softirq context; there is no queueing discipline, no copy of the payload, just an skb handed across. This is why veth is fast.

**9. Inside the container netns.** `eth0` receives, `172.18.0.4` is local, the container's own netfilter hooks run (the container has its own independent `nat` table — `[O]`: the DOCKER_OUTPUT/DOCKER_POSTROUTING DNS rules, §6), and nginx's listener on `0.0.0.0:80` accepts. `[O]` HIGH: `nsenter -t 2588141 -n ss -ltnp` → `LISTEN 0.0.0.0:80 users:(("nginx",pid=2588141,fd=6))`.

### 4.3 The reverse path

nginx replies: `172.18.0.4:80 → 203.0.113.9:54321`.

1. Container routing: default via `172.18.0.1` ⇒ out `eth0` ⇒ veth ⇒ bridge ⇒ host.
2. Host `raw/PREROUTING`: source is a container IP, not destination — the anti-spoof rules match on `-d`, so no match.
3. Host conntrack: the tuple is recognized as the **reply direction** of the existing flow. The recorded DNAT is **reversed**: source is rewritten from `172.18.0.4:80` back to `198.51.100.10:80`. **This happens in conntrack, not by any rule** — there is no `SNAT` rule for this and there does not need to be one. This is the piece people look for in `iptables-save` and cannot find.
4. `filter/FORWARD`: `DOCKER-CT`'s `-o br-… ESTABLISHED` rule doesn't match (we're going *out* `eth0`, not into a bridge). `DOCKER-FORWARD`'s `-i br-9d68a0cc943b -j ACCEPT` **does** — that final catch-all accepts anything originating from a Docker bridge.
5. `nat/POSTROUTING`: `-s 172.18.0.0/16 ! -o br-… -j MASQUERADE` — the source has *already* been un-DNAT'd to `198.51.100.10` by conntrack before POSTROUTING, so this does not match. Correct: we must not double-translate.
6. Out `eth0`. The client sees a reply from the address it contacted. `[R]` — this symmetry is entirely conntrack's doing; **if conntrack is disabled or the table is full, published ports break in bizarre partial ways.** Worth a `nf_conntrack_count` check in our diagnostics. `[O]`: `133 / 262144` on `host-a`.

**Egress (container → internet)** is the other direction and the MASQUERADE rule's actual job:
1. Container → default route → bridge → host.
2. Routing: destination is external ⇒ forward out `eth0` (needs `ip_forward`).
3. `filter/FORWARD` → `DOCKER-FORWARD` → `DOCKER-ISOLATION-STAGE-1` (`-i br-… ! -o br-…` **matches**) → `STAGE-2` (`-o br-9d68a0cc943b -j DROP`, `-o docker0 -j DROP` — neither matches, we're going out `eth0`) → RETURN → `DOCKER-BRIDGE` (no match, `-o` is `eth0`) → `-A DOCKER-FORWARD -i br-9d68a0cc943b -j ACCEPT`. **Accepted.**
4. `nat/POSTROUTING`: `-s 172.18.0.0/16 ! -o br-9d68a0cc943b` **MATCHES** ⇒ MASQUERADE ⇒ source becomes `198.51.100.10`.

### 4.4 `docker-proxy`: what it is actually for

`[O]` **HIGH** — Live on `host-a`:
```
$ ps aux | grep docker-proxy
root 2588176 /usr/bin/docker-proxy -proto tcp -host-ip 0.0.0.0 -host-port 80  -container-ip 172.18.0.4 -container-port 80  -use-listen-fd
root 2588184 /usr/bin/docker-proxy -proto tcp -host-ip ::      -host-port 80  -container-ip 172.18.0.4 -container-port 80  -use-listen-fd
root 2588192 /usr/bin/docker-proxy -proto tcp -host-ip 0.0.0.0 -host-port 443 -container-ip 172.18.0.4 -container-port 443 -use-listen-fd
root 2588199 /usr/bin/docker-proxy -proto tcp -host-ip ::      -host-port 443 -container-ip 172.18.0.4 -container-port 443 -use-listen-fd
$ ss -ltnp
LISTEN 0.0.0.0:443  users:(("docker-proxy",pid=2588192,fd=7))
LISTEN 0.0.0.0:80   users:(("docker-proxy",pid=2588176,fd=7))
LISTEN [::]:443     users:(("docker-proxy",pid=2588199,fd=7))
LISTEN [::]:80      users:(("docker-proxy",pid=2588184,fd=7))
```
Four processes, two per published port (one IPv4, one IPv6). Note `-use-listen-fd`: in Docker 28 the daemon creates and binds the socket itself and passes the fd to the proxy, rather than the proxy binding. That matters — it means **the bind, and therefore the "port is taken" error, happens in the daemon**, synchronously with `docker run`, which is where you want it.

`[F]` **MED** — What it does: a trivial userspace relay. `accept()` on the host-side listener, `connect()` to `<container-ip>:<container-port>`, then copy bytes both ways (for UDP, `recvfrom`/`sendto` with a per-source-tuple map). Two consequences: **the container sees the connection as coming from the bridge address, not the real client** (source IP is lost), and **every byte is copied through userspace twice**.

**When DNAT is sufficient and the proxy is idle** `[O]` **HIGH** — the counters settle this empirically. On `host-a` the IPv4 DNAT rules have carried `11878` and `23507` connections:
```
iifname != "br-9d68a0cc943b" tcp dport 80  counter packets 11878 bytes 648090  dnat to 172.18.0.4:80
iifname != "br-9d68a0cc943b" tcp dport 443 counter packets 23507 bytes 1339956 dnat to 172.18.0.4:443
```
Meanwhile the two IPv4 `docker-proxy` processes have accumulated `0:01` of CPU each over 11 days. **For ordinary IPv4 inbound traffic, DNAT does all the work and the proxy is a bystander** — it holds the port open and nothing more. Its `ss` entry is why `ss -ltnp` shows a listener at all, and that is itself load-bearing: it stops another process from binding the same port, and it makes `-p` visible to `ss`/`netstat`, which DNAT alone never would.

**The case where DNAT genuinely cannot work** `[O]` **HIGH — and this host proves it.**
```
$ ip6tables -t nat -S
-P PREROUTING ACCEPT
-N DOCKER
-A PREROUTING -m addrtype --dst-type LOCAL -j DOCKER
-A OUTPUT ! -d ::1/128 -m addrtype --dst-type LOCAL -j DOCKER
                                   <-- the DOCKER chain is EMPTY. No DNAT rules whatsoever.
$ docker network inspect webapp_default | grep EnableIPv6
"EnableIPv6": false
$ sysctl net.ipv6.conf.all.forwarding
net.ipv6.conf.all.forwarding = 0
```
There are `docker-proxy` processes listening on `[::]:80` and `[::]:443`, there are **zero** IPv6 DNAT rules, and IPv6 forwarding is off. Therefore: **every IPv6 connection to this production web server is being carried by `docker-proxy`, in userspace, and nothing else.** The reason is structural — the container has no IPv6 address at all, so there is no `IPv6 → IPv6` DNAT to write. A NAT rule cannot change address families. The only way to bridge `[::]:80` to `172.18.0.4:80` is a process that terminates the v6 connection and opens a fresh v4 one. This is documented behaviour: *"Ports on the host's IPv6 addresses will map to the container's IPv4 address if no host IP is given in a port mapping, the bridge network is IPv4-only, and `--userland-proxy=true` (default)."* — <https://docs.docker.com/engine/network/port-publishing/>

So the honest answer to "what is docker-proxy for" is **four separate things**, and only one of them is about the fast path:
1. **Address-family bridging (v6 host → v4 container).** Structurally impossible with NAT. *Verified live above.* This is the strongest reason it still exists.
2. **Holding the port.** Makes the mapping visible to `ss`, and makes a conflicting bind fail with `EADDRINUSE` at container-start time rather than silently shadowing.
3. **Loopback publishing without `route_localnet`.** See below.
4. **Hairpin / container-reaching-its-own-published-port**, by sidestepping the L2 same-port restriction (§3.5).

**Publishing on `127.0.0.1`** `[F]/[C]` **MED–HIGH** — two mechanisms, and Docker picks based on `userland-proxy`:
- **With the proxy (default):** `docker-proxy` simply `bind()`s `127.0.0.1:8080`. Nothing special needed; loopback bind is ordinary.
- **Without the proxy (`userland-proxy=false`):** DNAT must handle it, and here the kernel gets in the way. `[F]` HIGH — from `ip-sysctl.txt`, `route_localnet`: *"Do not consider loopback addresses as martian source or destination while routing. This enables the use of 127/8 for local routing purposes. default FALSE"*. A packet to `127.0.0.1` that needs to be **routed** to `172.18.0.4` is, by default, a martian and is dropped. So Docker sets `net.ipv4.conf.<bridge>.route_localnet=1`.
`[O]` **HIGH** — On `host-a`, `route_localnet` is **`0` on every single interface** (`all`, `default`, `lo`, `eth0`, `eth1`, all three bridges, all five veths). Consistent with: userland-proxy is enabled (default), no port is published to a loopback address (all `-host-ip 0.0.0.0`/`::`), so Docker never needed to set it. **Docker 28 does not enable `route_localnet` globally or speculatively.**
`[F]/[C]` **HIGH** — `route_localnet=1` is a genuine security downgrade, and Docker's own v28 notes describe the resulting bug: in releases **older than 28.0.0**, *"hosts within the same L2 segment (for example, hosts connected to the same network switch) can reach ports published to localhost."* Because with `route_localnet=1` the host will route a packet whose destination is `127.0.0.1` if one arrives from the wire, and `-p 127.0.0.1:8080:80` was therefore not private at all. Source: <https://docs.docker.com/engine/network/port-publishing/>

`[R]` **HIGH** — **Ship a userland proxy equivalent, but only for the cases that need it, and never as the data path for ordinary IPv4.** Concretely:
- IPv4 published port on a non-loopback host address ⇒ **DNAT only.** No helper process. This is the 95% case and it should be zero-overhead.
- IPv4 published port on `127.0.0.1`/`::1` ⇒ **helper process that binds the loopback address.** Do *not* use `route_localnet`; it re-introduces the pre-28 L2 leak, it is a persistent host mutation, and a `bind()` is free.
- IPv6 host address → IPv4-only container ⇒ **helper process.** No alternative exists.
- Bind the socket **in our daemon and pass the fd** to the helper (Docker 28's `-use-listen-fd`). This is strictly better: the `EADDRINUSE` surfaces synchronously in `mydocker run`, and if the helper crashes and is restarted the port is never briefly free.
Tradeoff: a helper process per published port is state that outlives us if we die (§10) and a supervision burden. Mitigate by making the helper die when its socket closes and by reconciling on startup.
`[R]` **MED** — **Better long-term: dual-stack the containers.** If containers get IPv6 addresses, the v6→v4 case disappears and DNAT covers everything. That is more work (IPv6 IPAM, ULA vs routable, `net.ipv6.conf.all.forwarding`), so it is a stage-6 item, but it is the *right* fix and it deletes the proxy's main justification.

### 4.5 Docker 28's `raw/PREROUTING` hardening — and why we need it too

`[O]` **HIGH** — one rule per container IP, `-d <ip> ! -i <its bridge> -j DROP`, in `raw/PREROUTING` (see §4.1).

`[C]` **HIGH** — Introduced in Docker Engine 28.0 to close a long-standing hole (moby issues #14041, #22054, #48815). The pre-28 exposure, from Docker's own writeup:
```
Prerequisites for exposure in pre-v28:
  1. net.ipv4.ip_forward enabled
  2. system-wide FORWARD chain set to ACCEPT
  3. another machine on the same LAN routed traffic to the container IP
Attack:  ip route add 172.17.0.2/32 via 10.0.0.3 ; nc 172.17.0.2 3306
```
i.e. **an attacker on the same LAN could reach *unpublished* container ports** just by adding a route, because nothing dropped traffic aimed directly at a container address. Source: <https://www.docker.com/blog/docker-engine-28-hardening-container-networking-by-default/>

Why `raw` and not `filter`: `raw/PREROUTING` runs at priority -300, **before conntrack and before nat**. Dropping there means the packet never creates a conntrack entry and never gets DNAT'd, so it cannot be laundered into looking legitimate.

`[R]` **HIGH** — **Implement this. It is four lines of rule-writing and it is the difference between "containers are isolated" being true and being aspirational.** Note the operational cost that comes with it: it is **one rule per container IP**, so the rule set grows linearly with running containers, and every container start/stop is a rule insert/delete in `raw/PREROUTING`. That is a strong argument for using a **named set** instead of individual rules — which is an argument for nftables (§5). `[C]` — Docker 28.0.0 originally used ipsets for this and backed it out in 28.0.1 to drop the `ip_set`/`xt_set` kernel module dependency; with native nftables we get sets without extra modules.

`[C]` **MED** — Docker 28 also generalized this into **`gateway_mode`** per network: `nat` (default), `nat-unprotected` (DNAT but no filtering — the pre-28 behaviour), `routed` (no NAT at all, but still only published ports reachable), `isolated` (with `--internal`). Source: <https://docs.docker.com/engine/network/port-publishing/>
`[R]` **LOW→MED** — worth copying the `routed` mode eventually, since it is the honest answer for people who *do* control their router and want real container addresses. Not MVP.

### 4.6 `DOCKER-USER`, and why host firewalls surprise everyone

`[O]` **HIGH** — This host is the textbook case: Docker and `ufw` coexist, and the FORWARD chain order tells the whole story.
```
-P FORWARD DROP
-A FORWARD -j DOCKER-USER                    <-- 1st
-A FORWARD -j DOCKER-FORWARD                 <-- 2nd  (ACCEPTs published traffic here)
-A FORWARD -j ufw-before-logging-forward     <-- 3rd  (never reached for that traffic)
-A FORWARD -j ufw-before-forward
-A FORWARD -j ufw-after-forward
-A FORWARD -j ufw-reject-forward
-A FORWARD -j ufw-track-forward
```
And ufw's actual user policy, which the admin clearly believes is the security boundary:
```
-A ufw-user-input -p tcp --dport 22  -j ACCEPT
-A ufw-user-input -p tcp --dport 80  -j ACCEPT
-A ufw-user-input -p tcp --dport 443 -j ACCEPT
```

`[F]` **HIGH** — Two independent reasons ufw does not restrict container traffic:
1. **Wrong chain.** ufw's rules above are in `ufw-user-input`, reached from **`INPUT`**. Traffic to a published port is **forwarded**, not delivered locally, so it traverses **`FORWARD`** and never touches `INPUT` at all. Docker's docs say it plainly: *"Docker routes container traffic in the `nat` table, which means that packets are diverted before it reaches the `INPUT` and `OUTPUT` chains that ufw uses."* — <https://docs.docker.com/engine/network/packet-filtering-firewalls/>
2. **Wrong order, even in FORWARD.** Docker's jumps are inserted at the **top** of `FORWARD` (via `iptables -I`), ahead of every ufw chain. `DOCKER-FORWARD` ACCEPTs the packet and iptables stops there — a terminal ACCEPT in a chain reached from a builtin chain ends traversal of that builtin. So ufw's FORWARD rules are dead code for published-port traffic.

The practical consequence, and it is severe: **`ufw deny 80` does not stop `-p 80:80`.** The admin's mental model ("I have a firewall") and reality ("the container engine punched through it and put itself first") diverge silently, with no warning anywhere. `[O]` — on this very production host, if the admin ran `ufw deny 80`, port 80 would remain open, because the ACCEPT happens in `DOCKER`/`filter` before ufw is consulted.

`[C]` **HIGH** — `DOCKER-USER` is Docker's answer: an **empty chain, jumped to first, that Docker creates but never writes rules into.** It is the documented place for admin rules that must be evaluated before Docker's ACCEPTs. It survives daemon restarts (Docker only ensures it exists and is first; it does not flush it). `[O]`: empty on `host-a` — the admin doesn't know about it, which is typical.

`[R]` **HIGH** — **Create a `MYDOCKER-USER` equivalent, jump to it first from `FORWARD`, never write into it, and never flush it.** It costs one chain and it is the only mechanism by which a host admin can actually override us.
`[R]` **HIGH** — **And go one better than Docker: tell the user.** On the first `-p` publish on a host where we detect a foreign firewall manager (a `ufw-*` or `firewalld`/`f2b-*` chain present in `filter`), print a one-line warning naming `MYDOCKER-USER`. This is a five-line check that prevents the single most common "Docker opened my port to the internet and I didn't know" incident. Docker's failure here is a documentation-only fix for a problem that wants a runtime warning.
`[R]` **MED** — Also integrate with firewalld if present (Docker creates a `docker` firewalld zone with target `ACCEPT` and a `docker-forwarding` policy). Lower priority than the warning; only relevant on RHEL-family hosts.

---

## 5. iptables vs nftables reality

### 5.1 What `iptables v1.8.10 (nf_tables)` actually means

`[F]` **HIGH** — Since iptables 1.8.0 there are two `iptables` binaries, selected by an alternatives symlink:
- `iptables-legacy` — talks to the **`x_tables`** kernel subsystem via `setsockopt`/`getsockopt` (`libiptc`). This is the original implementation.
- `iptables-nft` — talks to the **`nf_tables`** kernel subsystem via `libnftnl`/netlink. It parses iptables syntax, translates it to nft expressions, and writes it into nft tables.

The `(nf_tables)` in the version banner means the second. `[O]` HIGH — `host-a` reports `iptables v1.8.10 (nf_tables)`.

`[F]` **HIGH** — The translation layer works by creating nft tables **named after the iptables tables** (`ip filter`, `ip nat`, `ip mangle`, `ip raw`, `ip6 filter`, …) with base chains at the **classic iptables hook priorities**, and tagging them with a special udata marker so `nft` knows they are foreign.

`[O]` **HIGH** — The marker is visible, and `nft` warns about it, on `host-a`:
```
$ nft list tables
table ip filter
table ip6 filter
table ip nat
table ip6 nat
table ip raw
table inet f2b-table          <-- NOT iptables: native nft (fail2ban)
$ nft list table ip nat
# Warning: table ip nat is managed by iptables-nft, do not touch!
table ip nat { ... }
```

`[O]` **HIGH** — **The same rules, two views.** This is the clearest demonstration I got. The iptables view:
```
-A PREROUTING  -m addrtype --dst-type LOCAL -j DOCKER
-A POSTROUTING -s 172.18.0.0/16 ! -o br-9d68a0cc943b -j MASQUERADE
-A DOCKER ! -i br-9d68a0cc943b -p tcp -m tcp --dport 80 -j DNAT --to-destination 172.18.0.4:80
```
and the identical rules through `nft list table ip nat`:
```
chain PREROUTING {
    type nat hook prerouting priority dstnat; policy accept;
    fib daddr type local counter packets 14155948 bytes 761728165 jump DOCKER
}
chain POSTROUTING {
    type nat hook postrouting priority srcnat; policy accept;
    ip saddr 172.18.0.0/16 oifname != "br-9d68a0cc943b" counter packets 51369 bytes 3378872 masquerade
}
chain DOCKER {
    iifname != "br-9d68a0cc943b" tcp dport 80 counter packets 11878 bytes 648090 dnat to 172.18.0.4:80
}
```
The translation is mechanical and readable: `-m addrtype --dst-type LOCAL` → `fib daddr type local`; `-i`/`-o` → `iifname`/`oifname`; `-s` → `ip saddr`; `-j MASQUERADE` → `masquerade`. Note `counter` is added to every rule because iptables semantics guarantee per-rule counters, whereas in native nft counters are opt-in. **That is a real cost of the compatibility layer** — you pay for counters you may not want.

`[F]` **HIGH** — Consequences of the dual view that matter to us:
1. **`iptables-save` and `nft list ruleset` are two renderings of one kernel state.** There is no synchronization problem *between the views*; there is only one ruleset.
2. **`iptables-nft` cannot see native nft rules, and `iptables-save` will not show them.** `[O]` HIGH — `iptables -t filter -S` on `host-a` shows Docker's and ufw's chains and **does not mention `f2b-table` at all**, even though fail2ban's rule is live and rejecting SSH traffic. An admin auditing with `iptables-save` would conclude fail2ban wasn't running.
3. `iptables-legacy` and `iptables-nft` rules **can coexist in the kernel and both be evaluated**, because `x_tables` and `nf_tables` register independent hooks. This is the genuinely dangerous configuration: two rulesets, both live, each invisible to the other's tooling.

### 5.2 The ordering problem, live

`[F]` **HIGH** — nftables base chains declare a **hook** and a **numeric priority**; lower runs first. The named aliases: `raw = -300`, `mangle = -150`, `dstnat = -100`, `filter = 0`, `srcnat = 100`, `security = 50`. `iptables-nft` uses exactly these so that translated rules land where iptables users expect.

`[O]` **HIGH** — And here is the problem in production, on `host-a`:
```
$ nft list table inet f2b-table
table inet f2b-table {
    set addr-set-sshd { type ipv4_addr }
    chain f2b-chain {
        type filter hook input priority filter - 1; policy accept;
        tcp dport 22 ip saddr @addr-set-sshd reject with icmp port-unreachable
    }
}
```
fail2ban writes **native nft** and chooses `priority filter - 1` = **-1**, deliberately one less than the `filter` (0) priority where `iptables-nft`'s `INPUT` chain lives. So fail2ban's rules are evaluated **before** every iptables rule on the host, and there is nothing ufw or Docker could write in iptables to get ahead of it.

That is the whole ordering hazard, stated generally `[F]` **HIGH**:
- Within one table, rule order is what you'd expect.
- **Across** tables at the same hook, **priority decides**, and a native-nft program can pick any priority it likes — including one that preempts all iptables rules.
- iptables-nft is pinned to the legacy priorities and therefore **cannot compete** on priority. Its only lever is rule position within its own chain.
- Worse, a terminal verdict in a lower-priority chain (`accept` in nftables means "accept for this table/hook", `drop` is immediate and final) can end evaluation before the higher-priority table is consulted at all.

`[O]` **HIGH** — This host has **three** independent firewall writers: Docker (iptables-nft), ufw (iptables-nft), fail2ban (native nft). Two of the three are invisible to each other's tooling. This is not an exotic setup; it is a stock Ubuntu server with three ordinary packages.

### 5.3 What Docker and Podman do today

`[C]` **HIGH** — **Docker.** Historically iptables-only (`--iptables=true`). Docker 28 introduced a **firewall backend abstraction**, surfaced through the API: *"`GET /info` now returns a `FirewallBackend` containing information about the daemon's firewalling configuration"* (v28.1.0). `[O]` HIGH — on `host-a`, `docker info` reports `"FirewallBackend":{"Driver":"iptables"}`. So the abstraction exists and iptables is still what ships/defaults here. Docker's docs note *"For bridge networks, iptables and nftables have the same functionality."*
Docker 28 also restructured its chains explicitly to be a better neighbour: *"Move most of Docker's iptables rules out of the filter-FORWARD chain, so that other applications are free to append rules that must follow Docker's rules."* `[O]` — that is exactly the `DOCKER-FORWARD`/`DOCKER-CT`/`DOCKER-BRIDGE` structure in §4.1.
Sources: <https://docs.docker.com/engine/release-notes/28/>, <https://docs.docker.com/engine/network/packet-filtering-firewalls/>

`[C]` **MED–HIGH** — **Podman/netavark went further and finished the job.** netavark has both an `iptables` and an `nftables` firewall driver, and **nftables is now the default; the iptables driver is deprecated and slated for removal.** Fedora tracked the switch as a release change (`Changes/NetavarkNftablesDefault`). The stated reasons are the ones that matter to us: **nftables lets you batch all rule insertions into a single atomic transaction**, which is *"more performant and robust"* than iptables' rule-at-a-time model.
Sources: <https://fedoraproject.org/wiki/Changes/NetavarkNftablesDefault>, <https://github.com/containers/netavark/discussions/1332>, <https://github.com/containers/netavark/blob/main/docs/netavark-firewalld.7.md>
`[LOW]` on the exact removal timeline — the "removed next year" claim comes from a GitHub discussion, not a release note. What would settle it: netavark's release notes / the `firewall-driver` option docs.

### 5.4 Recommendation: target nftables natively

`[R]` **HIGH** — **Write native nftables via netlink (`NFNL_SUBSYS_NFTABLES`), in our own table, and do not touch iptables at all.**

The compatibility argument spelled out, because this is the decision that looks wrong at first glance:

**The naive worry is "iptables is more compatible."** It isn't, and the reason is that *`iptables` is no longer an implementation, it is a syntax*. On any modern distro, `iptables` **is** nftables underneath — `[O]` verified on `host-a`, Ubuntu 24.10 with `iptables v1.8.10 (nf_tables)`. So targeting nftables is not choosing the exotic option; it is choosing the thing that is actually running. Writing iptables syntax means routing our rules through a *translation layer* to reach the same kernel subsystem we could have addressed directly.

**Concrete reasons, in order of weight:**
1. **Atomicity.** nftables applies a whole transaction or none of it. This is the single biggest win, and it lands directly on the hardest problem in this document: §10, partial state after a crash. With iptables, publishing a port is N separate `setsockopt`/netlink operations, and dying at N-1 leaves a half-configured host that is *insecure* (a DNAT with no matching filter ACCEPT, or worse, a filter ACCEPT with no DNAT). With nftables it either happened or it didn't. **This converts our nastiest class of bug into a non-issue**, and no amount of careful ordering in iptables gets us the same guarantee.
2. **Named sets and maps.** §4.5's anti-spoof rules are one-per-container-IP and grow linearly. In nftables that is a single rule against a named set, and adding a container is a set-element insert — O(1) rule count, O(1) update, and a hash lookup instead of a linear scan. Same for published-port maps (`dnat to tcp dport map @portmap`). Docker had to *back out* an ipset-based version of this in 28.0.1 to avoid `xt_set` module dependencies; with native nft, sets are core functionality with no extra modules.
3. **Priority control.** We can place our chains where we choose. iptables-nft cannot (§5.2). Given that we must coexist with ufw, firewalld, and fail2ban, being able to pick a priority is real leverage — and it lets us be a *good* neighbour deliberately rather than by accident.
4. **One namespace of our own.** `table inet mydocker` is entirely ours. We can `nft delete table` it for a clean full reset — which makes reconciliation (§10) trivial: destroy and rebuild from our own state, atomically. With iptables we must surgically remove our rules from chains we share with others, matching on rule text.
5. **`inet` family.** One table handles IPv4 and IPv6, instead of maintaining parallel `iptables` and `ip6tables` rule sets. Halves the code and removes a whole class of "we forgot the v6 rule" bugs — which, per §4.4, is a real gap in the Docker setup on this very host.
6. **Direction of travel.** netavark has already made nftables the default and deprecated iptables; Docker has built the backend abstraction. Nobody is moving the other way.

**Tradeoffs, honestly:**
- **Older kernels/distros.** nftables needs ≥ 3.13 for basics and realistically ≥ 4.18 for comfortable `nat`/`inet` support. `[O]` MED — `old-host` runs 4.18 and `nft` works there, including `type nat hook postrouting` inside an unprivileged netns (§8). RHEL 7 (3.10) would be excluded; we do not care.
- **Admin familiarity.** Users know `iptables-save`. Our rules will not appear there. `[R]` — mitigate by making `mydocker network inspect` print the actual nft ruleset, and by documenting `nft list table inet mydocker` prominently. This is a genuine ergonomic cost and we should not pretend otherwise.
- **Coexistence with an iptables-based `FORWARD DROP` policy.** If ufw or another tool sets `filter/FORWARD` policy to DROP in *its* table, our `accept` in *our* table does not override it — both hooks run, and a DROP anywhere wins. `[F]` HIGH — this is the flip side of point 3: priority buys us ordering, not authority. **We must handle this explicitly**: detect a foreign DROP policy on FORWARD and warn (§4.6's warning covers it). Docker sidesteps it by living in the same table as ufw; we won't have that option. This is the strongest argument against native nft and it is worth stating plainly. It is still outweighed by atomicity.

`[R]` **MED** — **Implementation route: link `libnftnl` via the `nftnl` crate rather than hand-rolling the nftables netlink protocol.** Reasoning: unlike rtnetlink (§2.6, a small frozen message set), the nftables expression encoding is large, genuinely intricate, and still evolving — hand-rolling it is a month, not a weekend, and it is not where our learning value is. `nftnl` 0.9.4 (released 2026-08-11, MSRV 1.85, `mullvad/nftnl-rs`) is what Mullvad VPN ships in production. Tradeoff: a C library dependency (`libnftnl` + `libmnl`) and an `unsafe` surface that is "the whole C API". The pure-Rust alternative, `rustables` 0.8.8, has ~44k lifetime downloads and roughly annual releases — too thin to bet on. `[R]` LOW — revisit `rustables` later; if it matures, the migration is contained behind our own firewall trait.

`[R]` **HIGH** — **Do not shell out to `nft`** for the same reasons as `ip` (§2.6), with one addition: shelling out to `nft -f -` with a generated ruleset file actually *does* get us atomicity, so it is a **legitimate MVP shortcut here in a way it is not for netlink.** If we want to defer the `nftnl` dependency, `nft -f -` with a fully-generated table is an honest stopgap: atomic, debuggable, one exec per operation. `[R]` MED — take this shortcut first if it unblocks progress, and mark it clearly as a shortcut with a known ceiling (an exec per container start, and error strings instead of error codes).

---

## 6. DNS

### 6.1 The embedded resolver at `127.0.0.11`, fully mapped

`[O]` **HIGH** — Verified end to end on `host-a`. The container's `/etc/resolv.conf`, which Docker generates and helpfully self-documents:
```
# Generated by Docker Engine.
# This file can be edited; Docker Engine will not make further changes once it
# has been modified.

nameserver 127.0.0.11
search .
options edns0 trust-ad ndots:0

# Based on host file: '/etc/resolv.conf' (internal resolver)
# ExtServers: [host(127.0.0.53)]
# Overrides: []
# Option ndots from: internal
```
`[O]` **HIGH** — the netns-local NAT that makes `127.0.0.11:53` work:
```
$ nsenter -t 2588141 -n iptables -t nat -S
-N DOCKER_OUTPUT
-N DOCKER_POSTROUTING
-A OUTPUT      -d 127.0.0.11/32 -j DOCKER_OUTPUT
-A POSTROUTING -d 127.0.0.11/32 -j DOCKER_POSTROUTING
-A DOCKER_OUTPUT -d 127.0.0.11/32 -p tcp -m tcp --dport 53 -j DNAT --to-destination 127.0.0.11:39251
-A DOCKER_OUTPUT -d 127.0.0.11/32 -p udp -m udp --dport 53 -j DNAT --to-destination 127.0.0.11:55634
-A DOCKER_POSTROUTING -s 127.0.0.11/32 -p tcp -m tcp --sport 39251 -j SNAT --to-source :53
-A DOCKER_POSTROUTING -s 127.0.0.11/32 -p udp -m udp --sport 55634 -j SNAT --to-source :53
```
`[O]` **HIGH** — and the listeners those rules point at:
```
$ nsenter -t 2588141 -n ss -lunp
UNCONN 127.0.0.11:55634  users:(("dockerd",pid=70234,fd=52))
$ nsenter -t 2588141 -n ss -ltnp
LISTEN 127.0.0.11:39251  users:(("dockerd",pid=70234,fd=54))
```

**The complete mechanism, assembled:**
1. `dockerd` (host netns) creates a UDP and a TCP socket **while temporarily `setns`'d into the container netns** (§1.5), binding each to `127.0.0.11` on a **random high port** (55634 / 39251 here).
2. Docker writes `nameserver 127.0.0.11` into the container's `/etc/resolv.conf`.
3. The container's resolver sends to `127.0.0.11:53`. The container-local `nat/OUTPUT` DNATs that to the high port.
4. The reply's source would be `127.0.0.11:<highport>`, which the resolver would **reject** — glibc and musl both verify the reply comes from the address:port they queried. So `nat/POSTROUTING` SNATs the source back to `:53`.
5. `dockerd` answers container names from its own records, and forwards anything else to the external servers.

`[R]` **HIGH** — **Why the DNAT-to-random-port dance instead of just binding `:53`?** Not permissions — `dockerd` is root. The reason is **conflict avoidance**: a container may legitimately want to run its own DNS server on `:53` (a `bind`/`dnsmasq`/`coredns` container is not exotic). If Docker occupied `127.0.0.11:53`, that would still be fine (different address)… but the container might bind `0.0.0.0:53`, which **includes** `127.0.0.11:53` and would collide. Binding a random high port makes the embedded resolver immune to whatever the container does with port 53. `[MED]` on this being *the* stated rationale — it is the only explanation consistent with the observed design, but I did not find it written down by a Docker maintainer. What would settle it: the commit message / design doc for libnetwork's resolver. Note the practical corollary either way: **the port is random per container, so the rules must be written per container netns after the socket is bound.**

`[F]` **HIGH** — **Why `127.0.0.11`?** Three properties, all necessary: (a) it is in `127.0.0.0/8`, so it is unambiguously loopback and can never be confused with a real host on the network or collide with the container's subnet; (b) the whole `/8` is already routed to `lo` by the kernel's automatic loopback route, so no route needs adding; (c) it is **not** `127.0.0.1`, so it does not collide with services the container binds on localhost, and it is distinctive enough to be recognizable in a `resolv.conf`. `[C]` — the specific value `.11` is arbitrary convention.
`[C]` **HIGH** — Docker's docs: *"there is no IPv6 equivalent; the IPv4 address works even in IPv6-only containers."* — <https://docs.docker.com/engine/network/>

### 6.2 `/etc/resolv.conf` and `/etc/hosts` generation

`[F]` **HIGH** — These are **bind-mounted files, not part of the image.** The engine generates them per container on the host (Docker: `/var/lib/docker/containers/<id>/resolv.conf` and `hosts`) and bind-mounts them over the image's copies. That is why editing them inside a container does not survive a restart, and why they are not in the image's layers. This is a **storage/mount concern as much as a networking one** → **Handoff to D3.**

`[O]` **HIGH** — the generated `/etc/hosts` on `host-a`:
```
127.0.0.1	localhost
::1	localhost ip6-localhost ip6-loopback
fe00::	ip6-localnet
ff00::	ip6-mcastprefix
ff02::1	ip6-allnodes
ff02::2	ip6-allrouters
172.18.0.4	301b2be6f7c8
```
Six boilerplate lines plus **one** meaningful line: the container's own IP mapped to its hostname (which is the short container ID, matching `/etc/hostname`). `[R]` HIGH — that last line matters more than it looks: a huge amount of software calls `gethostname()` then resolves it, and fails confusingly if that doesn't work. Include it.

### 6.3 The systemd-resolved complication — present, and Docker documents its own workaround

`[O]` **HIGH** — `host-a` is the exact problem case:
```
$ readlink -f /etc/resolv.conf
/run/systemd/resolve/stub-resolv.conf
$ cat /etc/resolv.conf
nameserver 127.0.0.53
options edns0 trust-ad
search .
$ ss -ltnp | grep :53
LISTEN 127.0.0.54:53      users:(("systemd-resolve",pid=551,fd=17))
LISTEN 127.0.0.53%lo:53   users:(("systemd-resolve",pid=551,fd=15))
```

`[F]` **HIGH** — **The problem: `127.0.0.53` is meaningless inside a container netns.** Each netns has its own loopback. A container sending to `127.0.0.53:53` reaches *its own* loopback, where nothing is listening. So **naively copying the host's `/etc/resolv.conf` into a container yields a container with no working DNS** — on the majority of modern Linux desktops and servers, since systemd-resolved is the default on Ubuntu, Fedora, and Arch.

`[O]` **HIGH** — And Docker's generated file states its own detection and workaround in a comment:
```
# Based on host file: '/etc/resolv.conf' (internal resolver)
# ExtServers: [host(127.0.0.53)]
```
`(internal resolver)` = "I noticed the host file was unusable, so I switched to the embedded resolver at `127.0.0.11`". `ExtServers: [host(127.0.0.53)]` = "and I will forward upstream queries to `127.0.0.53`, **resolved from the host network namespace**" — the `host(...)` wrapper is exactly that annotation. Since `dockerd` lives in the host netns, it *can* reach `127.0.0.53`; the container cannot. The embedded resolver is the bridge across that boundary.

This is a genuinely elegant design and it is worth restating as the general principle `[R]` **HIGH**: **an in-netns listener owned by a host-netns process is the only clean way to proxy a loopback-scoped host service into a container.** The same pattern would serve for any `host.containers.internal`-style facility.

`[F]` **MED** — Docker's actual decision tree, as inferred from the generated file and documented behaviour: if every nameserver in the host's `resolv.conf` is a loopback address, the file cannot be used verbatim; fall back to the embedded resolver (or, historically and still as a documented fallback, substitute public resolvers like `8.8.8.8`). If at least one nameserver is non-loopback, the file can be filtered down to the usable entries and copied. `[MED]` on the precise algorithm; what would settle it: `libnetwork/resolvconf` in moby.

### 6.4 Name resolution: user-defined networks vs the default bridge vs `--link`

`[C]` **HIGH** — Docker's docs are explicit about the asymmetry: on **user-defined** networks containers *"can communicate with each other using container IP addresses or container names"*; on the **default** bridge they *"have unrestricted network access to each other using container IP addresses. They cannot refer to each other by name."*
`[O]` **HIGH** — Consistent with the host: all five containers are on the user-defined `webapp_default`, and the compose app relies on name resolution (`webapp-redis-1`, `webapp-web-1`, …). Nothing is on the default bridge.

`[C]` **HIGH** — **`--link` is the legacy mechanism and it worked completely differently**: it injected `/etc/hosts` entries (and environment variables) into the *dependent* container at start time. Its fatal flaws: static — if the target container restarted with a new IP, the entries were stale; unidirectional; required declaring the dependency at `docker run` time, so it could not express a cycle; and it polluted the environment with `<NAME>_PORT_80_TCP_ADDR`-style variables. It is deprecated.
`[R]` **HIGH** — **Do not implement `--link`.** It is strictly worse than a resolver and it is deprecated in the thing we are copying. The `/etc/hosts` injection trick is, however, *exactly the right MVP* (below) — the mistake in `--link` was making it a user-facing dependency declaration, not the file-writing itself.

### 6.5 The honest minimum for us

`[R]` **HIGH** — **A three-stage ladder, and stage 1 is genuinely shippable.**

**Stage 1 — rewrite, don't copy, the host `resolv.conf`. (~30 lines.)**
Read the host's `/etc/resolv.conf`. Filter the `nameserver` lines:
- Keep every non-loopback address.
- **Discard loopback addresses** (`127.0.0.0/8`, `::1`) — they are unreachable from the container netns.
- If nothing survives, fall back to a configured default (and log loudly). Do **not** silently hardcode `8.8.8.8`: that is a privacy decision we should not make for the user. Make it a flag with no default, and if unset, emit a clear error telling the user to pass `--dns`.
Keep `search`/`options` lines as-is. Write it to a per-container file and bind-mount it.
This gets working external DNS on every host, including systemd-resolved ones — where it works because we *drop* `127.0.0.53` and fall back, rather than because we understand it.
`[R]` — **also generate `/etc/hosts`** with the boilerplate plus `<container-ip> <hostname>`. Together these two files are the whole of stage 1.

**Stage 2 — `/etc/hosts` injection for container-to-container names. (~20 lines.)**
When a container joins a network, append `<ip> <name>` for every existing peer to the new container's `hosts` file, and append the new container's line to every peer's `hosts` file. This works, requires no resolver, no listener, no netns socket trickery, and no NAT rules.
**Its honest ceiling, stated:** it is a snapshot. A container that restarts with a new IP leaves every peer with a stale entry. `[R]` — this is the same flaw that killed `--link`, so it is a **known-ceiling shortcut, not a design**: acceptable while container IPs are stable within a run, and the trigger to move to stage 3 is the first time we support restart-with-new-IP or dynamic network attach.
`[R]` **MED** — mitigate cheaply by making IPAM assignment **sticky per container name** (§11): if a restarted container reliably gets its old IP back, stale entries are correct entries and stage 2 lasts much longer.

**Stage 3 — the embedded resolver.** Only when stage 2's ceiling is actually hit. It needs: a DNS server (query parse, A/AAAA answer from our own records, forward the rest, splice the response), the setns socket-creation dance (§1.5), and the per-netns DNAT/SNAT rules with a random port. That is a real chunk of work — a DNS message codec is not free — and it buys dynamic name resolution and nothing else.
`[R]` **MED** — When we get there, prefer **binding `127.0.0.11:53` directly** and skipping the DNAT/SNAT rules entirely, accepting the small risk of a container that binds `0.0.0.0:53`. Reasoning: it removes two NAT rules per container from the setup path and removes a whole failure mode (rules written before/after the socket bind), for a conflict that is rare and detectable. Tradeoff: a container running its own DNS server on `0.0.0.0:53` will fail to bind. That is a clear, loud failure — much better than the silent weirdness the rules exist to prevent. Revisit if it bites.

---

## 7. Container-to-container communication, and shared network namespaces

### 7.1 Same-bridge communication

`[F]` **HIGH** — Two containers on one bridge talk **L2, switched, without touching the IP forwarding path at all.** Container A ARPs for B's address (flooded on the bridge), B replies, the bridge learns both MACs, and thereafter frames go port-to-port. No routing decision, no `ip_forward`, no `FORWARD` chain, no NAT.

`[O]` **HIGH** — Confirmed on `host-a` from inside the nginx container: its neighbour table holds two *peer containers* (`172.18.0.6`, `172.18.0.3`) alongside the gateway, and the bridge's FDB has learned their MACs on the corresponding veth ports (§3.1).

Three consequences worth internalising `[F]` **HIGH**:
1. **Same-bridge traffic is invisible to iptables by default** — no `br_netfilter` (§3.4), no visibility. Any policy we want here needs either `br_netfilter` or bridge-port flags.
2. **`MASQUERADE` deliberately excludes it** via `! -o <bridge>` (§3.2), so peers see each other's real container IPs. Correct and necessary — otherwise every peer would appear to be the gateway.
3. It is **fast** — a veth-to-veth hop through the bridge, no routing, no NAT, no conntrack.

`[O]` **HIGH** — **Cross-bridge is blocked, and that is what `DOCKER-ISOLATION-STAGE-*` is for:**
```
-A DOCKER-ISOLATION-STAGE-1 -i docker0 ! -o docker0 -j DOCKER-ISOLATION-STAGE-2
-A DOCKER-ISOLATION-STAGE-1 -i br-9d68a0cc943b ! -o br-9d68a0cc943b -j DOCKER-ISOLATION-STAGE-2
-A DOCKER-ISOLATION-STAGE-2 -o br-9d68a0cc943b -j DROP
-A DOCKER-ISOLATION-STAGE-2 -o docker0 -j DROP
```
Read as a two-stage test: stage 1 selects "traffic leaving a Docker bridge for somewhere else"; stage 2 drops it if the destination is *another* Docker bridge. Traffic leaving a bridge for `eth0` passes both and reaches the MASQUERADE. **This is the mechanism behind "different Docker networks are isolated."** The two-stage split exists to keep the rule count at O(n) rather than O(n²) for n bridges.
`[R]` **MED** — copy this structure; with nftables it collapses further into a single rule with two named sets (`iifname @dockerbridges` / `oifname @dockerbridges`), which is O(1) rules.

### 7.2 `--icc`

`[C]` **HIGH** — `com.docker.network.bridge.enable_icc`, default `true`. `[O]` HIGH — on `host-a`: `"com.docker.network.bridge.enable_icc": "true"` on the default bridge.
`[C]` **MED** — With `enable_icc=false`, Docker installs a DROP for bridge-to-itself traffic, which **requires `br_netfilter`** to be effective (§3.4) since the traffic is L2-switched.
`[R]` **HIGH** — **Skip `--icc` in the MVP; when we do it, use `IFLA_BRPORT_ISOLATED = 1` (attr 33) per veth port, not netfilter.** Full reasoning in §3.4: it avoids a global module and sysctl, avoids conntrack pressure, and the kernel bridge enforces it directly. Tradeoff: no per-pair policy.

### 7.3 Sharing a network namespace between containers

`[F]` **HIGH** — "Sharing a netns" means two containers' processes have the **same** `net` entry in their `nsproxy`. They therefore share: interfaces, addresses, routes, the **loopback**, the port space, the netfilter tables, and `/proc/sys/net`. They can reach each other on `127.0.0.1`, and they **cannot both bind the same port**.

`[C]` **HIGH** — Mechanisms:
- `docker run --network=container:X` — join X's netns.
- **Kubernetes pods** — every container in a pod shares one netns. This is the *defining* property of a pod; it is why containers in a pod talk over `localhost` and why a pod has one IP.
- **The pause/sandbox container** — a tiny container that does nothing but hold the netns open. It exists because of the lifecycle problem: if the netns belonged to a *workload* container, restarting that container would destroy the namespace and every sibling's networking. The pause container's job is to be the stable owner, so workload containers can come and go while the pod's IP and interfaces persist.

`[R]` **HIGH** — **In our OCI-shaped design, no pause container is needed, because the nsfs bind mount already does its job.** This is a nice consequence of §1.2/§1.6: a bind-mounted netns persists with **zero** processes. The pause container is a workaround for runtimes that only had "a process holds the namespace" available; we have the better primitive from the start. `[R]` — this is worth stating in our design docs because "why don't you have a pause container?" will come up.

`[R]` **MED** — **Do we need netns sharing? Yes, and it is nearly free — implement it early.**
- **Cost:** essentially nothing. It is the *absence* of work: skip netns creation, skip veth, skip IPAM, skip rule installation; just `setns` the new container into the existing netns (or, in OCI terms, set `namespaces[].path` to the existing netns). The OCI Runtime Spec already expresses it (§0) — a `network` namespace entry *with* a `path`.
- **Value:** it is the foundation of pod semantics, it makes sidecars possible, it is the natural way to implement `--network=host` (`path` = `/proc/1/ns/net`), and it is how debugging tools attach (`mydocker exec`-style network debugging, `nicolaka/netshoot`).
- **Tradeoff:** lifecycle coupling. If container A owns the netns and B joined it, tearing down A must not break B. **This is exactly the problem the pause container solves, and our answer is the bind mount**: the netns is owned by *the network sandbox*, a first-class object in our state, not by whichever container happened to be created first. `[R]` **HIGH** — **model the network sandbox as its own entity with its own refcount from day one.** Getting this wrong later is a painful refactor, and getting it right costs one struct.

---

## 8. Rootless networking

### 8.1 The hard wall, verified

`[F]` **HIGH** — The gate is `CAP_NET_ADMIN` **in the user namespace that owns the target network namespace**. Every mutating rtnetlink message goes through it (`net/core/rtnetlink.c`):
```c
/* every non-GET rtnetlink message: */
if (kind != RTNL_KIND_GET && !netlink_net_capable(skb, CAP_NET_ADMIN)) return -EPERM;

/* resolving a target netns from IFLA_NET_NS_FD / _PID / IFLA_TARGET_NETNSID: */
if (!netlink_ns_capable(skb, net->user_ns, cap)) { put_net(net); return ERR_PTR(-EPERM); }
```
and `rtnl_get_net_ns_capable()` even documents the rule in a comment: *"For now, the caller is required to have CAP_NET_ADMIN in the user namespace owning the target net ns."*
`[F]` **HIGH** — `drivers/net/veth.c` itself contains **no** capability check; `veth_newlink()` just calls `rtnl_create_link(peer_net, …)`. The entire permission decision lives in the rtnetlink layer above it. So this is not a veth-specific policy — it is the uniform rule for every device operation.
Source: <https://git.kernel.org/pub/scm/linux/kernel/git/torvalds/linux.git/plain/net/core/rtnetlink.c>

**The consequence, stated precisely:**
- `unshare -Urn` gives you a new netns **owned by your new userns**, where you are uid 0. So `ns_capable(net->user_ns, CAP_NET_ADMIN)` **succeeds** for that netns. You can do essentially everything *inside* it.
- The **initial** netns is owned by `init_user_ns`, where you have nothing. So creating an interface there, moving a veth end there, or enslaving anything to a host bridge all return `EPERM`.

`[O]` **HIGH** — Verified on `old-host` (Rocky 8.8, kernel 4.18.0-425.3.1.el8, unprivileged uid 900000000). What **works** inside `unshare -Urn`:
```
$ unshare -Urn sh -c 'grep CapEff /proc/self/status; id -u'
CapEff:	000001ffffffffff          # full capability set...
0                                 # ...as uid 0 in the new userns

$ unshare -Urn sh -c 'ip link add v0 type veth peer name v1 && echo VETH_CREATE_OK'
VETH_CREATE_OK
$ unshare -Urn sh -c 'ip link add br0 type bridge && ip link add v0 type veth peer name v1
    ip link set v0 master br0 && ip addr add 10.99.0.1/24 dev br0 && ip link set br0 up'
BRIDGE_OK / ENSLAVE_OK / ADDR_OK / UP_OK
$ unshare -Urn sh -c 'nft add table ip t
    nft add chain ip t c "{ type nat hook postrouting priority srcnat; }"'
NFT_ADD_TABLE_OK / NFT_NAT_CHAIN_OK
$ unshare -Urn sh -c 'echo 1 > /proc/sys/net/ipv4/ip_forward; cat /proc/sys/net/ipv4/ip_forward'
1                                 # host value unchanged
```
**The entire bridge+NAT model works rootless — bridges, veths, addresses, enslaving, nftables nat chains, network sysctls — as long as it all stays inside the namespace.**

`[O]` **HIGH** — And the one thing that **fails**, which is the whole story:
```
$ sleep 300 &            # a helper process in the HOST netns
$ HELPER=$!
$ unshare -Urn sh -c "ip link add v0 type veth peer name v1
    ip link set v1 netns $HELPER; echo \"exit=\$?\""
RTNETLINK answers: Operation not permitted
exit=2
```
`EPERM`, exactly as the kernel source predicts. **This is the wall: there is no way to connect an unprivileged netns to the host's network stack with a veth.** It is not a distro policy, not a sysctl, not a missing capability we could request — it is the ownership model. Which is why usermode networking exists at all.

`[F]` **MED** — A second, independent hard stop even *with* privilege: `__dev_change_net_namespace()` refuses devices marked `dev->netns_immutable` (formerly `NETIF_F_NETNS_LOCAL`) — loopback and many physical drivers — with ext-ack `"The interface netns is immutable"`. So `lo` can never be moved between namespaces.

`[O]` **HIGH** — A related permission finding that surprised me and is worth recording: **being uid 0 in a userns does not give you DAC override over files owned by uids outside that userns's mapping.**
```
$ unshare -Urmn sh -c 'mkdir -p /var/run/netns'
mkdir /var/run/netns failed: Permission denied
```
Even as root-in-userns with a private mount namespace, `mkdir` in `/run` fails, because `/run` is owned by real `root:root` and our uid 0 maps to 900000000 (`uid_map: 0 900000000 1`). Capabilities are namespaced; file ownership is not. `[R]` — for rootless we must put our state under a directory the *real* user owns (`$XDG_RUNTIME_DIR/mydocker/`), never `/run/mydocker/`. Mostly D5's domain, but it lands on our netns bind-mount path directly.

### 8.2 slirp4netns

`[F]` **HIGH** — Architecture, from `main.c`:
1. `slirp4netns` **forks**. The child calls `nsenter()`: `setns(usernsfd, CLONE_NEWUSER)` **then** `setns(netnsfd, CLONE_NEWNET)` — note the order, which is exactly §1.4's rule.
2. The child does `open("/dev/net/tun")` + `ioctl(fd, TUNSETIFF, {IFF_TAP|IFF_NO_PI})`, creating **`tap0` inside the container netns**.
3. The child passes the tap fd back to the **parent** over a `socketpair` as an `SCM_RIGHTS` cmsg.
4. The **parent** — still in the host netns — runs libslirp against that fd.

`[O]` **HIGH** — That fd-passing *is* the "avoids copying packets across namespaces" claim: one process reads Ethernet frames from a tap living in another netns and speaks ordinary L4 sockets in the host netns. There is no second hop.

`[F]` **HIGH** — **libslirp** is the user-mode TCP/IP stack extracted from QEMU's SLIRP into a standalone library. Dynamically linked since slirp4netns 1.0.0. `[O]` HIGH — on `old-host`: `slirp4netns version 1.2.3, libslirp: 4.4.0, libseccomp: 2.5.2`.

`[F]` **HIGH** — Defaults, from `slirp4netns.1`: MTU 1500; CIDR `10.0.2.0/24`; **gateway/host `10.0.2.2`**; **DNS `10.0.2.3`**; DHCP range `10.0.2.15–10.0.2.30`; `--configure` assigns `10.0.2.100`; IPv6 `fd00::/64`, gw `fd00::2`, DNS `fd00::3`.
*(Correction to a widely-repeated error: the gateway is `10.0.2.2`, not `10.0.2.3` — `.3` is the DNS.)*

`[F]` **HIGH** — **Single-threaded, one copy per frame.** `do_slirp()` is one `g_poll()` loop: `slirp_pollfds_fill_socket()` → `g_poll()` → `read(tapfd, buf, ETH_BUF_SIZE)` → `slirp_input(slirp, buf, rc)` → `slirp_pollfds_poll()`. `[O]` HIGH — that loop is the throughput ceiling, and it is why MTU dominates every benchmark: fewer loop iterations per byte.

`[F]` **HIGH** — **ICMP.** libslirp `src/ip_icmp.c` first tries `socket(AF_INET, SOCK_DGRAM, IPPROTO_ICMP)` (a Linux "ping socket"), falling back to `SOCK_RAW` on `EAFNOSUPPORT|EPROTONOSUPPORT|EACCES`. The DGRAM path is gated by `net.ipv4.ping_group_range`, whose upstream default is `"1 0"` = nobody; the RAW fallback needs `CAP_NET_RAW`, unavailable rootless. Hence `ping` failing in rootless containers, and the documented fix `sysctl net.ipv4.ping_group_range="0 2147483647"`.
`[O]` **HIGH** — ties directly to my §1.1 finding: a fresh netns on `old-host` has `ping_group_range = 65534 65534`. Since the sysctl is per-netns and writable by userns-root, **we can fix this ourselves inside the container netns** — no host change needed.

`[F]` **HIGH** — **Protocol subset: TCP, UDP, and ICMP Echo only.** No SCTP, GRE, ESP, OSPF; no raw sockets reaching outside.
`[F]` **HIGH** — `--api-socket` gives dynamic port forwarding via a QMP-like JSON protocol (`add_hostfwd`/`list_hostfwd`/`remove_hostfwd`). Documented caveats: the client must `shutdown(SHUT_WR)` after each request, requests must be < 4096 bytes, IPv4 only, and — importantly — **"slirp4netns 'stops the world' during processing API requests"**.
`[F]` **HIGH** — Gotcha from the man page's BUGS section: kernel 4.20 raised default `net.ipv4.tcp_rmem` from 87380 to 131072, which *slows* slirp4netns port forwarding; the workaround is lowering it inside the netns, and *"no real root privilege is needed to modify the file since kernel 4.15."*

`[O]` **HIGH** — I could not run slirp4netns successfully on `old-host`, and the failure is itself the useful finding:
```
$ unshare -Urn sh -c 'ip link set lo up; sleep 8' &
$ slirp4netns --configure --mtu=65520 $CPID tap0
setns(CLONE_NEWNET): Operation not permitted
$ slirp4netns --configure --mtu=65520 --userns-path=/proc/$CPID/ns/user $CPID tap0
setns(CLONE_NEWNET): Operation not permitted     # still fails
```
Cause, per §1.4's kernel source: `netns_install()` requires `CAP_SYS_ADMIN` in **both** the target netns's userns and the **caller's own current userns**. `slirp4netns` here runs as an unprivileged user in the *initial* userns, so the second check fails. The reason it works under Podman is that Podman **re-executes slirp4netns from inside the userns** rather than invoking it as an outside sibling. `[R]` **HIGH** — if we ever shell out to a network helper for a rootless container, **the helper must be launched from a process already inside the container's userns**, not from the daemon. That is a process-topology constraint → **Handoff to D2.**

### 8.3 passt / pasta — why it is better

`[F]` **HIGH** — Two names, one codebase: **passt** ("Plug A Simple Socket Transport") targets VMs and talks to qemu over a **UNIX socket** — no tap, no capabilities. **pasta** ("Pack A Subtle Tap Abstraction") targets network namespaces and *does* create a tap device in the namespace.
`[O]` **HIGH** — **Correcting a common misreading (including the premise I was given): pasta does use a tap device.** What it avoids is the tap for *local* traffic.

**Why it is better than slirp4netns — five concrete reasons:**

1. **No NAT: it copies the host's addressing into the namespace.** `[F]` HIGH — *"with default options, maps routing and addressing information to the namespace, avoiding any need for NAT."* DHCP hands the namespace **the same address as the host's upstream interface**, SLAAC derives the prefix from the host, and `--config-net` makes pasta set it all up over netlink, copying all non-link-local addresses and all routes from the host's uplink. Consequence: the container sees the *host's* IP and routes. No `10.0.2.x` fiction, no double NAT, and outbound connections need no translation at all.

2. **Zero-copy `splice()` bypass for local traffic.** `[F]` HIGH — for local TCP/UDP, pasta *"creates a socket in the destination namespace, with matching Layer-4 protocol… For TCP, data is forwarded between the originating socket and the new socket using the `splice(2)` system call, and for UDP, a pair of `recvmmsg(2)` and `sendmmsg(2)`"*. `splice()` moves data between fds **inside the kernel**, never through userspace. This is the single biggest architectural difference: for the common case of container↔host, pasta does not run an IP stack at all. Documented limit: *"Because it's not possible to bind sockets to foreign addresses, this bypass only applies to local connections and traffic."*

3. **It deliberately is *not* a full TCP stack.** `[F]` HIGH — *"Unlike `slirp4netns`(1), passt doesn't implement a full TCP stack: the TCP translation layer has no stateful data buffering and operates by reflecting one peer's observed parameters (congestion window size, acknowledged data, etc.) to the corresponding peer."* This is a feature, not a shortcut: no buffering means no buffer bloat, no duplicated congestion control fighting the real stacks on either side, and a far smaller attack surface. Cost: no SACK, no PAWS/timestamps, no IP fragmentation, no IPv6 jumbograms.

4. **Serious hardening, by construction.** `[F]` HIGH — no dynamic memory allocation after startup (`brk`/`mmap` blocked by seccomp); a **34-syscall (passt) / 43-syscall (pasta)** seccomp filter on x86_64; all capabilities dropped except `CAP_NET_BIND_SERVICE` if granted; user/mount/IPC/UTS/PID namespaces detached by default; no dependencies beyond libc; AppArmor and SELinux profiles shipped; ~5000 LoC target. slirp4netns's sandboxing is opt-in (`--enable-sandbox`, `--enable-seccomp`) and RootlessKit defaults both to `false`.

5. **A 65520-byte MTU by default.** `[F]` HIGH — *"the maximum 802.3 MTU minus the length of a 802.3 header, rounded to 32 bits"*, advertised via DHCP option 26 / NDP option 5. `[O]` HIGH — this is where most of the measured throughput advantage comes from; both stacks collapse at 1500.

`[F]` **HIGH** — Both are **single-threaded**: *"passt and pasta are currently single-threaded."*

`[C]` **HIGH** — **Podman 5.0.0 made pasta the default**, listed under Breaking Changes: *"The default tool for rootless networking has been swapped from `slirp4netns` to `pasta` for improved performance."* Podman's rationale: *"Pasta fully supports IPv6 and is architecturally secure (runs in a separate process, uses modern Linux mechanisms for isolation etc)."* `--net=pasta` first appeared in Podman 4.3.2.

`[C]` **HIGH** — **pasta and netavark are not competitors**, which is a common confusion: netavark is Podman's backend for **bridge/user-defined** networks; pasta is the default for rootless `--network=private`. Rootless custom networks use netavark *inside* Podman's rootless netns, plus `rootlessport` for host ports.

### 8.4 Port forwarding without privileges

`[F]` **HIGH** — **RootlessKit's `builtin` driver**, the mechanism in full (from `pkg/port/builtin/`):
1. The **parent** (host netns, unprivileged) does `net.Listen(proto, ParentIP:ParentPort)` and `accept()`s.
2. Per accepted connection it dials a UNIX socket served by the **child** inside the container netns and sends a length-prefixed JSON `Request{Type:"connect", Proto, IP, Port, SourceIP, SourcePort}`.
3. The **child** dials the container-side target *from inside the child netns* and sends the resulting socket **fd back as an `SCM_RIGHTS` cmsg**.
4. The parent wraps that fd and runs `bicopy()` — two goroutines doing `io.Copy` in opposite directions with half-close propagation. Source comment: *"NOTE: sendfile(2) cannot be used for sockets."*

`[O]` **HIGH** — So the fd-passing solves only *"who is allowed to create the container-side socket"*; **the payload is relayed in userspace by the parent.** It is a genuine userland L4 proxy — read/write per chunk per direction, plus one UNIX round trip and an fd pass per **new connection**. Hence the weak spot is connection rate and latency, not bulk throughput. But because it never touches a usermode IP stack, it is ~4× faster than forwarding through slirp4netns.

`[F]` **HIGH** — **Source-IP preservation.** Naively the container sees the proxy's address. Since RootlessKit v3.0 the parent forwards the real `SourceIP`/`SourcePort` and the child dials with `IP_TRANSPARENT` bound to the client's address, plus one-time policy routing (a table-100 "treat all addresses as local" rule, and `tcp_fwmark_accept=1` so replies route back to the transparent socket). Flag `--source-ip-transparent`, default true. **TCP only** — there is no `udp_fwmark_accept`, so enabling it for UDP would break UDP forwarding entirely.
`[F]` **HIGH** — Docker caveat: *"In the case of Rootless Docker, userland-proxy has to be disabled for propagating the source IP."*
`[O]` **MED** — the `socket` vs `tcp` port-driver mode I was asked about **does not exist** in current RootlessKit. What exists is per-protocol sub-drivers, the `--source-ip-transparent` boolean, and `tcp`/`tcp4`/`tcp6`/`udp`/… protocol strings. Treat the `socket`/`tcp` distinction as a confusion with Docker's `--userland-proxy` toggle.

`[F]` **HIGH** — Driver menu: `none` (default), `builtin`, `slirp4netns` (forwards inside libslirp via `add_hostfwd`, so source IP is native but all data crosses the single-threaded slirp loop), `implicit` (pasta's own port autodetection — fast, but *"may expose ports that were not intended to be reachable from the host"*), `pesto` (experimental, explicit pasta control), `gvisor-tap-vsock`.

### 8.5 Privileged ports — the widely-repeated claim is wrong

`[F]` **HIGH** — `Documentation/networking/ip-sysctl.rst`, verbatim:
> `ip_unprivileged_port_start - INTEGER`
> This is a per-namespace sysctl. It defines the first unprivileged port in the network namespace. Privileged ports require root or CAP_NET_BIND_SERVICE in order to bind to them. To disable all privileged ports, set this to 0. They must not overlap with the ip_local_port_range.
> Default: 1024

`[F]` **HIGH** — Confirmed in source: the entry lives in `ipv4_net_table[]` (the **per-netns** table, not the global `ipv4_table[]`), and `ipv4_net_table_dup()` `kmemdup`s it per netns, rebasing each `.data` pointer: `table[i].data += (void *)net - (void *)&init_net;`. Default set per-netns at creation: `net->ipv4.sysctl_ip_prot_sock = PROT_SOCK;` (1024).

`[F]` **HIGH** — **And the capability check is per-netns too**, `net/ipv4/af_inet.c`:
```c
if (!(flags & BIND_NO_CAP_NET_BIND_SERVICE) &&
    snum && inet_port_requires_bind_service(net, snum) &&
    !ns_capable(net->user_ns, CAP_NET_BIND_SERVICE))
        goto out;                  /* -EACCES */
```
with `inet_port_requires_bind_service(net, port) { return port < READ_ONCE(net->ipv4.sysctl_ip_prot_sock); }`. **Both the threshold and the capability are evaluated against the socket's netns and that netns's owning user namespace.**

`[O]` **HIGH** — Verified on `old-host`:
```
host value: 1024
$ unshare -Urn sh -c '
    echo 80 > /proc/sys/net/ipv4/ip_unprivileged_port_start   # exit 0
    cat /proc/sys/net/ipv4/ip_unprivileged_port_start          # 80
    ip link set lo up
    python3 -c "import socket; s=socket.socket(); s.bind((\"127.0.0.1\",80)); print(\"BIND_80_OK\")"'
BIND_80_OK
host value AFTER: 1024        # unchanged
```

`[R]` **HIGH** — **So "rootless containers can't use port 80" is wrong as stated, and it matters for our design.** Inside a rootless container's netns — owned by the userns where we are uid 0 — `ns_capable(net->user_ns, CAP_NET_BIND_SERVICE)` **succeeds**, so nginx binds `:80` with no sysctl change at all. The **only** thing that fails is binding a host-side port < 1024 **in the initial netns**. Three remedies for that, in preference order:
1. **Publish to a host port ≥ 1024.** Free, no privilege, no host change. Default to this.
2. **`net.ipv4.ip_unprivileged_port_start=0` on the host.** A persistent global host mutation — we should never do this silently, only document it. passt's docs call this *"the recommended way"* of the two.
3. **`CAP_NET_BIND_SERVICE` as a file capability on our helper binary.** passt's docs advise against it: *"in general, not the recommended way, because passt and pasta might be used as vector to effectively use this capability from another process."* Agreed.
`[R]` **HIGH** — and set `ping_group_range` inside the container netns (§8.2) so `ping` works. It is per-netns and free.

### 8.6 `lxc-user-nic` — the setuid escape hatch

`[F]` **HIGH** — *"a setuid-root program with which unprivileged users may manage network interfaces for use by a lxc container."* It consults `/etc/lxc/lxc-usernet` (`USERNAME TYPE BRIDGE COUNT`, e.g. `penguin veth lxcbr0 1`), tracks usage in `/run/lxc/nics`, and *"ensures that the calling user is privileged over the network namespace to which the interface will be attached."*
`[O]` **HIGH** — This is exactly §8.1's requirement bought with a setuid bit: the helper runs with `CAP_NET_ADMIN` in `init_net` and performs the two operations the caller cannot — create the pair in `init_net`, enslave one end to the host bridge, move the other into the caller's netns.
`[F]` **HIGH** — Costs: less secure by construction, needs host admin configuration, **no IPv6**, `COUNT` limits concurrency, and DHCP-based addressing takes seconds.
`[R]` **HIGH** — **Do not build a setuid helper.** For a project whose whole premise is understanding the machinery, a setuid binary is the one component where a mistake is a local root exploit. If someone needs kernel-speed rootless networking, `--network=host` or accepting the usermode cost are honest answers; a setuid helper we wrote ourselves is not.

### 8.7 Performance cost of usermode networking

`[F]` **HIGH** — RootlessKit CI, iperf3 child→parent (run dated 2026-07-26):

| driver | MTU 1500 | MTU 65520 |
|---|---|---|
| `slirp4netns` | 1.69 Gbps | 8.11 Gbps |
| `slirp4netns` + sandbox + seccomp | 1.68 Gbps | 8.32 Gbps |
| `vpnkit` | 0.14 Gbps | unsupported |
| `pasta` | 0.24 Gbps | 31.9 Gbps |
| `gvisor-tap-vsock` | 2.46 Gbps | 8.75 Gbps |
| `lxc-user-nic` (setuid, kernel path) | 49.1 Gbps | 50.7 Gbps |
| **rootful veth (reference)** | **49.3 Gbps** | **50.8 Gbps** |

`[F]` **HIGH** — RootlessKit **port** drivers, iperf3 parent→child (2026-08-04): `slirp4netns` 7.56 · `builtin` 29.3 · `implicit` 39.9 · `pesto` 45.2 · `gvisor-tap-vsock` 3.77 Gbps.

`[F]` **HIGH** — passt's own CI: pasta **via tap**, TCP IPv4 ns→host: 2.5 / 4.1 / 4.9 / **17.4** Gbps at MTU 1500 / 4000 / 16384 / 65520. pasta **local (splice bypass)**, MTU 65535: **64.2** Gbps ns→host, **71.2** Gbps host→ns; RR latency 24–25 µs, CRR 79–83 µs.

**Synthesis** `[O]` **HIGH**:
- **At a realistic 1500-byte MTU, kernel veth is ~30× usermode networking** (49 vs 1.7 Gbps). Usermode only closes to ~1.5–6× by using a 64K MTU, which is available only on the internal tap link, not on the real path to the internet.
- **`splice()` changes the picture completely** where it applies: pasta's local bypass hits 64–71 Gbps, i.e. *at or above* kernel veth. That is the strongest argument for pasta over slirp4netns — for container↔host traffic it is not a slower path, it is a faster one.
- Port forwarding through a userspace **L4 proxy** (29.3 Gbps) beats forwarding through a **usermode IP stack** (7.56 Gbps) by ~4×, because the proxy skips the packet path entirely — at the cost of source-IP fidelity, recovered only for TCP via `IP_TRANSPARENT`.
- slirp4netns's hardening is essentially **free** (1.68 vs 1.69 Gbps). There is no performance excuse for leaving `--enable-sandbox`/`--enable-seccomp` off.
- `[O]` **MED — discrepancy flagged:** RootlessKit measures pasta at **0.24** Gbps @1500 while passt's own CI measures **2.5** Gbps for the same path — a 10× disagreement. RootlessKit still labels `--net=pasta` experimental. Treat 0.24 as an integration artifact, not a property of pasta. **Unresolved**; what would settle it: running both benchmarks on one host.

`[R]` **HIGH** — **Rootless plan for us: do not write a usermode network stack. Shell out to `pasta`.** Reasoning: (a) rootless is explicitly phase 2 for us; (b) a usermode TCP/IP stack is a multi-month project with a large security surface, and it is *not* the part of container engines we set out to understand; (c) `pasta` is ~5000 LoC, seccomp-sandboxed, actively maintained, and the *default* in Podman 5 — the ecosystem has converged; (d) shelling out is acceptable here in a way it is not for netlink, because the interface is a long-lived process with a stable CLI, not a per-operation API we need typed errors from. Tradeoff: a runtime dependency on a binary the user must install, and we do not learn how a usermode stack works. Accept both. Per §8.2, **launch it from inside the container's userns**, not from the daemon.
`[R]` **MED** — For rootless port publishing, implement the **`builtin`-style accept-and-relay proxy ourselves** (§8.4) rather than depending on RootlessKit: it is ~100 lines, it is the same accept/connect/copy loop we already need for the IPv4-loopback and IPv6 cases in §4.4, and it is 4× faster than routing ports through the usermode stack. Skip `IP_TRANSPARENT` initially; add it when someone needs real client IPs, and only for TCP.

---

## 9. Host state that outlives our process

> **Numbering note.** §1.6, §2.6, §3.3, §4.4 and §5.4 all forward-reference "§10" for cleanup / partial state / reconciliation, and §6.5 references "§11" for sticky IPAM. Those references land on **this section (§9)** and on **§10 (IPAM)** respectively. The stale pointers are listed in §14 for the verification pass; I have not edited the earlier text.

This is the section that changes the shape of our design rather than adding a feature. Every other section describes what to build; this one describes what happens when we don't finish building it.

### 9.1 The inventory: what dies with us, what doesn't

`[F]` **HIGH** — Complete accounting of the host state a single container start touches, and who frees it:

| Object | Freed when our process dies? | Freed by | Cost of leaking it |
|---|---|---|---|
| container-side veth (`eth0`) | **yes** | kernel, when netns is freed (§1.6) | none |
| host-side veth | **yes** | kernel, deleting either end deletes both (`veth(4)`) | none |
| the netns itself | **only if nothing pins it** | last process exit / last fd close / **umount of the bind mount** | *the* leak: pins veth, ifindexes, sysctls, nft tables forever |
| nsfs bind mount at `/run/mydocker/netns/<id>` | **no** | `umount2()` + `unlink()`, by us, explicitly | pins the netns above |
| the bridge | **no** | `RTM_DELLINK`, by us, explicitly | a permanently-UP device holding an address and an on-link route |
| the bridge's address + kernel connected route | **no** | dies with the bridge | burns a subnet from our pool (§10.3) |
| `nat/POSTROUTING` MASQUERADE for the subnet | **no** | rule delete, by us | masquerades a subnet nobody owns |
| `raw/PREROUTING` anti-spoof rule per container IP (§4.5) | **no** | rule delete, by us | drops traffic to an address that may be reassigned |
| DNAT + filter ACCEPT per published port | **no** | rule delete, by us | **security-relevant**: see §9.4 |
| IPAM allocation | depends entirely on where we put it | see §10 | pool exhaustion, or worse, double-allocation |
| userland-proxy helper process | **no** — reparented to pid 1 | kill, by us | holds a host port hostage; `EADDRINUSE` forever |
| `net.ipv4.ip_forward = 1` | **no** | never; we must not revert it blindly (§1.1) | the host is now a router |
| `filter/FORWARD` policy DROP (§3.3) | **no** | never | can break the admin's own routing |
| generated `/etc/hosts`, `/etc/resolv.conf` + their bind mounts | **no** | umount + unlink, by us | mount-table clutter, D3's domain |
| **conntrack entries** | **no** | timeout, or explicit flush | see §9.5 — silently defeats *correct* rules |

`[R]` **HIGH** — Two conclusions fall straight out of that table and they should drive the code:
1. **veth needs no cleanup code at all** (already established in §1.6). Do not write it. Every line of veth teardown you write is a line that can fail and leave you in a worse state than doing nothing.
2. **Every remaining row is a thing we wrote and only we can unwrite.** That means our teardown path is not a nicety — it is the only garbage collector on the host. And a teardown path that only runs on the happy path is not a garbage collector, it is a hope.

### 9.2 A live leak, on the production host, found by accident

`[O]` **HIGH** — `host-a` (`HOST-A`, Ubuntu 24.10, kernel 6.11.0-29-generic, Docker 28.3.1), read-only inspection. §3.1 recorded three bridges without remarking on it. Here is what the third one actually is:

```
$ docker network ls
NETWORK ID     NAME                DRIVER    SCOPE
9d68a0cc943b   webapp_default   bridge    local     -> 172.18.0.0/16
f8b0597542e5   bridge              bridge    local     -> 172.17.0.0/16 (docker0)
7d220e3d8bb1   host                host      local
81f2952c2508   none                null      local
                                             ^^^ nothing owns 172.19.0.0/16

$ ip -d link show br-0d4cf39c58f7
26: br-0d4cf39c58f7: <NO-CARRIER,BROADCAST,MULTICAST,UP> mtu 1500 state DOWN
    bridge ... stp_state 0 ageing_time 30000 fdb_n_learned 0 ...
$ ip -o addr show br-0d4cf39c58f7
26: br-0d4cf39c58f7    inet 172.19.0.1/16 brd 172.19.255.255 scope global
$ bridge link show
    ... five ports, ALL master br-9d68a0cc943b, none on br-0d4cf39c58f7

$ iptables -t nat -S POSTROUTING
-A POSTROUTING -s 172.19.0.0/16 ! -o br-0d4cf39c58f7 -j MASQUERADE     <-- ORPHAN

$ iptables -t raw -S
-A PREROUTING -d 172.19.0.2/32 ! -i br-0d4cf39c58f7 -j DROP            <-- ORPHAN
-A PREROUTING -d 172.19.0.3/32 ! -i br-0d4cf39c58f7 -j DROP            <-- ORPHAN
(plus the five live 172.18.x rules)

$ iptables -t filter -S | grep -E '172\.19|br-0d4cf39c58f7'
(empty)
```

Read that carefully, because the pattern is the finding:

- A **bridge device** exists, administratively UP, holding `172.19.0.1/16` and therefore an on-link route, with **zero ports** and `fdb_n_learned 0`.
- **No Docker network claims that subnet.** The network `0d4cf39c58f7` is gone from Docker's store.
- The **nat MASQUERADE** rule for the dead subnet is still installed.
- **Two `raw/PREROUTING` anti-spoof rules survive for two container IPs** (`172.19.0.2`, `172.19.0.3`) belonging to containers that no longer exist on a network that no longer exists. These are per-container rules (§4.5), so this is not one leak, it is one leak *per container that ever ran on that network*.
- The **filter-table** rules for that bridge are **gone**.

`[O]` **HIGH** — and the daemon is not configured for live-restore, which matters for §9.3: `docker info` → `ServerVersion = 28.3.1`, `LiveRestoreEnabled = False`, `FirewallBackend = {Driver: iptables}`, `DefaultAddressPools = None`.

`[R]` **HIGH** — The asymmetry (filter cleaned, nat + raw + device leaked) is the whole lesson, and §9.3 explains the mechanism. **Docker's filter chains are rebuilt from scratch at daemon start, so anything that is not in the store disappears from them; the nat rules, the raw rules, and the bridge device are written once at network-create time and are only removed by an explicit network-delete that reaches completion.** Once the store record is gone, nothing can ever find them again.

`[LOW→MED]` on the *provenance* of this particular orphan — I did not establish whether it came from a `docker network rm` that partially failed, a daemon kill during removal, or a `docker-compose down` race. **What would settle it:** `journalctl -u docker | grep -i '0d4cf39c58f7'`, which I did not run (read-only budget, and the logs have almost certainly rotated since; the bridge's ifindex 26 vs. the live bridge's 67 says it is old). The *state* is `[O]` HIGH regardless of how it got there, and the state is the point: **this is what a naive implementation leaves behind, observed in production, on a host that has never had a networking outage.**

### 9.3 What Docker actually does about it — primary source

`[C]` **HIGH** — Docker has no background reconciler. It has a **startup GC**, and its shape is worth copying almost exactly. From `libnetwork.New()`, `moby/daemon/libnetwork/controller.go`, verbatim and in order:

```go
	// Reserve pools first before doing cleanup. Otherwise the
	// cleanups of endpoint/network and sandbox below will
	// generate many unnecessary warnings
	c.reservePools()

	if err := c.sandboxRestore(c.cfg.ActiveSandboxes); err != nil {
		log.G(ctx).WithError(err).Error("error during sandbox cleanup")
	}

	// Cleanup resources
	if err := c.cleanupLocalEndpoints(); err != nil {
		log.G(ctx).WithError(err).Warnf("error during endpoint cleanup")
	}
	c.networkCleanup()
	...
	c.setupPlatformFirewall()
```
Source: <https://github.com/moby/moby/blob/master/daemon/libnetwork/controller.go>

Four distinct mechanisms, and each answers a different question:

**1. `reservePools()` — rebuild derived state by replay.** It walks the persisted networks, re-runs `ipamAllocate()` for each pool, then walks each network's *persisted endpoints* and re-reserves each address. The IPAM index is not persisted; it is reconstructed. Details in §10.2.

**2. `sandboxRestore(activeSandboxes)` — the liveness join.** For every persisted sandbox: if its id is in `activeSandboxes` (populated by the daemon only under live-restore, from containers found still running), restore it; **otherwise**, `moby/daemon/libnetwork/sandbox_store.go`:
```go
		if !isRestore {
			log.G(ctx).Info("Removing stale sandbox")
			if err := sb.delete(context.WithoutCancel(ctx), true); err != nil {
				log.G(ctx).WithError(err).Warn("Failed to delete sandbox while trying to clean up")
			}
			continue
		}
```
Note `sb.delete(ctx, true)` — the `true` is `force`. `[F]` **HIGH** — the consequence, which explains a great deal of real-world Docker behaviour: **without live-restore, every sandbox is "stale" at daemon start and is force-deleted.** That is why restarting `dockerd` tears down and rebuilds container networking, and it is why the `--live-restore` failure reports in §9.6 are all about the network.

**3. `cleanupLocalEndpoints()` — mark and sweep.** `moby/daemon/libnetwork/endpoint.go`: build the set of endpoint ids referenced by *restored* sandboxes, then for every network, delete every persisted endpoint **not** in that set, logging `Removing stale endpoint`. This is a mark-and-sweep GC whose root set is the live sandboxes. Structurally identical to CNI's `GC` verb (§9.4).

**4. `networkCleanup()` — tombstone retry.** `moby/daemon/libnetwork/store.go`, in full:
```go
func (c *Controller) networkCleanup() {
	for _, n := range c.getNetworksFromStore(context.TODO()) {
		if n.inDelete {
			log.G(context.TODO()).Infof("Removing stale network %s (%s)", n.Name(), n.ID())
			if err := n.delete(true, true); err != nil {
				log.G(context.TODO()).Debugf("Error while removing stale network: %v", err)
			}
		}
	}
}
```

`[F]` **HIGH** — **And there is the bug that produced §9.2's orphan.** `networkCleanup` can only retry a delete for a network whose record still exists with `inDelete` set. The record is the *only* handle on the bridge name, the subnet, and therefore the rules. If the record is removed while host state remains, the garbage is **unreachable**: there is no tombstone, no name, nothing to sweep. `docker network ls` cannot show it, `docker network rm` cannot target it, `docker network prune` cannot find it. The only recovery is a human with `ip link delete` and `iptables -D`.

`[R]` **HIGH** — **The invariant, stated as a rule we will actually enforce: the record must outlive the host state it describes.** Concretely: write the tombstone *before* the first host mutation of a teardown, and delete the record *after* the last host mutation succeeds. A delete that removes the record first is not "optimistic", it is a leak generator. This is one `if` in the right order and it is the difference between recoverable and unrecoverable garbage.

`[C]` **MED** — On live-restore's actual limits, from Docker's own docs: *"The live restore option only works to restore containers if the daemon options, such as bridge IP addresses and graph driver, didn't change"* and it is *"only supported when installing patch releases (`YY.MM.x`), not for major (`YY.MM`) daemon upgrades"*. Source: <https://docs.docker.com/engine/daemon/live-restore/>. The docs are conspicuously silent on iptables. `[C]` **LOW→MED** — the field reports are not: rule recreation dominating restart time on hosts with many published ports, duplicated `nat/POSTROUTING` rules across restarts, and container-netns NAT tables (the §6.1 DNS rules) coming back broken. Sources: <https://github.com/moby/moby/issues/34906>, <https://github.com/moby/moby/issues/31647>, <https://forums.docker.com/t/live-restore-does-not-restore-iptables-rules/136677>. `[LOW]` because these are issue reports, not measured by me. **What would settle it:** a local VM with 50 published ports, `systemctl restart docker` with and without live-restore, timing the gap and diffing `iptables-save` before/after.

### 9.4 What CNI mandates, and the verb it had to add

`[F]` **HIGH** — The CNI spec's cleanup contract, verbatim (<https://github.com/containernetworking/cni/blob/main/SPEC.md>):
- *"DEL command invocations are always considered best-effort - plugins should always complete a DEL action without error to the fullest extent possible, even if some resources or state are missing."*
- *"Plugins MUST accept multiple DEL calls for the same (CNI_CONTAINERID, CNI_IFNAME) pair, and return success if the interface in question, or any modifications added, are missing."*
- *"The container runtime is responsible for cleanup of the container's network namespace."*
- *"The container runtime must ensure that add is eventually followed by a corresponding delete. The only exception is in the event of catastrophic failure, such as node loss."*
- and on DEL after the namespace is gone: *"The plugin MAY assume that the isolation domain (e.g. network namespace) has been deleted, and thus any resources (e.g. network interfaces) therein have been removed."*

Four things to extract:
1. **`DEL` is idempotent by *specification*, not by politeness.** "MUST accept multiple DEL calls … and return success if the interface is missing." A teardown that returns an error because the thing is already gone is a spec violation in CNI and a bug in us: the retry loop above it will spin forever on garbage it has already collected.
2. **The netns is the *runtime's* to destroy, never the network layer's.** Which matches §1.6's finding exactly: destroy the netns and the interfaces garbage-collect themselves, so the plugin genuinely does not need to touch them.
3. **`DEL` is *required* to eventually happen** — the runtime owns that guarantee, and the spec names exactly one exception (node loss). So "the supervisor was SIGKILLed" is not an excuse the spec grants; it is a case we must handle.
4. **Best-effort means partial success is the normal outcome.** DEL must therefore not stop at the first failure. Ours must run every step, collect the errors, and report them together.

`[F]` **HIGH** — And CNI, after a decade of this, concluded that idempotent DEL is **not sufficient** and added a garbage-collection verb in spec **1.1.0**: *"The GC command provides a way for runtimes to specify the expected set of attachments to a network. The network plugin may then remove any stale resources related to attachments that do not exist in this set."* Same source.

`[R]` **HIGH** — **That is the single most important design signal in this section.** The de-facto standard for container networking, written by people who had DEL-must-be-idempotent from day one, still had to add a *mark-and-sweep pass driven by the runtime's live set* — because idempotent DEL only helps when someone calls DEL, and the interesting failures are precisely the ones where nobody does. Note also that CNI's GC and Docker's `cleanupLocalEndpoints()` are the same algorithm arrived at independently: **hand the network layer the authoritative live set; it deletes everything else it owns.** We should build that from the start, not after the first leak. It is cheaper than it sounds — see §9.7.

`[C]` **MED** — host-local's own leak, as a warning about how fine-grained this gets: <https://github.com/containernetworking/plugins/issues/498> reports that when the `O_EXCL` create of an allocation file succeeds but the subsequent write and the compensating `os.Remove` both fail, an **empty** allocation file is left behind, and kubelet's retry loop then creates one per attempt until the range is exhausted. The reporter's fix is the right instinct and worth stealing: *"We should check if IP file is empty to determine whether an IP can be allocated. This not only prevents exhausting all free IPs, but also helps to recover leaked IP files."* i.e. **make the half-written state self-identifying so the sweep can recognise it.**

### 9.5 The leak nobody mentions: conntrack entries defeat correct rules

`[F]` **HIGH** — This one is not about teardown at all, and it is the subtlest item in §9.1's table. From §4.2 we already know that NAT rules are consulted **only when a conntrack entry is created**; every subsequent packet of a flow is translated from the existing entry without re-traversing the `nat` table. The corollary is unpleasant: **a conntrack entry created *before* our rules existed will keep bypassing them for the entry's whole lifetime.**

`[C]` **HIGH** — Docker hit this three times and now flushes conntrack explicitly. From `moby/daemon/libnetwork/drivers/bridge/bridge_linux.go`, verbatim comment:
```
// clearConntrackEntries flushes conntrack entries matching endpoint IP address
// or matching one of the exposed UDP port.
// In the first case, this could happen if packets were received by the host
// between userland proxy startup and iptables setup.
// In the latter case, this could happen if packets were received whereas there
// were nowhere to route them, as netfilter creates entries in such case.
// This is required because iptables NAT rules are evaluated by netfilter only
// when creating a new conntrack entry. When Docker latter adds NAT rules,
// netfilter ignore them for any packet matching a pre-existing conntrack entry.
// As such, we need to flush all those conntrack entries to make sure NAT rules
// are correctly applied to all packets.
// See: #8795, #44688 & #44742.
```
and on the narrower variant, used when ports change on a running container:
```
// Unlike [clearConntrackEntries], it does not flush by the endpoint's IP
// address, because that would tear down established flows to the endpoint's
// other, unmodified ports. It's used when ports are added to or removed from an
// endpoint that's already joined to a sandbox. Only UDP entries need clearing;
// a stale TCP flow to a just-changed port is reset by its peer.
```
Source: <https://github.com/moby/moby/blob/master/daemon/libnetwork/drivers/bridge/bridge_linux.go>

`[R]` **HIGH** — **Flush conntrack for the container's addresses at the end of setup, not (only) at teardown.** Reasoning: the failure this prevents is a container that is configured perfectly and still unreachable, or reachable-but-unmasqueraded, with a completely clean ruleset — the worst possible debugging experience. Note the two race windows Docker names: a packet arriving between our helper's `bind()` and our rule install, and a packet arriving at an address that has nothing listening (netfilter creates an entry anyway). Both windows exist in our design too.
`[R]` **MED** — **UDP only for the port-change case, IP-wide for setup and teardown.** Docker's reasoning is sound and worth copying verbatim: an obsolete TCP flow gets an RST from its peer and heals itself; a UDP "flow" has no such mechanism and will happily keep translating to a dead address until it times out.
`[R]` **MED** — Mechanism: `NETLINK_NETFILTER` / ctnetlink `IPCTNL_MSG_CT_DELETE` with a filter, which is a *fourth* netlink family beyond the three in §2.6. Crate coverage is thin (§12.4). **Cheapest honest MVP: shell out to `conntrack -D -d <ip>`**, mark it as a shortcut, and note the ceiling (an exec, and the `conntrack` binary is not installed by default on minimal images — detect and warn rather than fail).

### 9.6 Stale netns bind mounts, empirically

`[O]` **HIGH** — `old-host` (Rocky 8.8, kernel 4.18.0-425.3.1.el8, unprivileged uid 900000000). Everything below runs inside `unshare -Urmn` with a private tmpfs on `/run`, so nothing touches the real host and it all vanishes on exit. This simulates dying in the two-syscall window §1.3 describes: `open(O_CREAT|O_EXCL)` has created the placeholder, the `mount --bind` has not happened yet.

```
$ unshare -Urmn sh -c '
    mount -t tmpfs tmpfs /run; mkdir -p /run/netns
    ip netns add good              # a real one, bind-mounted
    : > /run/netns/crashed         # simulate death after placeholder, before bind mount
    ...'

[A] ip netns list:
    Error: Peer netns reference is invalid.
    Error: Peer netns reference is invalid.
    crashed
    good
[B] ip netns add crashed:      rc=1   Cannot create namespace file "/var/run/netns/crashed": File exists
[C] ip netns exec crashed true: rc=255 setting the network namespace "crashed" failed: Invalid argument
[D] ip netns delete crashed:   rc=0   (no output)
[E] ls /run/netns after delete: good
[F] nsfs mounts in mountinfo:  1
```

Five things to read off this, all of which map onto our design:

1. **`ip netns list` reports the corpse as a namespace.** `crashed` and `good` are listed identically. The directory listing cannot distinguish a live namespace from a zero-byte placeholder — because `ip netns list` is `readdir()` and nothing more (§1.2).
2. **It is not silently identical, though: `ip netns list` also emits `Error: Peer netns reference is invalid.`** — once per entry, because it tries to resolve each file's nsid and fails on the non-nsfs one. So the *detection signal* exists; it is just delivered as noise on stderr that nobody reads.
3. **The retry is blocked.** `ip netns add crashed` fails `rc=1` with `File exists` — this is the `O_EXCL` in iproute2's own sequence biting the next attempt. A naive implementation that retries container creation with a deterministic name (which we want, for §9.7's self-describing naming) **wedges permanently** on its own corpse.
4. **Using it fails differently and worse.** `ip netns exec crashed` → `Invalid argument`, i.e. `setns(2)` returning `EINVAL` because the fd refers to a tmpfs regular file, not an nsfs inode. `[R]` — a plain `EINVAL` from `setns` on a path we ourselves created is a **positive identification of a stale placeholder**, and it is exactly the check our recovery path should make: `open` the path, `setns(fd, CLONE_NEWNET)`, and treat `EINVAL` as "this is garbage, unlink it and proceed" versus success as "this namespace is real".
5. **`ip netns delete` recovers it, rc=0.** It attempts the umount (which fails, harmlessly) and then unlinks. `[F]` **HIGH** — so `umount2()` failing with `EINVAL`/`ENOENT` **must not abort our teardown**; it is the expected result on a placeholder, and treating it as fatal converts a recoverable leak into a permanent one. Note `[F]`'s ordering requirement: `umount2` then `unlink`, and unlink must run even if umount failed. This is the concrete shape of CNI's "best-effort DEL".

`[O]` **HIGH** — For contrast, the healthy case on `host-a`: five bind mounts in `/run/docker/netns/`, five running containers (`docker ps -q | wc -l` = 5), `lsns -t net` = 6 rows (five plus the host). No leak. `[R]` — **that count comparison is our cheapest possible health check**, and it belongs in `mydocker info`: `#nsfs mounts under our directory` vs `#containers in our store with a network`. A mismatch is a leak, in either direction, and it is two `readdir`s.

### 9.7 What this means for our lifecycle design

`[R]` **HIGH** — **Ownership: exactly one owner per object, and it is never "the container".** Three owners, and the distinction is load-bearing:

| Object | Owner | Lifetime |
|---|---|---|
| netns + its bind mount, veths, container addresses/routes | the **network sandbox** (already a first-class entity per §7.3) | refcounted; last detach frees it |
| bridge, subnet, MASQUERADE rule, `ip_forward` | the **network** | created on first use, freed on explicit network delete only |
| DNAT/filter rules, proxy helper, anti-spoof rule, conntrack flush | the **attachment** (sandbox × network) | strictly inside the sandbox's lifetime |

The container is a *user* of a sandbox, not the owner of one. §7.3 already reached this conclusion for pod semantics; §9 reaches it again for cleanup, which is a good sign it is right.

`[R]` **HIGH** — **Setup must be idempotent, because the recovery path is "run setup again".** This is not a theoretical nicety; it is precisely what netavark does. `netavark firewall-reload` reads its persisted intent and re-runs the *same* `setup_network` / `setup_port_forward` calls used for a fresh container (`netavark/src/commands/firewall_reload.rs`):
```rust
    let conf = read_fw_config(config_dir).wrap("read firewall config")?;
    if let Some(conf) = conf {
        let fw_driver = get_supported_firewall_driver(Some(conf.driver))?;
        for net in conf.net_confs { fw_driver.setup_network(net, conn)?; }
        for port in &conf.port_confs { fw_driver.setup_port_forward(port.into(), conn)?; }
    }
```
Source: <https://github.com/containers/netavark/blob/main/src/commands/firewall_reload.rs>
There is no separate "restore" code path. There is one setup path, and it is safe to call twice. `[R]` — **we should have exactly one setup path too.** A second, subtly-different restore path is the classic way to get a system that works on first boot and corrupts itself on recovery.

`[R]` **HIGH** — **Persist declarative intent, separately from and *before* the host mutation.** netavark's layout, from `netavark/src/firewall/state.rs`, is a good model and it is deliberately dumb — files, not a database:
```
$config/firewall/
    firewall-driver          -> name of the firewall driver
    networks/$netID          -> network config setup
    ports/$netID_$conID      -> port config
    firewall-reload.lock     -> flock, held while reading/removing
```
and the lock's rationale, verbatim from the source comment, is exactly the race we will have:
```rust
    /// This file is required to ensure that remove_fw_config is not racing against
    /// the firewall reload service, i.e. without it would be possible that we read
    /// the config files and then during re-adding the rules the file got removed.
    /// This leaves a chance that the service will add rules that should not be added
    /// anymore.
```
Source: <https://github.com/containers/netavark/blob/main/src/firewall/state.rs>
`[R]` — steal all three ideas: **(a) intent as one small file per object, keyed by id; (b) the driver name recorded alongside, so recovery knows which backend wrote the rules; (c) a single flock held across "read intent + apply" and across "remove intent + unapply", so a concurrent reconcile cannot resurrect a container we are deleting.** Point (c) is the non-obvious one and it is a real bug we would otherwise ship.

`[R]` **HIGH** — **Make the host self-describing, so we can find our garbage without our own state.** This is the belt to the intent-file's braces, and it is what would have saved §9.2's orphan. Every object we create carries our identity and the id of the thing it belongs to:
- netns bind mount: `/run/mydocker/netns/<container-id>` — already decided in §1.2, and now doing real work.
- host veth: `mdk-<12 hex of container id>`. `[F]` **HIGH** — the budget is tight: `IFNAMSIZ` is 16 *including* the NUL, so **15 usable characters**. `[O]` — Docker uses exactly all of it for bridges: `br-0d4cf39c58f7` is 3 + 12 = 15. Our `mdk-` + 11 hex = 15. Pick the prefix once and never change it, because it is a data format.
- bridge: `mdk-br-<8 hex of network id>`.
- nft: **one table, `inet mydocker`, and nothing outside it** (§5.4). This is where native nftables pays off twice: `nft delete table inet mydocker` is a complete, atomic, total reset of every rule we ever wrote, with no rule-text matching and no risk of deleting somebody else's rule. There is no iptables equivalent.
- proxy helper: `argv[0]` contains the container id, and it dies when its socket closes.

`[R]` **HIGH** — **The reconcile pass, and it is small.** On daemon start (or on the first CLI invocation after boot, if D2 goes daemonless), with the intent lock held:
1. Enumerate reality: `readdir /run/mydocker/netns/`; `RTM_GETLINK` dump filtered to names matching `mdk-*`; `nft list table inet mydocker`; `readdir` our intent directory; grep `/proc/*/cmdline` for our proxy helpers. Every one of these is cheap and none of them needs our state to be intact.
2. For each netns bind mount: `open` + `setns` probe. `EINVAL` ⇒ stale placeholder ⇒ `unlink`. Success ⇒ real; check whether the container id is in our live set.
3. Compute the live set from the container store (D2 owns this), then **sweep**: any netns/veth/rule/helper whose id is not live gets torn down with the ordinary idempotent teardown. This is CNI's `GC` and Docker's `cleanupLocalEndpoints`, in about 80 lines.
4. Rebuild the nft table from intent for the live set, atomically. Because the table is ours alone and the transaction is atomic (§5.4), the correct implementation is **delete the table and re-add it from intent in one transaction** — not a diff. Diffing rules is where everyone else's bugs live.
`[R]` **MED** — Do **not** sweep the *bridge* on the same trigger. A bridge with zero ports is normal (`[O]`: `docker0`, §3.1) and deleting a network's bridge because it is momentarily idle would break `docker run` on the very next command. Bridges are freed by network delete only — but a `mydocker network prune` should then be able to find them by our naming convention *even when the record is gone*, which is exactly the recovery §9.2's Docker cannot do.

`[R]` **HIGH** — **Ordering, written out, because the order is the whole design.**

Setup (each step idempotent, each recording intent before mutating):
```
1. flock the network            5. veth pair, peer directly into the netns (§2.6 op 4)
2. allocate IP (§10)            6. addresses, routes, lo up
3. write intent record          7. nft transaction: masq + publish + anti-spoof  (ATOMIC)
4. create netns + bind mount    8. flush conntrack for the address (§9.5)
                                9. mark the record ready
```
Teardown (best-effort, never abort early, collect all errors):
```
1. flock; mark record deleting   4. kill proxy helpers
2. umount2(netns, MNT_DETACH)    5. flush conntrack for the address
   + unlink  <-- FIRST, and it   6. free the IPAM allocation
   garbage-collects both veths    7. delete the record  <-- LAST
   (§1.6)
3. nft transaction removing this
   attachment's rules (ATOMIC)
```
`[R]` **HIGH** — Step 2 being first is §1.6's corollary and it is counter-intuitive enough to be worth a comment in the code: the unmount is not filesystem hygiene, it is the operation that deletes the interfaces. Step 7 being last is §9.3's invariant. Everything between them can fail and be retried, because a record in `deleting` state is a tombstone the reconcile pass will find.

`[R]` **HIGH** — **`mydocker rm` on a container whose supervisor died mid-setup: the requirement is that it does not need the supervisor.** Spelled out, because this was the explicit question:
- The record exists and is `ready` ⇒ ordinary teardown.
- The record exists and is `creating` or `deleting` ⇒ **run the ordinary teardown anyway.** Every step tolerates its object being absent (`ENOENT` from unlink, `EINVAL` from umount, "no such table/rule" from nft, "no such link" from `RTM_DELLINK`, an IPAM release of an address that is already free). This is the *entire* payoff of idempotent teardown, and it is why CNI made it mandatory.
- No record at all, but host state matching our naming convention ⇒ the reconcile sweep (§9.7 step 3) handles it, and `mydocker network prune` must be able to trigger it on demand.
- **`mydocker rm` must never return an error merely because something was already gone.** Per CNI: *"return success if the interface in question, or any modifications added, are missing."* It should return an error only if something we still own could not be removed — and then it must leave the tombstone in place so the next attempt can retry.

`[R]` **MED** — **What we will still not recover, stated honestly.** `ip_forward=1` and a `FORWARD` policy of DROP are global host mutations we deliberately do not revert (§1.1, §3.3): we cannot distinguish "we set this" from "the admin set this", and guessing wrong breaks the host's own routing. `[R]` — record in our own state that *we* were the one to change them, log loudly at the time, and surface it in `mydocker info` — but never auto-revert. A container engine that quietly turns off IP forwarding on uninstall is worse than one that leaves it on.

---

## 10. IPAM

Two separate allocation problems that get conflated. **Pool allocation**: picking a subnet for a network, once, where the adversary is the *host's own* routing table and the user's VPN. **Address allocation**: picking an address inside that subnet, per container, where the adversary is our own concurrency. They have different failure modes, different state, and different correct answers.

### 10.1 What Docker allocates from — the actual default pools

`[C]` **HIGH** — Verbatim from `moby/daemon/libnetwork/ipamutils/utils.go`:
```go
	localScopeDefaultNetworks = []*NetworkToSplit{
		{netip.MustParsePrefix("172.17.0.0/16"), 16},
		{netip.MustParsePrefix("172.18.0.0/16"), 16},
		{netip.MustParsePrefix("172.19.0.0/16"), 16},
		{netip.MustParsePrefix("172.20.0.0/14"), 16},
		{netip.MustParsePrefix("172.24.0.0/14"), 16},
		{netip.MustParsePrefix("172.28.0.0/14"), 16},
		{netip.MustParsePrefix("192.168.0.0/16"), 20},
	}
	globalScopeDefaultNetworks = []*NetworkToSplit{
		{netip.MustParsePrefix("10.0.0.0/8"), 24},
	}
```
Source: <https://github.com/moby/moby/blob/master/daemon/libnetwork/ipamutils/utils.go>

`[F]` **HIGH** — `NetworkToSplit{Base, Size}` means "chop `Base` into chunks of prefix length `Size`". So the local pool set is: `172.17.0.0/16`, `172.18.0.0/16`, `172.19.0.0/16`, then `172.20.0.0/14`–`172.31.0.0/14` chopped into /16s (that is `172.20`–`172.31`, twelve more /16s), then `192.168.0.0/16` chopped into **/20s** (sixteen of them, `192.168.0.0/20` … `192.168.240.0/20`). Total: **15 × /16 + 16 × /20**, allocated in that order. `[O]` **HIGH** — consistent with `host-a`: `docker0` = `172.17.0.0/16` (first), `webapp_default` = `172.18.0.0/16` (second), and the §9.2 orphan = `172.19.0.0/16` (third). Allocation is strictly first-fit in list order, and the observed history reads straight off it.

`[F]` **HIGH** — Reserved addresses inside a pool, from `moby/daemon/libnetwork/ipams/defaultipam/allocator.go`:
```go
	// Reserve the first address in the range for the:
	// - IPv4 network address
	//   - Except in a /31 point-to-point link, https://datatracker.ietf.org/doc/html/rfc3021
	// - IPv6 Subnet-Router anycast address, https://datatracker.ietf.org/doc/html/rfc4291#section-2.6.1
	bits := pool.Addr().BitLen() - pool.Bits()
	if !pool.Addr().Is4() || bits > 1 { h.Add(pool.Addr()) }
	// For IPv4, reserve the broadcast address.
	// - Except in a /31 point-to-point link
	if pool.Addr().Is4() && bits > 1 { h.Add(netiputil.LastAddr(pool)) }
```
So IPAM itself reserves exactly two addresses: network and broadcast (with the RFC 3021 /31 exception). **The gateway is not special-cased.** `[O]` **HIGH** — on `host-a`, `docker network inspect webapp_default` → `{"Driver":"default","Config":[{"Subnet":"172.18.0.0/16","Gateway":"172.18.0.1"}]}`, and the containers hold `.2 .3 .4 .5 .6` in creation order. The gateway is `.1` simply because the **bridge driver is the first thing to request an address** from a fresh pool, and first-fit after the reserved `.0` yields `.1`. `[R]` **MED** — do the same rather than hardcoding `.1`: it means `--gateway` works for free, and it means a `/31` or a subnet with an unusual gateway needs no special case. Tradeoff: the gateway address depends on allocation order, so it must be recorded in the network's record, not recomputed.

### 10.2 Where the state lives — and the answer is surprising

`[F]` **HIGH** — **Docker's IPAM allocator is pure in-memory state, guarded by a mutex, and it is not persisted at all.** `moby/daemon/libnetwork/ipams/defaultipam/address_space.go`:
```go
type addrSpace struct {
	allocated  []netip.Prefix                  // ordered list of allocated subnets
	subnets    map[netip.Prefix]*PoolData      // allocations, indexed by prefix
	predefined []*ipamutils.NetworkToSplit
	mu         sync.Mutex
}
```
and the driver registers itself with:
```go
	cps := &ipamapi.Capability{RequiresRequestReplay: true}
```
where that capability is documented in `ipamapi/contract.go` as:
```go
	// Whether of daemon start, libnetwork must replay the pool
	// request and the address request for current local networks
	RequiresRequestReplay bool
```
Sources: <https://github.com/moby/moby/blob/master/daemon/libnetwork/ipams/defaultipam/address_space.go>, <https://github.com/moby/moby/blob/master/daemon/libnetwork/ipamapi/contract.go>

`[F]` **HIGH** — The replay is `reservePools()` (§9.3), and it is worth reading for what it *derives from*: for each persisted network it calls `n.ipamAllocate()` to re-reserve the pool, then `n.getEndpointsFromStore()` and re-requests **each endpoint's recorded address** as a preferred address. Source: <https://github.com/moby/moby/blob/master/daemon/libnetwork/controller.go>

`[R]` **HIGH** — **This is the correct architecture and we should copy it exactly: IPAM is an index, not a database.** The authoritative record of "who holds 172.18.0.4" is the *endpoint/container record* — the same record that holds the container's id, its network, and its config. The allocator is a derived acceleration structure that answers "what is free" quickly, and it is rebuilt by replay at startup. Reasoning, in order of weight:
1. **It makes double-booking structurally impossible.** With two stores (a container record *and* an allocation bitmap) you have a two-phase commit and therefore a window where they disagree; every crash in that window produces either a leaked address or, far worse, an address allocated twice. With one store there is no window.
2. **It makes the §9.7 sweep free.** "Which addresses are allocated" and "which containers exist" are the same query, so a container that vanished cannot leak an address — it stops existing and the address is simply free on the next replay.
3. **It is less code.** No bitmap serialization format, no schema migration, no compaction.
Tradeoff: replay is O(containers) at startup, and a corrupted container record loses an address rather than being independently recoverable. Both are fine at our scale; the first is a few thousand `RequestAddress` calls at worst.

`[O]` **MED** — What *is* persisted, on `host-a`: `/var/lib/docker/network/files/local-kv.db`, a single 131072-byte file (a bolt database), last written 2026-08-25. That holds networks, endpoints and sandboxes — the things §9.3's replay and sweep read. `[MED]` on the contents specifically; I did not open it (production host, read-only, and it is a live database). **What would settle it:** `bbolt buckets` on a copy taken from a scratch host.

### 10.3 Restart, and the subnet the orphan burned

`[F]` **HIGH** — On daemon restart the pool set is empty, then `reservePools()` re-reserves exactly the pools of networks in the store. §9.2's orphaned `172.19.0.0/16` is **not** in the store, so IPAM considers it free.

So does the next `docker network create` collide with the orphaned bridge that still owns `172.19.0.1/16`? `[F]` **HIGH** — **No, and the mechanism is the interesting part: Docker asks the kernel routing table, not its own bookkeeping.** From `moby/daemon/libnetwork/netutils/utils_linux.go`:
```go
// InferReservedNetworks returns a list of network prefixes that seem to be
// used by the system and that would likely break it if they were assigned to
// some Docker networks. It uses two heuristics to build that list:
//
// 1. Nameservers configured in /etc/resolv.conf ;
// 2. On-link routes ;
```
and the second heuristic, in full:
```go
func queryOnLinkRoutes() []netip.Prefix {
	routes, err := ns.NlHandle().RouteList(nil, netlink.FAMILY_V4)
	...
	for _, route := range routes {
		if route.Scope == netlink.SCOPE_LINK && route.Dst != nil && !route.Dst.IP.IsUnspecified() {
			...
		}
	}
}
```
The result is passed as `PoolRequest.Exclude`, documented as *"a list of prefixes the requester wish to not be dynamically allocated"*. Source: <https://github.com/moby/moby/blob/master/daemon/libnetwork/netutils/utils_linux.go>

`[R]` **HIGH** — **Implement both heuristics. They are ~30 lines against `RTM_GETROUTE` + a `resolv.conf` parse we are already writing (§6.5), and they prevent the two most-reported Docker networking complaints there are** — "Docker broke my VPN" and "Docker broke my office LAN". Note the second-order benefit for §9: because the orphaned bridge's address creates an on-link route, the kernel *itself* tells us that subnet is in use, so the orphan is **safe but permanent** — it silently burns one /16 of the 15 available and nothing will ever reclaim it. That is the mild version of the leak; the raw/PREROUTING rules are the sharp version.

`[F]` **MED–HIGH** — The same source file carries a long comment explaining why the heuristic is limited to *on-link* routes, and it is the best short writeup of this problem I found. Compressed: checking all non-default routes broke split-tunnel VPNs (which push large aggregates covering subnets nobody uses), and it broke OpenVPN full-tunnel entirely, because `def1` mode installs `0.0.0.0/1` + `128.0.0.0/1` — *"functionally indistinguishable from a default route, yet the Engine was treating them differently. With those routes, there was no way to use dynamic subnet allocation at all."* The conclusion is the useful part: *"The type of route says little about the availability of subnets it covers, except for on-link routes as they specifically define what subnet the current host is part of."* Narrowed in PR #42598, first released in v23.0. Also `[F]`: **the heuristic is IPv4-only**, because *"there's no such thing as on-link routes for IPv6"*.

`[R]` **MED** — Two consequences for us: (a) an on-link route is *evidence*, not proof, so when we skip a pool because of it, **log which route caused the skip** — otherwise "why did my network get 172.24 instead of 172.17" is unanswerable; (b) IPv6 pool selection has no such safety net, which is one more reason IPv6 is a stage-6 item (§4.4).

### 10.4 The other design: CNI host-local, file-per-IP

Worth reading closely because it is the daemonless answer, and D2 may well pick daemonless.

`[F]` **HIGH** — `dataDir` defaults to `/var/lib/cni/networks/<network name>`, and the docs note *"Environments where IPs are released automatically on reboot … may wish to specify `/var/run/cni` or another tmpfs mounted directory instead."* Source: <https://cni.dev/plugins/current/ipam/host-local/>

`[F]` **HIGH** — The whole allocator, from `containernetworking/plugins/plugins/ipam/host-local/backend/disk/backend.go`:
```go
const (
	lastIPFilePrefix = "last_reserved_ip."
	LineBreak        = "\r\n"
)
// Store is a simple disk-backed store that creates one file per IP
// address in a given directory. The contents of the file are the container ID.

func (s *Store) Reserve(id string, ifname string, ip net.IP, rangeID string) (bool, error) {
	fname := GetEscapedPath(s.dataDir, ip.String())
	f, err := os.OpenFile(fname, os.O_RDWR|os.O_EXCL|os.O_CREATE, 0o600)
	if os.IsExist(err) { return false, nil }               // <-- taken; try the next address
	if err != nil { return false, err }
	if _, err := f.WriteString(strings.TrimSpace(id) + LineBreak + ifname); err != nil {
		f.Close(); os.Remove(f.Name()); return false, err
	}
	if err := f.Close(); err != nil { os.Remove(f.Name()); return false, err }
	// store the reserved ip in lastIPFile
	ipfile := GetEscapedPath(s.dataDir, lastIPFilePrefix+rangeID)
	return true, os.WriteFile(ipfile, []byte(ip.String()), 0o600)
}
```
and the mutual exclusion, from `.../disk/lock.go` — `flock(2)` via `go-filemutex` on a file named `lock` inside the same directory:
```go
	if fi.IsDir() { lockPath = path.Join(lockPath, "lock") }
	f, err := filemutex.New(lockPath)
```
Sources: <https://github.com/containernetworking/plugins/blob/main/plugins/ipam/host-local/backend/disk/backend.go>, <https://github.com/containernetworking/plugins/blob/main/plugins/ipam/host-local/backend/disk/lock.go>

`[F]` **HIGH** — So there are **two** independent correctness mechanisms stacked, and understanding why is the point:
- **`flock` on `<dir>/lock`** serializes whole allocation *transactions*, so the "scan for a free address" step sees a stable view.
- **`O_EXCL` on the per-IP file** makes the *claim itself* atomic against anything the lock misses — a second CNI implementation, a stale process holding a fd, a plugin that forgot the lock. It is defence in depth, and it costs nothing.
- **`last_reserved_ip.<rangeID>`** is a hint, not state: it makes allocation round-robin rather than always-lowest-free, which matters because reusing a just-released address immediately collides with stale ARP/FDB entries and lingering conntrack (§9.5).

`[F]` **HIGH** — The properties that follow, and they are excellent: the store is **self-describing** (the filename is the address, the content is the container id, so `grep -r <container-id> /var/lib/cni/networks/` is a complete reverse index), needs **no schema**, and **releases by content match** (`ReleaseByKey` walks the directory comparing file contents), so releasing works even if the caller has forgotten which address it held.
`[C]` **MED** — and the failure mode, per §9.4: an empty file (create succeeded, write failed, remove failed) is an unreleasable allocation, because nothing matches its content.

### 10.5 Concurrency: two `mydocker run`s must not collide

This was the explicit question, so here is the option set with the reasoning, then a verdict.

`[F]` **HIGH** — First, the constraint that rules out the clever answers: **the kernel cannot arbitrate this for us.** There is no "allocate me an address from this subnet" syscall. The kernel learns of an address only when we `RTM_NEWADDR` it, and by then we have already chosen it — and we choose it *inside the container's netns*, where the address is invisible to any other netns, so a duplicate assignment succeeds silently on both sides and produces two containers with the same IP on one bridge. There is no `EEXIST` to catch. **The collision is undetectable after the fact and must be prevented before it.**

| Mechanism | Correct? | Cost | Verdict |
|---|---|---|---|
| **`flock` a single allocation file, read-modify-write a list** | yes | one open + one flock per run | correct, trivially; but the file is a second source of truth (§10.2) |
| **`flock` the network dir + `O_EXCL` file per address** (host-local) | yes, doubly | two opens | self-describing, no schema, releases by content |
| **In-process mutex** (Docker) | yes **iff one process ever allocates** | free | correct only with a daemon; catastrophic if `mydocker run` is a plain process |
| **Derive from a hash of the container id** | **no** | free | see below |
| **Ask the kernel / probe with ARP** | **no** | slow | see below |
| **Sequential from a counter file, no lock** | **no** | free | classic lost-update; two readers get the same value |

`[R]` **HIGH** — **Reject derive-from-something-unique, explicitly, because it is the tempting one.** Hashing the container id into the subnet fails on four counts, any one of which is disqualifying: (1) **birthday collisions** — a /24 has 253 usable addresses, so the expected first collision is around 19 containers, which is *routine*, not an edge case; (2) there is **no release**, so a restarted container cannot reliably get a different address and a colliding pair can never be separated; (3) it cannot honour the reservations — network address, broadcast, gateway, and the user's `--ip` — so it needs a fallback path anyway, and then the fallback needs a lock and we are back where we started; (4) the resulting addresses are unpredictable, which makes `/etc/hosts` snapshots (§6.5 stage 2) and debugging worse. The only defensible use of a hash is **choosing the *host-side veth name*** — 11 hex characters of container id, where a collision is detectable via `EEXIST` on `RTM_NEWLINK` and recoverable by retrying.

`[R]` **HIGH** — **Reject probe-then-claim (ARP/ping the candidate address).** It cannot distinguish "free" from "allocated but the container has not started yet", which is precisely the window two concurrent `run`s are in. It also costs a timeout per candidate. `[C]` — for contrast, note that `lxc-user-nic` uses DHCP for exactly this and §8.6 already records the cost: *"DHCP-based addressing takes seconds"*.

`[R]` **HIGH** — **Our answer: `flock` on the network's directory, held across the whole allocate-and-record transaction, with the container record as the source of truth and an `O_EXCL` per-address claim file as the lock-free backstop.** Concretely:
```
open("/run/mydocker/net/<netid>/lock", O_CREAT|O_RDWR, 0600)   # once
flock(fd, LOCK_EX)                                             # blocking
  scan  /run/mydocker/net/<netid>/addrs/            -> the taken set
  pick  the next free after the recorded cursor     -> round-robin, not lowest-free
  open(".../addrs/172.18.0.4", O_WRONLY|O_CREAT|O_EXCL, 0600)  # the claim
  write(container-id); close                                   # self-describing
  write the container record (address included)                # the truth
  update  .../cursor
flock(fd, LOCK_UN)   # or just close/drop
```
Reasoning: (1) `flock` is one syscall, is released automatically on process death *including `SIGKILL`* — which is exactly the failure mode §9 is about, and is the property no hand-rolled lockfile has; (2) `O_EXCL` gives us a claim that is atomic even against a buggy future version of ourselves that forgets the lock; (3) the claim file is self-describing, so §9.7's sweep can reconcile addresses by reading the directory, with no dependence on our container store being intact; (4) it works identically with or without a daemon, so it does not have to be rewritten when D2 decides.
Tradeoffs, stated: **`flock` is advisory and per-open-file-description**, so it does not protect against a process that does not participate, and it does not work correctly over NFS-mounted state (irrelevant — our state is in `/run`). It also serializes container starts on one network; that is a global lock and it will be the throughput ceiling if we ever start hundreds of containers at once. `// ponytail: global per-network flock; shard by address range if start throughput ever matters.`
`[R]` **MED** — Steal host-local's **round-robin cursor**, not lowest-free. It is one extra small file and it buys real robustness: an address that was just released does not come straight back into service with stale neighbour entries on the bridge, stale peer `/etc/hosts` lines (§6.5), and lingering conntrack (§9.5) pointing at it.

`[R]` **HIGH** — **Interaction with D2's state model, which is the real decision here → Handoff to D2.** The choice above is deliberately the one that is correct under *both* of D2's options, so that this does not become a blocker:
- **Daemon** (Docker's shape): an in-process mutex would suffice and `flock` is redundant — but harmless, and it keeps `mydocker network prune`, a recovery tool, and the daemon from racing.
- **Daemonless CLI** (Podman/netavark's shape): `flock` is **mandatory**; there is no other candidate. Note that netavark itself carries `fs2 = "0.4.3"` as a direct dependency purely for `FileExt::lock_exclusive` on its firewall state (§9.7), so this is the shape the daemonless reference implementation actually uses.
The one thing D2 must *not* choose is "the allocation bitmap is the source of truth, and the container record is a copy". §10.2 explains why: that is the two-phase commit that produces double-allocation on crash.

`[R]` **MED** — **Sticky addresses, which §6.5 asked for.** With the container record as the source of truth this is nearly free: record the address on the record, and on restart pass it as the *preferred* address (Docker's `RequestAddress(poolID, prefAddress, opts)` has this parameter for exactly this reason). If it is still free, the container keeps its address and every peer's `/etc/hosts` snapshot stays correct — which is what makes §6.5's stage 2 last long enough to be worth shipping. If it is taken, fall back to a fresh allocation and rewrite the peers' files. Tradeoff: holding an address across a stop makes "stopped containers" quietly consume the pool; make that visible in `mydocker info` rather than surprising.

### 10.6 MAC addresses, and why deriving them from the IP was abandoned

`[O]` **HIGH** — `host-a`, `docker network inspect webapp_default`:
```
webapp-redis-1     172.18.0.2/16   6e:ee:20:71:68:37
webapp-frontend-1  172.18.0.3/16   32:fa:e6:8a:a4:eb
webapp-nginx-1     172.18.0.4/16   ee:47:c6:16:34:61
webapp-certbot-1   172.18.0.5/16   86:92:c3:83:35:99
webapp-web-1       172.18.0.6/16   ca:be:0b:2b:4b:76
```
`[C]` **HIGH** — The historic Docker convention was `02:42:` + the four bytes of the IPv4 address, so `172.18.0.4` would be `02:42:ac:12:00:04`. **None of these match**, and the local/unicast bits are set randomly per interface. Docker 28 does not derive the MAC from the IP.
`[C]` **MED** — The change landed in the 25.0.x series and was messy: 25.0.1 *re-generated* an IP-derived MAC on restart *"in case the generated IP/MAC addresses have been reused"*; 25.0.3 settled on "a generated MAC is not restored on restart, a configured MAC is". Containers created on 25.0.0 could have **duplicate MACs** and had to be recreated. Sources: <https://docs.docker.com/engine/release-notes/25.0/>, <https://github.com/moby/moby/issues/47146>, <https://github.com/moby/moby/issues/47228>. `[MED]` on the exact version boundaries — the release notes and the issues disagree in detail. **What would settle it:** the moby commit that removed the derivation.

`[R]` **HIGH** — **Let the kernel generate the MAC; do not derive it from the IP.** The reason is a direct §10.5 consequence and worth internalising: if MAC is a function of IP, then **reusing an address reuses the MAC**, so the bridge's FDB and every peer's ARP cache (§3.5) hold entries that are *still syntactically valid* but point at the wrong port for up to `ageing_time` (300s, `[O]` §3.1). The failure is intermittent, direction-dependent, and self-healing after five minutes — the worst diagnostic profile there is. A random MAC on a reused IP produces a clean ARP miss and immediate re-resolution instead. Tradeoff: `--mac-address` must then be recorded explicitly if we support it, and a restarted container gets a new MAC, which breaks MAC-based licensing inside containers (a real, if unsympathetic, complaint against Docker 25).

### 10.7 The honest minimum for us

`[R]` **HIGH** — In the same shape as §6.5, because the same "ship stage 1" logic applies.

**Stage 1 — one network, one subnet, addresses from a locked directory. (~120 lines.)**
- One hardcoded default subnet from a Docker-shaped pool list (§10.1), chosen at network-create time with the on-link-route and nameserver exclusions (§10.3). `--subnet` overrides.
- Gateway = first free address after the reserved network address, recorded on the network record.
- Per-address `flock` + `O_EXCL` claim as in §10.5, with the container record authoritative.
- Reserve network + broadcast; skip the /31 special case entirely at this stage (we will not have point-to-point links).
- Release on teardown; **rebuild the taken-set from the claim directory on start**, and cross-check it against the container store, warning on any disagreement (that warning is the leak detector from §9.7).

**Stage 2 — multiple networks.** The pool list becomes real: allocate a subnet per network, first-fit, with the exclusion list. Add `mydocker network prune` able to find orphaned subnets *by our bridge naming convention* rather than only from records (§9.7) — this is the specific thing Docker cannot do and §9.2 is what it costs.

**Stage 3 — replay instead of a persisted index.** Only if stage 1's directory-of-claims becomes a bottleneck or a correctness problem. Drop the claim files, derive the taken-set purely from container records at startup like `reservePools()` does. `[R]` — this is *simplification*, not a feature, and it is the right end state; it is stage 3 only because stage 1's claim files are also our crash-recovery evidence and we should not give those up until the reconcile pass is proven.

**Never:** IPv6 IPAM before §4.4's dual-stack work; a pluggable IPAM driver interface (one implementation, no second caller — §0's CNI-shaped trait already gives us the seam if we ever need it); DHCP.

---

## 11. The minimal networking ladder

The proposed progression is **validated with two corrections**. Numbering is 1-indexed so that "bridge + NAT" lands on stage 4, which is what §3.2 already calls it (*"bridge + NAT is our only stage-4 model"*).

| # | Stage | What the user gets | Host state created | Success test |
|---|---|---|---|---|
| 1 | share the host netns | nothing isolated; a working container in every other dimension | **none** | `mydocker run alpine wget -qO- example.com` succeeds; `ip addr` inside == host's |
| 2 | new netns, `lo` up | isolation; a private port space | one nsfs bind mount | `nc -l 127.0.0.1:8080` inside works; host `ss -ltn` does not show it |
| 3 | veth to host, manual routes | container↔host only. **A detour** — see below | + one host veth, one host route, one host address | container pings the host's veth address; host pings the container |
| 4 | + bridge + NAT | **the internet.** The first genuinely useful rung | + a bridge, a subnet, `ip_forward`, a MASQUERADE rule, a FORWARD policy | `apk add curl` succeeds inside; two containers ping each other by IP |
| 5 | + published ports (DNAT) | inbound; the container is a *service* | + N DNAT + N filter-ACCEPT + N anti-spoof rules, ± a proxy helper, per port | `curl localhost:8080` from the host **and from another machine**; unpublished ports refused |
| 6 | + DNS | names instead of addresses | + generated `resolv.conf`/`hosts` bind mounts (stage 1 of §6.5) | `nslookup example.com` inside; then peer-name resolution |

### 11.1 Correction 1: stage 1 is real work, and it is not "no networking"

`[R]` **HIGH** — **Sharing the host netns is not the degenerate case; it is a first-class feature with a real implementation and it should be built first.** §7.3 already establishes the mechanism (`namespaces[].path = /proc/1/ns/net`, i.e. the OCI spec's own "join an existing namespace" form) and establishes that it is *the absence of work*: no netns, no veth, no IPAM, no rules. Building it first buys three things disproportionate to its cost:
1. It proves out the **whole rest of the container** — rootfs, exec, cgroups, signals, wait — with networking removed as a variable. Every "is my container broken or is my network broken" question during weeks 1–3 is answered for free.
2. It is the **debugging tool** we will use to develop stages 2–6 (`nicolaka/netshoot` in the host netns, per §7.3).
3. It is the code path that `--network=host` and `--network=container:X` and pod-sharing all reduce to, so it is not throwaway scaffolding.

`[R]` **HIGH** — And stage 2 is not throwaway either: it is `--network=none`, it is the OCI spec's default meaning of a bare `{"type": "network"}` entry (§0), and it is genuinely used (batch jobs, build steps, untrusted code). §1.1's finding is the entire content of stage 2 and it is the highest value-per-byte line in the document: **`lo` must be brought up**, or a namespace-isolated container fails in ways that look nothing like a networking problem.

### 11.2 Correction 2: stage 3 is the best learning rung and the worst product rung

`[R]` **HIGH** — **Do stage 3, but do it as an afternoon's experiment, not as a shipped mode.** What "veth pair to host with manual routing" actually requires, spelled out, because the phrase hides the cost:
- a host-side address to be the container's next hop, on the veth itself (a `/30`, or a `/32` plus an on-link route);
- **a host route per container**, `<container-ip>/32 dev mdk-<id>` — because with no bridge there is no shared segment, so nothing is learned and everything must be routed explicitly;
- `proxy_arp` on the host veth, or a link-scoped route inside the container, to make the container's ARP for its gateway resolve at all;
- no container-to-container path whatsoever without a route per pair, i.e. **O(n²) host routes**.

Compare with stage 4, which replaces all of that with: one bridge (`RTM_NEWLINK`, `INFO_KIND="bridge"`), one `IFLA_MASTER` on the veth, and one kernel-generated connected route for the whole subnet. `[R]` — **the bridge is not an optimisation over manual routing, it is less work than manual routing**, because the kernel's FDB does the per-container bookkeeping we would otherwise do by hand in the routing table. That is the actual reason every container engine uses a bridge, and it is not obvious until you have priced stage 3.

`[R]` **HIGH** — The learning value, though, is high and specific: stage 3 is where you find out that `veth` transmit is a direct `netif_rx` into the peer (§4.2 step 8), that `ifi_change` matters (§2.6), that a veth is `LOWERLAYERDOWN` until its peer is up (§2.4), and that `RTM_NEWROUTE` needs the address to exist first (§2.6 op 10 note). Do it once, from netlink, by hand. Then delete it.

### 11.3 Which rung is first genuinely useful: stage 4, and only stage 4

`[R]` **HIGH** — **Stage 4 (veth + bridge + NAT).** The test is not "does a packet move" but "would a user pick this over `--network=host`", and the answer is no until egress works. Stages 2 and 3 give a container that cannot install a package, cannot fetch a dependency, cannot call an API, and cannot resolve a name — so anyone with real work to do runs stage 1 instead and gets no isolation at all. Stage 4 is the first rung where isolation stops costing the user something they need.

`[R]` **HIGH** — Two supporting arguments worth keeping:
- **Stage 4 is the whole of §3.2's decisive property**: bridge+NAT *"requires nothing whatsoever from the network the host sits on"*. Stage 3 shares that property but not the utility; stages 5–6 are refinements. So stage 4 is the unique point where usefulness and zero-infrastructure-dependency first coincide.
- **`docker run -it alpine sh` followed by `apk add curl` is the actual acceptance test of a container engine**, and it needs exactly stage 4 plus §6.5-stage-1 DNS (~30 lines). Everything above that is a product, not a proof.
`[R]` **MED** — Therefore the pragmatic first release is **stage 4 + the DNS `resolv.conf` rewrite** (a slice of stage 6 pulled forward), which is cheap and without which stage 4 *looks* broken: addresses work, names do not, and every user diagnoses it as "networking is broken". §6.5 already says the rewrite is ~30 lines; pull it in.

### 11.4 Which rung carries the most host-state risk: two different answers, and both matter

`[R]` **HIGH** — Separate the two questions, because conflating them is how people get this wrong.

**Largest *irreversible, global* mutation: stage 4.** It is the only rung that changes what the host *is* rather than what it has:
- `net.ipv4.ip_forward = 1` — **the host becomes a router.** Per §1.1 this also resets every per-interface parameter, so it must be first, read-then-write, and never blind-written.
- `filter/FORWARD` policy DROP — §3.3's paired mitigation, which can break the admin's own routing and is why Docker made it opt-out-able.
- a bridge holding an address and an on-link route, which per §10.3 **permanently reserves a subnet** as far as any future allocator is concerned, and which per §9.2 is the exact object that leaked in production.
- Nothing on this list has an obvious inverse. There is no safe "undo `ip_forward`".

**Highest *rate* of mutation, and the only rung where partial state is a security hole: stage 5.** Per §4.5 the anti-spoof rule is **one per container IP**, and per §4.1 a published port is **three coordinated rules** (`nat` DNAT, `filter` ACCEPT, `raw` DROP) plus possibly a helper process. So stage 5 mutates the host on every container start *and stop*, at O(containers × ports), for the entire life of the installation. And the partial states are not symmetric:
- DNAT installed, filter ACCEPT missing ⇒ the port silently does not work. Annoying.
- **filter ACCEPT installed, `raw` anti-spoof missing ⇒ §4.5's pre-Docker-28 hole is open**: anyone who can route to the container's address reaches *every* port on it, not just the published one. That is a security failure produced by a crash, and it is the specific reason §5.4's atomicity argument is the most important recommendation in this file.
`[R]` **HIGH** — **So: stage 4 needs the loudest logging and the most conservative defaults; stage 5 needs the atomic transaction.** These are different mitigations for different risks and we need both. Stage 5 must not be started before the nftables transaction path from §5.4 works — implementing stage 5 on top of rule-at-a-time iptables means shipping a known security hole with a crash as its trigger.

### 11.5 Where the OCI boundary sits, and whether networking should be a hook

`[F]` **HIGH** — What the spec actually offers, and it is only this: a `linux.namespaces` entry, and six hook phases *"called in the listed order"* — `prestart` (deprecated), `createRuntime`, `createContainer`, `startContainer`, `poststart`, `poststop`. `createRuntime` runs *"during the create operation, after the runtime environment has been created and before the pivot root"* **in the runtime namespace**; `poststop` runs *"after the container is deleted but before the delete operation returns"*, also in the runtime namespace. Source: <https://github.com/opencontainers/runtime-spec/blob/main/config.md#posix-platform-hooks>

A hooks-based `config.json` for stage 5 would look like this — and writing it out is what settles the question:
```json
{
  "linux": { "namespaces": [ { "type": "network" }, { "type": "mount" }, ... ] },
  "hooks": {
    "createRuntime": [ { "path": "/usr/bin/mydocker-cni", "args": ["mydocker-cni","ADD"],
                         "env": ["CNI_NETNS=/proc/self/ns/net", "..."] } ],
    "poststop":      [ { "path": "/usr/bin/mydocker-cni", "args": ["mydocker-cni","DEL"] } ]
  }
}
```
The runtime creates the netns; the `createRuntime` hook receives the container's `state` JSON (including its pid) on stdin, reaches `/proc/<pid>/ns/net`, and configures it; the `poststop` hook tears it down. That is precisely how every CNI-based stack works and it is a genuinely good factoring.

`[R]` **HIGH** — **Build networking in, not as a hook — confirming §0's recommendation, and §9 supplies the decisive new argument that §0 did not have.** §0's reasons were process isolation we do not want and JSON serialization we do not need. The stronger reason is the lifecycle:
1. **`poststop` only runs if someone calls `delete`.** The spec's wording is exact: *"after the container is deleted but before the delete operation returns"*. If our supervisor is `SIGKILL`ed, `delete` is never invoked, `poststop` never runs, and **the hook contract has no answer at all** — while CNI's own contract does have one (the runtime *"must ensure that add is eventually followed by a corresponding delete"*, §9.4), it puts the burden right back on the runtime we were trying to keep thin. The hook boundary does not remove the reconciliation problem; it relocates it somewhere with *less* information.
2. **The hook boundary makes §9.7's sweep harder, not easier.** The sweep needs to enumerate host reality and join it against the live container set. A hook process has the first half and not the second; our engine has both. Putting the network code behind a fork/exec means the component that knows what garbage exists is not the component that knows what is alive.
3. **Errors get worse at the boundary.** §2.6 and §9.7 both depend on distinguishing `EEXIST` from `EPERM` from `ENOENT` to make setup idempotent and resumable. Across a process boundary those become an exit code and English on stderr — the same objection §2.6 raises against shelling out to `ip`, applied to ourselves.
4. **§1.5's socket trick does not survive the boundary.** Our embedded DNS resolver is a socket created inside the container netns and served from a host-netns process — our own event loop. A hook process exits; whatever it created either dies with it or becomes a helper process to supervise (and to leak, §9.1).

`[R]` **HIGH** — **But keep the CNI *shape*, and now for a second reason.** §0 wanted the ADD/DEL/CHECK factoring for future plugin compatibility. §9.4 adds **GC**: CNI 1.1's verb set is `ADD` / `DEL` / `CHECK` / `GC` / `STATUS`, and those five are, independently, exactly the five operations §9.7 concluded we need — attach, detach-idempotently, verify, sweep-against-a-live-set, and am-I-ready. **Copy all five into the internal trait.** That the standard converged on the same five after ten years of production is the best endorsement available, and it costs one trait definition:
```rust
trait NetworkDriver {
    fn add(&self, sandbox: &Sandbox, cfg: &NetConf) -> Result<Attachment>;
    fn del(&self, sandbox: &SandboxId, cfg: &NetConf) -> Result<()>;   // idempotent, best-effort
    fn check(&self, att: &Attachment) -> Result<()>;
    fn gc(&self, live: &HashSet<SandboxId>) -> Result<Vec<Removed>>;   // §9.7's sweep
    fn status(&self) -> Result<()>;
}
```
`[R]` **MED** — And keep the *runtime core* free of all of it, per §0: the runtime core creates and joins namespaces per the OCI `namespaces` array and does nothing else network-shaped. Networking lives in the layer that calls it. That keeps a future `mydocker-runtime` honestly OCI-compatible and keeps the possibility of shelling out to real CNI plugin binaries as a wrapper around the same trait rather than a rewrite.

---

## 12. Rust crates for networking

All version and date data below was read from the crates.io API and the crates' own manifests on 2026-09-04, and the RUSTSEC data from a fresh clone of `rustsec/advisory-db` on the same day. `[O]` on every number in the tables in this sense.

### 12.1 The netlink family, and where the async decision actually lives

`[F]` **HIGH** — The `rust-netlink` org publishes a layered stack, and the layers matter because the async constraint enters at exactly one of them:

| Layer | Crate | What it is | Async? |
|---|---|---|---|
| socket | `netlink-sys` 0.9.0 | `socket()/bind()/connect()/sendto()/recvfrom()` on `AF_NETLINK`, plus `setsockopt` helpers | **no** — `default = []` |
| framing | `netlink-packet-core` 0.9.0 | `nlmsghdr`, `NetlinkMessage<T>`, `NetlinkPayload` (Done/Error/Noop/Overrun/InnerMessage), serialize/deserialize | **no** |
| codec | `netlink-packet-route` 0.33.0 | the `RTM_*` messages and every attribute: link, addr, route, neighbour, rule, tc, … | **no** |
| transport | `netlink-proto` 0.13.0 | futures `Sink`/`Stream` over a netlink socket, sequence demux, connection actor | **yes, inherently** |
| API | `rtnetlink` 0.23.0 | ergonomic builders (`handle.link().add().veth(...)`) | **yes** |
| other families | `genetlink` 0.3.0, `netlink-packet-generic` 0.5.0, `netlink-packet-netfilter` 0.4.0 | generic-netlink and nfnetlink | genetlink: yes |

`[F]` **HIGH** — **`rtnetlink` forces an async executor and there is no escape hatch.** From its own `Cargo.toml`:
```toml
[features]
default = ["tokio_socket"]
tokio_socket = ["netlink-proto/tokio_socket", "tokio"]
smol_socket  = ["netlink-proto/smol_socket", "async-global-executor"]
[dependencies]
netlink-proto = { default-features = false, version = "0.13" }
futures-util = "0.3.11"
futures-channel = "0.3.11"
```
Source: <https://github.com/rust-netlink/rtnetlink/blob/main/Cargo.toml>
There is no `sync` feature. The dependency on `netlink-proto` is unconditional, its entire API is `Future`-returning, and the only choice offered is *which* executor (tokio or smol). So taking `rtnetlink` means either tokio or smol becomes a project-wide dependency, and every netlink call site becomes `.await`.

`[F]` **HIGH** — **The layer below is completely async-free.** `netlink-sys`'s manifest: `default = []`, with `tokio`, `mio` and `async-io` all `optional = true` behind `tokio_socket` / `mio_socket` / `smol_socket`. Source: <https://github.com/rust-netlink/netlink-sys/blob/main/Cargo.toml>. Its default `Socket` is a plain blocking fd.

`[R]` **HIGH** — **The constraint our domain imposes on D2 is therefore not "async or sync"; it is narrower and stranger, and it points the other way from `rtnetlink`.** Three facts from earlier in this document combine into it:
1. **A netlink socket is permanently bound to the netns it was created in** (§2.6 item 4). We need one socket per container netns, opened *after* `setns`.
2. **`setns` is per-thread** and there is no whole-process form (§1.4). So the socket must be created on a thread that is currently in the container's netns.
3. **§1.5's socket trick** requires the same: create a socket inside the netns, then move the thread back out, and keep serving that socket.

So the shape our domain needs is: **"run this closure on a dedicated OS thread pinned into netns X, then restore."** That is a *thread* primitive, not a task primitive. An async runtime actively fights it — a `tokio` task can be moved between worker threads between `.await` points, and a worker thread that has been `setns`'d is a landmine for every unrelated task the scheduler hands it. Getting this right under tokio means `spawn_blocking` or a dedicated single-threaded runtime per namespace, i.e. paying for the executor and then working around it.
`[R]` **HIGH** — **Our recommendation to D2: nothing in D4 requires async, and D4's central primitive is hostile to a work-stealing executor. Do not let networking be the reason the project takes on tokio.** If D2 wants tokio for other reasons (an HTTP API, concurrent image pulls in D3), that is a legitimate decision D2 owns — but then **networking must run on dedicated `std::thread`s with blocking netlink sockets anyway**, not as tasks. Either way the netlink code is the same synchronous code. **→ Handoff to D2: the async call is yours; this is the constraint, not a vote.**

`[F]` **MED** — One concrete gotcha from a production implementation that we would otherwise discover the hard way: netavark sets `NETLINK_GET_STRICT_CHK` on every socket, with the comment *"Needs to be enabled for dump filtering to work"*:
```rust
let mut socket = wrap!(netlink_sys::Socket::new(P::PROTOCOL), "open")?;
let addr = &SocketAddr::new(0, 0);
socket.set_netlink_get_strict_chk(true)?;
wrap!(socket.bind(addr), "bind")?;
wrap!(socket.connect(addr), "connect")?;
```
Source: <https://github.com/containers/netavark/blob/main/src/network/netlink.rs>. Without it, kernel dump requests ignore the filter fields in your request and hand you everything, so `RTM_GETLINK` filtered by ifindex silently returns the whole link table. `[R]` — set it; and note it is exposed by `netlink-sys`, which is a small argument for taking that crate rather than calling `libc::setsockopt` ourselves.

### 12.2 The decisive evidence: what netavark actually depends on

`[C]` **HIGH** — **netavark 2.1.0 is a production Rust container network stack — Podman's only supported backend as of Podman 6.0 — and its dependency list is a direct answer to this entire section.** Verbatim from its `Cargo.toml`:
```toml
netlink-packet-route = "0.33.0"
netlink-packet-core  = "0.9.0"
netlink-sys          = "0.9.0"
nftables             = "0.6.3"
nix     = { version = "0.31.3", features = ["net", "sched", "signal", "socket", "user"] }
ipnet   = { version = "2.12.1", features = ["serde"] }
fs2     = "0.4.3"
tokio   = { version = "1.53.1", features = ["rt", "rt-multi-thread", "signal", "fs", "time"] }
zbus    = "5.19.0"
mozim   = "0.3.2"
```
Source: <https://github.com/containers/netavark/blob/main/Cargo.toml>

Read what is *absent*: **no `rtnetlink`, no `netlink-proto`, no `nftnl`, no `rustables`, no `iptables`.** netavark uses the packet crates and writes its own ~200-line synchronous `Socket<P>` on top of `netlink_sys::Socket` — send, recv, sequence check, multipart loop — and that is the entirety of its netlink transport (§12.1's snippet, plus a `recv` loop that walks multipart messages and matches on `NetlinkPayload::{Done, Error, InnerMessage}`). Its `tokio` dependency is for the **DHCP proxy's gRPC server** (`tonic`), not for netlink. Its `fs2` dependency is for `flock` on firewall state (§9.7).

`[R]` **HIGH** — **This is the single strongest signal available and we should follow it: take the packet crates, write the socket loop, skip `rtnetlink`.** It is also almost exactly what §2.6 independently concluded from first principles ("write it ourselves… the message set is tiny and frozen") — with one refinement. §2.6's argument was against libraries in general; the netavark evidence splits the decision cleanly: **the byte-level TLV encoding is worth taking from a library** (it is 47k lines of tedium with zero learning value beyond the first hour), while **the socket, the sequencing, and the error mapping are worth writing** (that is the part that is actually about how a container engine talks to the kernel, it is ~200 lines, and it is where all the interesting failure modes live).

### 12.3 Crate-by-crate

Health data from crates.io, 2026-09-04. `unsafe` counts are `grep -c unsafe` over `src/` from a fresh shallow clone — a crude but honest proxy.

| Crate | ver / last release | recent dl (90d) | `unsafe` / src lines | RUSTSEC | Hides mechanics we want to learn? | Verdict |
|---|---|---|---|---|---|---|
| `netlink-sys` | 0.9.0 / 2026-08-18 | 8.87M | 22 / 1712 | none | barely — it is `libc::socket/bind/sendto` plus `setsockopt` | **take** |
| `netlink-packet-core` | 0.9.0 / 2026-08-18 | 9.15M | **0** / 2653 | none | no — it *is* `nlmsghdr` in Rust types | **take** |
| `netlink-packet-route` | 0.33.0 / 2026-08-18 | 8.43M | **0** / 47028 (286 files) | none | somewhat — hides the TLV byte layout, which is the one thing §2.6 says will bite. Read `link_info/veth.rs` once, then use it | **take** |
| `netlink-proto` | 0.13.0 / 2026-08-18 | 7.20M | 1 / 1406 | none | yes — hides sequencing and demux, which we want to own | **avoid** |
| `rtnetlink` | 0.23.0 / 2026-08-18 | 4.24M | 0 / 9916 | none | **yes, and the wrong ones** — hides the message construction *and* imposes an executor | **avoid** |
| `neli` | 0.7.4 / 2026-01-28 | 10.07M | — | none | different design; has a real `sync` feature | **avoid** (see below) |
| `genetlink` | 0.3.0 / 2026-08-18 | 213k | — | none | n/a — we need no generic-netlink family | **defer/never** |
| `netlink-packet-netfilter` | 0.4.0 / 2026-08-22 | **680** | — | none | for conntrack (§9.5) | **defer** — 680 downloads is not a dependency, it is a prototype |
| `nix` | 0.31.3 / 2026-05-11 | 170M | large but audited | RUSTSEC-2021-0119 (`getgrouplist`, patched ≤0.23) | no — thin typed syscall wrappers | **take** |
| `ipnet` | 2.12.1 / 2026-08-02 | 136M | — | none | no — CIDR arithmetic | **take** |
| `nftables` | 0.6.3 / 2025-08-15 | 546k | **0** / 3993 | none | partly — hides the nft expression *encoding*, not the semantics | **take (MVP)** |
| `nftnl` (+`nftnl-sys`) | 0.9.4 / 2026-08-11 | 58k | 137 / 3007 **+ 8537 / 46288** | **RUSTSEC-2025-0126** (patched ≥0.9.0) | no, but see the C tax below | **defer** |
| `rustables` | 0.8.8 / 2026-06-23 | 20k | 13 / 5753 | none | no — pure-Rust nft netlink | **defer** |
| `iptables` | 0.6.0 / 2025-08-07 | 379k | — | none | it is `Command::new("iptables")` | **avoid** |
| `fs2` | 0.4.3 / **2018-01-06** | 16.6M | — | none | no — `flock` | **avoid**, use `nix` (see below) |

Notes on the entries that need them:

`[R]` **MED** — **`neli` vs the rust-netlink stack.** `neli` 0.7.4 is healthy (10M downloads/90d, releases through Jan 2026) and is the only crate here that offers **both** `sync` and `async` as first-class features, which on paper is exactly our constraint. Take rust-netlink anyway, for one reason: **`netlink-packet-route` has the specific attributes we need, already typed, and `neli` leans more on you to build the attribute payloads.** Verified coverage — every item in §2.6's table plus every bridge-flag recommendation in §3.4/§3.5/§7.2:
```
InfoVeth::Peer(LinkMessage)          <- §2.6 op 4, the embedded ifinfomsg, the "genuinely fiddly encoding"
LinkAttribute::NetNsFd(RawFd) | NetNsPid(u32)        <- §2.3, and the fd form that avoids the pid race
LinkAttribute::Controller(u32)       <- IFLA_MASTER,  §2.6 op 5
LinkAttribute::Link(u32)             <- IFLA_LINK, the peer's ifindex
LinkAttribute::{IfName, Mtu, Address}, link_info/bridge.rs
InfoBridgePort::Isolated(bool)       <- §3.4/§7.2's recommended --icc mechanism
InfoBridgePort::HairpinMode(bool)    <- §3.5
```
Sources: <https://github.com/rust-netlink/netlink-packet-route/blob/main/src/link/attribute.rs>, `.../src/link/link_info/veth.rs`, `.../src/link/link_info/bridge_port.rs`. Also: netavark bets on rust-netlink, so bugs we hit are bugs Podman hits.

`[F]` **MED** — A genuinely useful fact carried in `netlink-packet-route`'s own source comment on `InfoVeth`, which would otherwise cost us an afternoon:
```rust
// This data is not for querying/dumping as in kernel 6.5.8,
// because the `struct rtnl_link_ops veth_link_ops` does not have `fill_info`.
// Only for create veth
```
i.e. **you cannot read a veth's peer back out of `IFLA_LINKINFO`/`INFO_DATA`** — the kernel never fills it in. The peer is discoverable only as `IFLA_LINK` (an ifindex, which is why §1.6 saw `hostend@if2`). That matters for §9.7's reconcile pass: to find our veths on the host we match on the **name prefix**, not on peer information.

`[R]` **HIGH** — **`fs2`: last release January 2018.** netavark uses it; we should not. It is eight years unmaintained and it does nothing `nix` does not: `nix::fcntl::{Flock, FlockArg}` gives `flock(2)` with an RAII guard, and we are taking `nix` regardless. One fewer dependency for zero lines of code. (`fs4` is the maintained fork if a guard-shaped API is wanted, but this is not worth a crate.)

`[R]` **HIGH** — **What `nix` covers, so we do not go looking elsewhere.** With features `["sched","mount","socket","net","fs","signal","user","process"]`: `sched::{unshare, setns, CloneFlags}` (§1.3, §1.4), `mount::{mount, umount2, MsFlags, MntFlags}` (§1.2's bind mount and §9.7's `MNT_DETACH`), `fcntl::Flock` (§10.5), `net::if_::if_nametoindex`, `sys::socket::*`, `unistd::{fork, pipe, dup2}`. `[F]` — **what `nix` does *not* cover is any netlink message encoding at all.** It gets us every syscall in §1 and §9 and none of §2. That division is the reason the answer is "`nix` + the packet crates", not "one or the other".

### 12.4 nftables and iptables crates

Three genuinely different designs, and the differences are more interesting than the version numbers.

**`nftables` 0.6.3 (`nftables-rs`) — typed Rust structs → nft's JSON API → the `nft` binary.** `[F]` **HIGH** — `helper::apply_ruleset` builds JSON with serde and pipes it to a subprocess:
```rust
const NFT_EXECUTABLE: &str = "nft"; // search in PATH
...
let default_args = ["-j", "-f", "-"];
let process = nft_cmd.args(args).args(default_args)
    .stdin(Stdio::piped()).stdout(Stdio::piped()).spawn();
```
and reads back with `get_current_ruleset_with_args(..., ["list","table","inet","netavark"])` → `serde_json::from_str::<Nftables>`. Errors are `NftFailed { program, hint, stdout, stderr }`. Source: <https://github.com/nftables-rs/nftables-rs/blob/main/src/helper.rs>
`[C]` **HIGH** — **This is what netavark uses**, via `nftables::batch::Batch` + `helper::apply_ruleset`, in its `src/firewall/nft.rs`, for a table named `netavark` with chains at explicit priorities `DNATPRIO = -100`, `SRCNATPRIO = 100`, `FILTERPRIO = 0`. Source: <https://github.com/containers/netavark/blob/main/src/firewall/nft.rs>
`[R]` **HIGH** — **Take it for the MVP.** It gets us the four things §5.4 actually asked for: **atomicity** (one `nft -j -f -` per operation is one kernel transaction), **typed rule construction** (a serde struct, checked at compile time, not a formatted string), **named sets and maps**, and **`inet`-family single-table rules** — with zero `unsafe`, zero C linkage, and 546k downloads/90d behind Podman's production use. Tradeoffs, stated plainly: (1) a **runtime dependency on the `nft` binary**, which on a minimal host may be absent — detect at startup and fail with a clear message, do not discover it mid-setup; (2) **errors arrive as stdout/stderr strings**, so we cannot switch on an errno — which §2.6 correctly calls a trap for `ip`, but is far less costly here because §5.4's design has exactly one nft operation per container (a whole-table transaction) rather than twelve; (3) one `fork`+`exec` per container start; (4) `nftables-rs`'s last release is 2025-08-15, about a year old — slow, but not dead, and Podman's dependence on it is meaningful insurance.
`[R]` **MED** — Note this **partly contradicts §5.4's `[R] MED` recommendation of `nftnl`.** §5.4 framed `nft -f -` as "an honest stopgap"; the new evidence is that it is what the reference production implementation ships permanently. Flagged in §14 for the verification pass.

**`nftnl` 0.9.4 + `nftnl-sys` — FFI to `libnftnl` (+`libmnl`).** `[R]` **MED** — **Defer, and the reason is a concrete tax rather than a vibe.** `nftnl-sys` carries **26 checked-in bindgen outputs, one per libnftnl release** (`nftnl_1_0_6.rs` … `nftnl_1_3_0.rs`, 46288 lines total, 8537 `unsafe`), selected by 24 cumulative Cargo features (`nftnl-1-1-0 = ["nftnl-1-0-9"]`, …). You choose at *build* time which libnftnl ABI you are compiling against, and it must agree with the `libnftnl.so` on the *host* at run time. Source: <https://github.com/mullvad/nftnl-rs>. For a project that wants to ship one binary that works on Ubuntu 24.10 and Rocky 8.8 alike, that is a real distribution problem, not a footnote.
`[F]` **HIGH** — And the safety record is the argument §5.4 could not have made, because both advisories postdate it being written into a recommendation:
- **RUSTSEC-2025-0126** (2025-10-18), `nftnl`, category `memory-corruption`: *"A heap-buffer-overflow vulnerability exists in the Rust wrapper for libnftnl, triggered via the nftnl::Batch::with_page_size constructor."* Patched in ≥0.9.0.
- **RUSTSEC-2025-0142** (2025-10-18), `mnl` (the `libmnl` wrapper underneath), also `memory-corruption`: *"The function `mnl::cb_run` is marked as safe but exhibits unsound behavior when processing malformed Netlink message buffers… The underlying issue is a bug in `libmnl` where during validation `nlh->nlmsg_len` is cast to an `int` and becomes negative if `nlmsg_len` is greater than `INT_MAX`."* Patched in ≥0.3.1.
Sources: <https://github.com/rustsec/advisory-db/blob/main/crates/nftnl/RUSTSEC-2025-0126.md>, <https://github.com/rustsec/advisory-db/blob/main/crates/mnl/RUSTSEC-2025-0142.md>
`[R]` **MED** — Both are patched and neither is exploitable by an attacker in our threat model (we control the page size and the buffers). The signal is not "these crates are dangerous"; it is that **two memory-corruption advisories landed on the same day against the C-FFI layer of a firewall stack, and one of them was a `safe` function that segfaults on malformed input we would be feeding it from the kernel.** For a project whose stated premise is owning the mechanism, taking on 8500 lines of generated `unsafe` to write firewall rules is the wrong trade when a zero-`unsafe` option with the same atomicity exists.

**`rustables` 0.8.8 — pure Rust, its own nftables netlink implementation.** `[R]` **LOW→MED** — **Defer, but revise the earlier assessment upward slightly.** §5.4 called it "too thin to bet on" at ~44k lifetime downloads; it is now 47.5k lifetime / 20k in 90 days, so it is being used a little. The interesting datum §5.4 did not have: it is **5753 lines with 13 `unsafe`** — a genuine pure-Rust nft netlink stack, no C, and structurally the thing we would eventually want. It is still ~2% of `nftables-rs`'s usage and it moved to GitLab. `[R]` — the right posture is §5.4's: keep the firewall trait, revisit later, migrate behind it if it matures. Migration is contained precisely because §5.4 put the trait there.

**`iptables` 0.6.0 — a subprocess wrapper, and a nice illustration of the trap.** `[F]` **HIGH** — it is `std::process::Command`, it parses `iptables --version` with a regex to discover the version, and it branches on the result:
```rust
let cmd = if is_ipv6 { "ip6tables" } else { "iptables" };
let version_output = Command::new(cmd).arg("--version").output()?;
...
fn exists(...) { if <old version> { return self.exists_old_version(table, chain, rule); } ... }
...
output = output_cmd.args(args).arg("--wait").output()?;   // --wait only on new enough versions
```
Source: <https://github.com/yaa110/rust-iptables/blob/master/src/lib.rs>
`[R]` **HIGH** — **Avoid, and note that the crate's own existence is the evidence for §5.4's argument.** A well-maintained wrapper (379k downloads/90d) still has to sniff the binary's version and keep two code paths for "does this build support `-C`", plus conditional `--wait` for the xtables lock. That is version skew as a permanent, structural cost — on top of §5.1's finding that `iptables` on any modern host is *already* a translation layer onto nftables. Using this crate means: our Rust talks to a subprocess, which parses a syntax, which translates to nft expressions, which reach the same kernel subsystem `nftables-rs` addresses in one hop.

`[R]` **MED** — **Conntrack (§9.5) has no good crate.** `netlink-packet-netfilter` 0.4.0 has **680 downloads in 90 days**; `conntrack` 0.1.5 has 12.8k. Neither is a dependency I would take for a security-relevant flush. `[R]` — **shell out to `conntrack -D -d <ip>` for the MVP**, treat a missing binary as a warning not an error, and hand-roll ctnetlink later if it matters (`IPCTNL_MSG_CT_DELETE` is a small message and we will already own a netlink socket loop by then).

### 12.5 Is shelling out an acceptable MVP shortcut, or a trap?

`[R]` **HIGH** — **It depends entirely on the tool, and the deciding question is: "is the thing I need back from this tool an error I have to branch on?"** That single test sorts all four cases, and it explains why §2.6 and §5.4 reach opposite conclusions about `ip` and `nft` without contradicting each other.

| Tool | Ops per container start | What we need back | Verdict |
|---|---|---|---|
| `ip` | ~12 | **typed errors** — `EEXIST` vs `EPERM` vs `ENODEV` is what makes §9.7's setup idempotent and resumable | **trap.** §2.6 settled this; §9 strengthens it |
| `nft` | **1** (a whole-table transaction) | success or failure, atomically | **acceptable, and it is what production Rust ships** |
| `iptables` | ~6, non-atomic | typed errors, on a translation layer, with version skew | **trap, twice over** |
| `conntrack` | 1, best-effort | nothing; failure is a warning | **acceptable** |

The asymmetry is not about taste. §2.6's four objections to `ip` are (1) English errors, (2) the pid-not-fd race, (3) the `/var/run/netns` convention we reject, (4) `~12` execs. **Only objection (1) transfers to `nft`, and §5.4's atomicity turns 12 operations into 1, which shrinks the cost of a stringly-typed error by an order of magnitude** — a single failed transaction means "nothing happened", which is a complete and actionable answer even without an errno. Conversely, `ip`'s per-operation failures each need a *different* recovery decision, which is exactly what a parsed English string cannot give us.

`[R]` **HIGH** — **Does this contradict "own the mechanism"? No, and the distinction is worth naming, because it will come up for every dependency decision in this project.** Our premise is understanding and owning the **interface between a container engine and the kernel**. For networking that interface is:
- **rtnetlink** — a frozen, ~14-attribute, 3-message-type protocol (§2.6). Owning it is a weekend and it *is* the learning. **Own it.**
- **nftables expressions** — a large, evolving bytecode-like encoding where `nft` is the reference compiler. Owning it is a month and teaches us about nftables' internal representation, not about containers. **Do not own it.** What we must own — and do own, completely, under §5.4 — is **the ruleset's semantics**: which chains, which priorities, which hooks, which sets, in what order, and why. That knowledge is in §4 and §5 and it is unaffected by whether the last hop is `libnftnl` or `nft -j -f -`.
The honest line is: **we own every design decision and every packet path; we decline to own two byte-encodings, one of which we take a library for (TLV, `netlink-packet-route`) and one of which we take a compiler for (nft expressions, the `nft` binary).** Both are marked as such, with an upgrade path.
`// ponytail: nft(8) as the firewall backend; swap in nftnl or rustables behind the firewall trait if the exec cost or the string errors ever bite.`

### 12.6 Recommended set

**Take:**
```toml
netlink-sys          = "0.9"    # blocking AF_NETLINK socket + setsockopt (NETLINK_GET_STRICT_CHK)
netlink-packet-core  = "0.9"    # nlmsghdr framing, NetlinkMessage<T>
netlink-packet-route = "0.33"   # RTM_* codec: veth peer, NetNsFd, Controller, bridge-port flags
nftables             = "0.6"    # typed nft JSON -> `nft -j -f -`, one atomic transaction per op
nix     = { version = "0.31", features = ["sched","mount","socket","net","fs","signal","user","process"] }
ipnet   = { version = "2",   features = ["serde"] }   # CIDR arithmetic, subnet/host iteration
```
Six crates, no C linkage, no async runtime, no `unsafe` of our own except what `nix` and `netlink-sys` already encapsulate.

**Avoid, implement ourselves (~250 lines total, and this is the part with the learning in it):**
- **the netlink socket loop** — bind, connect, sequence numbers, multipart/`NLMSG_DONE` walking, `NLM_F_DUMP_INTR` restart, `ENOBUFS`, and mapping the `NetlinkPayload::Error` code onto a typed error our setup path can branch on (§2.6's long tail). Model on netavark's `Socket<P>`; ~200 lines.
- **the setns-pinned-thread primitive** — `std::thread` + `setns(userns)` then `setns(netns)` (§1.4's order, non-negotiable), a socket created inside, restore-on-panic. ~50 lines and the single most architecturally important thing in this document (§1.5, §12.1).
- **the userland proxy** for the three cases in §4.4 and the rootless case in §8.4 — accept/connect/copy, ~100 lines, no crate.
- **IPAM** (§10) — no crate exists that is worth the dependency; it is a locked directory and a cursor.
- **file locking** — `nix::fcntl::Flock`, not `fs2`.
- **the firewall trait** — one trait, two eventual impls, per §5.4.

**Defer:**
- `nftnl` / `rustables` — behind the firewall trait, if `nft`'s exec cost or string errors ever measurably hurt. Revisit `rustables` specifically; it is the pure-Rust end state.
- `netlink-packet-netfilter` / `conntrack` — for §9.5's flush. `conntrack -D` until then.
- `genetlink` / `netlink-packet-generic` — nothing in stages 1–6 needs a generic-netlink family.
- `rtnetlink` — only reconsider if D2 independently adopts tokio *and* we discover the pinned-thread model is unworkable, which §12.1 argues it will not be.
- `neli` — the fallback if rust-netlink's release cadence ever stalls; it has the sync story and comparable reach.

---

## 13. Handoff

Consolidating the inline handoff notes (§0, §1.3, §1.5, §2.6, §6.2, §8.1, §8.2) plus what §9–§12 added. Ordered by how much each item constrains the other domain's design rather than merely informing it.

### 13.1 → D1 (kernel mechanics & isolation)

1. **A newborn netns has `lo` DOWN with no `127.0.0.1`** (§1.1, `[O]`). `127.0.0.1/8` is added by the kernel in response to `NETDEV_UP` on a loopback device, not at namespace creation. Bringing `lo` up is mandatory in every mode including `--network=none`. If D1 owns namespace creation, this line belongs there.
2. **`setns(2)` for a netns requires `CAP_SYS_ADMIN` in *two* user namespaces** — the target netns's owning userns *and* the caller's current userns (`netns_install()`, §1.4, kernel source quoted). **The order is always `setns(userns)` then `setns(netns)`.** This is the single most confusing `EPERM` in the whole domain and it is a general namespace fact, not a networking one.
3. **`setns` is per-thread with no whole-process form** (§1.4). Combined with §2.6 item 4 (a netlink socket is permanently bound to its creating netns) and §1.5 (a socket's netns is fixed at `socket(2)` time), this produces the primitive D2 must provide (§13.2 item 1).
4. **`/proc/sys/net/**` is entirely per-netns and writable by userns-root** (§1.1, §8.1, both `[O]`). So `ping_group_range` and `ip_unprivileged_port_start` are fixable *inside* the container with no host change — which is why §8.5's "rootless can't bind :80" folklore is wrong.
5. **Writing `net.ipv4.ip_forward` resets all other IP configuration to defaults** (`ip-sysctl.txt`, §1.1, `[F]` HIGH). Any sysctl-setting code D1 owns must order `ip_forward` first and must read-then-write, never blind-write.
6. **Device migration on netns death is asymmetric**: physical devices move back to the initial netns, virtual devices (veth) are destroyed (`network_namespaces(7)`, §1.6). This is the fact that makes veth cleanup unnecessary and it is worth stating in D1's namespace-lifecycle section.
7. **`dev->netns_immutable` (was `NETIF_F_NETNS_LOCAL`)** blocks moving `lo` and many physical drivers between namespaces even with full privilege (§8.1). Ext-ack text: `"The interface netns is immutable"`.
8. **nsfs identity**: `stat().st_ino` on a netns fd *equals* the namespace id printed in `net:[...]` (§1.2, `[O]`). Cheap identity comparison with no string parsing — useful anywhere D1 needs to answer "same namespace?".
9. **New in §12.1**: `NETLINK_GET_STRICT_CHK` must be set on a netlink socket or the kernel ignores dump filters and returns everything. A general netlink fact, easy to lose a day to.

### 13.2 → D2 (runtime architecture, the async decision, the state model)

**The four hard constraints, in order of cost-if-ignored:**

1. **We need "run this closure on a dedicated OS thread pinned into netns X, then restore".** Forced by §1.4 (setns is per-thread), §1.5 (socket netns is fixed at creation), and §2.6 item 4 (netlink socket is netns-bound). It must be a *thread* primitive with a `Drop`/panic-safe restore, and it must be usable both transiently (create a socket, come back) and for the whole duration of a setup sequence. Everything in D4 is built on it.
2. **The async decision is D2's, and D4 does not vote for async — it votes against work-stealing.** (§12.1, with manifest evidence.) `rtnetlink` is the only crate that would force tokio and we recommend not taking it; the packet crates are async-free. More important than the crate choice: **a work-stealing executor is actively hostile to constraint 1**, because a `setns`'d worker thread will later be handed unrelated tasks. If D2 adopts tokio for D3/API reasons, networking must still run on dedicated `std::thread`s with blocking sockets. State this explicitly in D2 so nobody "fixes" it later.
3. **A synchronization point before the container's `execve`.** §1.3 established that networking setup happens *after* the container process exists (we pin `/proc/<pid>/ns/net`), so the container must block until setup completes or its first instruction runs without `eth0`. Docker/CNI use a socket or a completed `createRuntime` hook. D2 needs this barrier anyway; D4 depends on it.
4. **The network sandbox is a first-class entity with its own refcount, distinct from the container** (§7.3, reinforced by §9.7). It owns the netns, the bind mount, and the veths; containers attach to it. Getting this wrong is a painful refactor and getting it right costs one struct. It is also why we need no pause container: the nsfs bind mount is a better primitive than "a process holds the namespace".

**The state model, which §9 and §10 turn into requirements:**

5. **One source of truth, and it is the container/endpoint record — not a separate allocation store.** (§10.2, following Docker's `RequiresRequestReplay` + `reservePools()`.) IPAM is an index rebuilt by replay, not a database. This eliminates the two-phase commit whose crash window produces double-allocated addresses. **D2 must not model the IPAM store as authoritative.**
6. **The record must outlive the host state it describes.** (§9.3, and §9.2's production leak is what violating it costs.) Write the tombstone *before* the first host mutation of a teardown; delete the record *after* the last one succeeds. Docker's `networkCleanup()` can only retry deletes for records that still exist with `inDelete` set — the orphan in §9.2 is unreachable precisely because its record went first.
7. **Setup must be idempotent, and there must be exactly one setup path.** (§9.7, following netavark's `firewall-reload`, which recovers by re-running the ordinary `setup_network`/`setup_port_forward`.) No separate "restore" code path. This is a constraint on how D2 structures create/start/restore.
8. **Teardown must be best-effort and must never error merely because something is already gone.** (§9.4, CNI: *"Plugins MUST accept multiple DEL calls … and return success if the interface in question, or any modifications added, are missing."*) Every step runs, errors are collected, not short-circuited.
9. **We need a reconcile/GC entry point that D2 calls with the live container set.** (§9.7, §11.5.) CNI 1.1 added a `GC` verb for exactly this and Docker's `cleanupLocalEndpoints()` is the same algorithm. D2 owns "what is alive"; D4 owns "what exists on the host". Neither can do it alone. Trigger it on daemon start, or on first CLI invocation after boot if daemonless.
10. **The daemon/daemonless choice determines the IPAM concurrency mechanism** (§10.5). Daemon ⇒ an in-process mutex suffices; daemonless ⇒ `flock` is mandatory. We recommend `flock` either way (correct under both, and it protects the daemon from its own recovery tooling), but D2 should know this is the coupling.
11. **Rootless: any network helper must be launched from a process already inside the container's userns**, never from the daemon (§8.2, verified `EPERM` on `old-host`). A process-topology constraint that is invisible until it fails.
12. **Never `unshare(CLONE_NEWNET)` on a thread of a multi-threaded daemon** (§1.3). Prefer letting container creation make the netns and pinning it afterwards.
13. **Our state directory is `/run/mydocker/` rootful and `$XDG_RUNTIME_DIR/mydocker/` rootless** (§8.1, `[O]`: `mkdir` in `/run` fails even as root-in-userns, because file ownership is not namespaced). Netns bind-mount paths, the IPAM lock, and the firewall intent files all live there.

### 13.3 → D3 (images & storage)

1. **`/etc/resolv.conf` and `/etc/hosts` are generated per container on the host and bind-mounted over the image's copies** (§6.2, `[F]` HIGH). They are not image content and never enter a layer. D3 owns the file lifecycle and the mount; D4 owns the content. Docker's paths: `/var/lib/docker/containers/<id>/{resolv.conf,hosts}`.
2. **Both files are mutable after creation and must be rewritable while the container runs.** §6.5 stage 2 appends peer entries to `/etc/hosts` as containers join and leave a network, which means the bind-mount source must be a real file we can rewrite in place — not a copy baked at create time. This constrains D3's mount plan.
3. **`/etc/hostname` must agree with the `/etc/hosts` line for the container's own IP** (§6.2, `[O]`: `172.18.0.4 301b2be6f7c8`). A large amount of software calls `gethostname()` then resolves it.
4. **These bind mounts are host state that outlives us** (§9.1) — unmounting them is our teardown's job, and a leaked one is a stale mount-table entry pointing into a deleted container directory.
5. **Nothing in D4 needs the image at all.** Worth stating: networking is entirely independent of image format, layers and the storage driver. There is no ordering dependency between D3 and D4 beyond items 1–4.

### 13.4 → D5 (cgroups, security, rootless)

1. **`CAP_NET_ADMIN` in the userns owning the target netns is the whole gate** (§8.1, kernel source). The permission decision lives in the rtnetlink layer, not in `veth.c` — so it is uniform across every device operation, and there is no per-device policy to discover.
2. **The rootless wall, precisely**: everything works *inside* an unprivileged netns (bridges, veths, addresses, enslaving, nft `nat` chains, network sysctls — all `[O]` verified on `old-host`); the *only* thing that fails is connecting that netns to the host stack (`ip link set <peer> netns <host-pid>` → `EPERM`). That single `EPERM` is why usermode networking exists.
3. **"Rootless containers can't bind port 80" is wrong** (§8.5, kernel source + `[O]`). `ip_unprivileged_port_start` and the `CAP_NET_BIND_SERVICE` check are both evaluated against the *socket's* netns and its owning userns, so a rootless container binds `:80` internally with no change at all. Only a **host-side** port < 1024 is blocked. D5 should not carry the folklore.
4. **Do not build a setuid helper** (§8.6). For a project whose premise is understanding the machinery, a setuid binary is the one component where a mistake is a local root exploit. `--network=host` or the usermode cost are honest answers.
5. **Shell out to `pasta`; do not write a usermode TCP/IP stack** (§8.3, §8.7). ~5000 LoC, seccomp-sandboxed (34/43 syscalls), Podman's default since 5.0 and now its *only* rootless option (§14 item 3). Its `splice()` local bypass measures 64–71 Gbps, i.e. at or above kernel veth.
6. **Per-netns sysctls we should set for the container and the security reasoning**: `ping_group_range` (so `ping` works without `CAP_NET_RAW`; free and per-netns) and, only if asked, `ip_unprivileged_port_start`. Never `route_localnet` (§4.4: it re-introduces the pre-Docker-28 L2 leak where hosts on the same switch reach ports published to localhost).
7. **New in §11.4: enabling `ip_forward` is a security change the admin did not request.** If we set it we must also install a default-deny on `filter/FORWARD` (Docker 28's `ip-forward-no-drop` pairing), and log both loudly. And per §9.7 we must never auto-revert either on uninstall.
8. **New in §9**: partial firewall state is a *security* failure, not just a cosmetic one — a `filter` ACCEPT without the matching `raw` anti-spoof rule reopens §4.5's pre-28 hole (any host that can route to a container address reaches every port on it). This is the security argument for §5.4's atomic-transaction requirement, and it is why stage 5 must not ship on rule-at-a-time iptables.
9. **`MYDOCKER-USER`**: create an empty chain jumped to first, never write into it, never flush it (§4.6) — it is the only mechanism by which a host admin can override us. And warn on first `-p` when we detect a foreign firewall manager (`ufw-*`, `firewalld`, `f2b-*`), because `ufw deny 80` does not stop `-p 80:80` and nothing anywhere tells the admin that.
10. **Rootless state must live under a directory the real user owns** (§8.1, `[O]`: root-in-userns cannot `mkdir` in `/run`). Capabilities are namespaced; file ownership is not.

---

## 14. Open questions

### 14.1 Unresolved in this document

1. **Provenance of the §9.2 production orphan.** The *state* is `[O]` HIGH; how it got there is not established. **Settle with:** `journalctl -u docker --since '<network creation date>' | grep 0d4cf39c58f7` on `host-a` (read-only, permitted, and not yet run), plus reproducing the three candidate paths on a local VM: `docker network rm` with a rule-delete forced to fail, `SIGKILL` of dockerd mid-`network rm`, and `docker network rm` while a container is still attached.
2. **Whether Docker's `nat`/`raw` rules for a live network are re-derived at daemon start or only written at network-create time.** §9.2's evidence (filter cleaned, nat + raw leaked) implies the latter, but I inferred it from the end state rather than reading `setupPlatformFirewall()`/the bridge driver's init path. **Settle with:** `moby/daemon/libnetwork/drivers/bridge/bridge_linux.go` `createNetwork`/`Restore` paths, plus `iptables-save` diffs across a `systemctl restart docker` on a VM.
3. **Whether our reconcile pass can safely run while containers are live.** §9.7 proposes "delete the nft table and re-add from intent in one transaction". That is atomic at the netfilter level, but conntrack entries created under the old ruleset survive it (§9.5), so a rebuild may need a conntrack flush for every live container — which would briefly break in-flight UDP. **Settle with:** an experiment on a local VM measuring whether established TCP and UDP flows survive `nft delete table` + immediate re-add in one transaction.
4. **The cost of `nft -j -f -` per container start.** §12.5 accepts one `fork`+`exec` per operation. Unmeasured. **Settle with:** timing 100 sequential container starts with the `nftables` crate vs. a hand-rolled `nftnl` path on a VM. If the exec is <5 ms it is settled forever.
5. **Whether `flock` on a `/run` tmpfs is sufficient under all our access patterns** (§10.5). It is advisory and per-open-file-description; a process that opens the file twice can deadlock itself. **Settle with:** a small concurrency test — 50 parallel `mydocker run` on one network, asserting 50 distinct addresses and no wedge.
6. **IPv6 end to end.** §4.4 concluded dual-stack containers are the right fix for the userland proxy, and §10.3 notes that the on-link-route safety net for pool selection **does not exist for IPv6**. So IPv6 IPAM has no collision protection and we have not designed one. Unresolved and deliberately deferred.
7. **Whether a stale `docker-proxy`-equivalent can be identified reliably after our death.** §9.7 proposes matching `argv[0]`, which is racy (a user can spawn anything). A pidfd or a cgroup would be better. **→ overlaps D5.**
8. **Sticky IPs across a host reboot** (§10.5). If our state lives in `/run` (tmpfs), it is gone at boot and every container gets a fresh address, invalidating §6.5's `/etc/hosts` snapshots on restart. host-local documents both options (`/var/lib/cni` vs `/var/run/cni`) without choosing. We have not chosen either. **Settle with:** a decision, not research — and it belongs with D2's state model.

### 14.2 Existing claims (§0–8) the verification pass should challenge

9. **Cross-reference drift, mechanical.** §1.6 ("See §10"), §2.6 ("(§10)"), §3.3 ("→ §10"), §4.4 ("(§10)") and §5.4 ("§10, partial state after a crash", "reconciliation (§10)") all mean **§9**. §6.5's "sticky per container name (§11)" means **§10**. §5.4's "(§12)" and §2.6's "the alternative libraries all drag in tokio (§12)" are correct. Fix the pointers; do not renumber the sections.
10. **§5.4's `[R] MED` recommending `nftnl` should be downgraded or reversed.** New evidence in §12.4: netavark — Podman's only supported backend as of Podman 6.0 — uses the `nftables` crate (typed JSON → `nft -j -f -`) in production, not `nftnl`; `nftnl-sys` carries 26 per-libnftnl-version bindgen outputs behind 24 cumulative Cargo features, which is a real distribution problem for a portable binary; and `nftnl`/`mnl` both took memory-corruption RUSTSEC advisories on 2025-10-18. §5.4's own fallback ("`nft -f -` is an honest stopgap") is in fact the production answer, not a stopgap.
11. **§5.3's `[LOW]` on the iptables-removal timeline is now settled and should be upgraded to `[C] HIGH`.** Podman's `RELEASE_NOTES.md`, 6.0.0 Breaking Changes, verbatim: *"Support for running on iptables has been removed. Please use nftables instead."* The same list also removes CNI (*"Support for CNI networking has been removed. Please use Netavark instead."*) and slirp4netns (*"Support for the slirp4netns rootless network stack has been removed. Please use Pasta instead."*). Source: <https://github.com/containers/podman/blob/main/RELEASE_NOTES.md>. This also retroactively strengthens §5.4 and §8.3 from "direction of travel" to "the ecosystem has finished moving".
12. **§1.1's `[LOW]` mechanism for `ip_forward = 1` in a fresh netns** is still open. Two hosts, one kernel each, and the mechanism (a mutated `ipv4_devconf` template) is a guess. **Settle with:** `devinet_init_net()` for the exact kernel, plus a repeat on a host whose `ip_forward` is 0 — which is easy on the coming local VM and is the cleanest test in the file.
13. **§6.1's `[MED]` on *why* the embedded resolver binds a random high port** rather than `:53`. The conflict-avoidance explanation is the only one consistent with the design but is not sourced. **Settle with:** the libnetwork resolver's commit history. Note §6.5 stage 3 already recommends *not* copying the design, so this is curiosity, not a blocker.
14. **§8.7's flagged 10× discrepancy on pasta at MTU 1500** (RootlessKit 0.24 Gbps vs. passt's own CI 2.5 Gbps) is still unresolved. Both benchmarks on one host would settle it. Note that §13.4 item 5 leans on pasta, so this is worth closing.
15. **§3.1's `[O]` list of "the three bridges" should be re-read in light of §9.2.** It records `br-0d4cf39c58f7 172.19.0.1/16 NO-CARRIER` as ordinary state alongside `docker0`. One of those two is a normal idle bridge and the other is an orphan with no owning network; the section treats them identically. The observation is correct, the framing under-reads it.
16. **§4.4's `[F] MED` on what `docker-proxy` does internally** (accept/connect/copy, per-source-tuple UDP map) is inferred from behaviour, not from source. Low stakes — §4.4 recommends writing our own — but if it is going to be our design template it should be read: `moby/cmd/docker-proxy/`.
17. **§2.6's `[O] MED` that `ip link set v1 netns /proc/<pid>/ns/net` is rejected while the pid form is accepted** was observed on one iproute2 version on `old-host` (4.18-era). Worth re-checking on the 6.11 host, since the claim is used to argue that even `ip`'s argument grammar is unstable.
18. **Minor numbering nit:** §4.4 calls IPv6 dual-stack "a stage-6 item", but §11's ladder (which matches §3.2's "stage-4 model" for bridge+NAT) puts DNS at stage 6. Read §4.4's as "stage 6+".

---

## 15. Sources

Consolidated. Items marked † are cited in §0–8 by name only (no URL in the original text) and the URL here is my reconstruction — verify before quoting. **Known gap:** a handful of §8's numbers (the RootlessKit CI tables, the passt CI figures, the `slirp4netns.1` and `passt.1` man-page quotations, Podman 5.0.0's release note, and moby issues #14041/#22054/#48815) are cited in the text without URLs and I could not recover the exact pages consulted; the canonical entry points are listed below instead.

**Kernel documentation & man pages**
- `network_namespaces(7)` — <https://man7.org/linux/man-pages/man7/network_namespaces.7.html>
- `namespaces(7)` — <https://man7.org/linux/man-pages/man7/namespaces.7.html>
- `veth(4)` — <https://man7.org/linux/man-pages/man4/veth.4.html>
- `netlink(7)` — <https://man7.org/linux/man-pages/man7/netlink.7.html>
- `rtnetlink(7)` — <https://man7.org/linux/man-pages/man7/rtnetlink.7.html>
- `setns(2)` — <https://man7.org/linux/man-pages/man2/setns.2.html>
- `unshare(2)` — <https://man7.org/linux/man-pages/man2/unshare.2.html>
- `ip-netns(8)` — <https://man7.org/linux/man-pages/man8/ip-netns.8.html>
- `ip-sysctl.txt` (`ip_forward`, `route_localnet`, `ping_group_range`, `ip_unprivileged_port_start`) — <https://www.kernel.org/doc/Documentation/networking/ip-sysctl.txt>
- Linux bridge + netfilter — <https://www.kernel.org/doc/html/latest/networking/bridge.html>
- † `ip-link(8)` (bridge and bridge_slave sections) — <https://man7.org/linux/man-pages/man8/ip-link.8.html>
- † `slirp4netns(1)` — <https://github.com/rootless-containers/slirp4netns/blob/master/slirp4netns.1.md>
- † `passt(1)` / `pasta(1)` — <https://passt.top/passt/about/>

**Kernel source**
- `net/core/net_namespace.c` (`netns_install`, the two-userns check) — <https://raw.githubusercontent.com/torvalds/linux/v6.11/net/core/net_namespace.c>
- `net/core/rtnetlink.c` (`CAP_NET_ADMIN` gate, `rtnl_get_net_ns_capable`) — <https://raw.githubusercontent.com/torvalds/linux/v6.11/net/core/rtnetlink.c> and <https://git.kernel.org/pub/scm/linux/kernel/git/torvalds/linux.git/plain/net/core/rtnetlink.c>
- `drivers/net/veth.c` (no capability check of its own) — <https://git.kernel.org/pub/scm/linux/kernel/git/torvalds/linux.git/plain/drivers/net/veth.c>
- `net/ipv4/devinet.c` (`inetdev_init`, the loopback address) — <https://git.kernel.org/pub/scm/linux/kernel/git/torvalds/linux.git/plain/net/ipv4/devinet.c>
- `include/uapi/linux/netlink.h` — <https://git.kernel.org/pub/scm/linux/kernel/git/torvalds/linux.git/plain/include/uapi/linux/netlink.h>
- † `net/ipv4/af_inet.c` (`inet_port_requires_bind_service`, §8.5) — <https://git.kernel.org/pub/scm/linux/kernel/git/torvalds/linux.git/plain/net/ipv4/af_inet.c>

**iproute2 source**
- `lib/libnetlink.c` (`addattr_l`, the alignment rule) — <https://git.kernel.org/pub/scm/network/iproute2/iproute2.git/plain/lib/libnetlink.c>
- `ip/link_veth.c` — <https://git.kernel.org/pub/scm/network/iproute2/iproute2.git/plain/ip/link_veth.c>
- `ip/ipaddress.c` — <https://git.kernel.org/pub/scm/network/iproute2/iproute2.git/plain/ip/ipaddress.c>
- `ip/iproute.c` — <https://git.kernel.org/pub/scm/network/iproute2/iproute2.git/plain/ip/iproute.c>
- † `ip/ipnetns.c` (`netns_add`, §1.3) — <https://git.kernel.org/pub/scm/network/iproute2/iproute2.git/plain/ip/ipnetns.c>

**Specifications**
- OCI Runtime Spec, namespaces — <https://github.com/opencontainers/runtime-spec/blob/main/config-linux.md#namespaces>
- OCI Runtime Spec, POSIX hooks — <https://github.com/opencontainers/runtime-spec/blob/main/config.md#posix-platform-hooks>
- CNI Specification (DEL idempotency, GC, STATUS, runtime responsibilities) — <https://github.com/containernetworking/cni/blob/main/SPEC.md>
- RFC 1918, private address space — <https://www.rfc-editor.org/rfc/rfc1918>
- RFC 3021, /31 point-to-point links — <https://datatracker.ietf.org/doc/html/rfc3021>
- RFC 4291 §2.6.1, IPv6 Subnet-Router anycast — <https://datatracker.ietf.org/doc/html/rfc4291#section-2.6.1>

**Docker / Podman documentation**
- Docker networking overview — <https://docs.docker.com/engine/network/>
- Port publishing (gateway modes, IPv6→IPv4 proxying, `route_localnet` history) — <https://docs.docker.com/engine/network/port-publishing/>
- Packet filtering & firewalls (`DOCKER-USER`, ufw) — <https://docs.docker.com/engine/network/packet-filtering-firewalls/>
- Docker Engine 28 release notes — <https://docs.docker.com/engine/release-notes/28/>
- Docker Engine 25.0 release notes (MAC address changes) — <https://docs.docker.com/engine/release-notes/25.0/>
- Live restore — <https://docs.docker.com/engine/daemon/live-restore/>
- Docker Engine 28 hardening blog — <https://www.docker.com/blog/docker-engine-28-hardening-container-networking-by-default/>
- Podman `RELEASE_NOTES.md` (6.0.0 removals: iptables, CNI, slirp4netns) — <https://github.com/containers/podman/blob/main/RELEASE_NOTES.md>
- Fedora change: netavark nftables by default — <https://fedoraproject.org/wiki/Changes/NetavarkNftablesDefault>
- netavark firewalld integration — <https://github.com/containers/netavark/blob/main/docs/netavark-firewalld.7.md>
- netavark iptables-deprecation discussion — <https://github.com/containers/netavark/discussions/1332>

**Implementation source (mature reference implementations)**
- moby, default IPAM pools — <https://github.com/moby/moby/blob/master/daemon/libnetwork/ipamutils/utils.go>
- moby, IPAM allocator (reserved addresses, `RequestAddress`) — <https://github.com/moby/moby/blob/master/daemon/libnetwork/ipams/defaultipam/allocator.go>
- moby, IPAM address space (in-memory, mutex) — <https://github.com/moby/moby/blob/master/daemon/libnetwork/ipams/defaultipam/address_space.go>
- moby, IPAM contract (`RequiresRequestReplay`) — <https://github.com/moby/moby/blob/master/daemon/libnetwork/ipamapi/contract.go>
- moby, controller init (`reservePools`, cleanup order) — <https://github.com/moby/moby/blob/master/daemon/libnetwork/controller.go>
- moby, `sandboxRestore` / "Removing stale sandbox" — <https://github.com/moby/moby/blob/master/daemon/libnetwork/sandbox_store.go>
- moby, `cleanupLocalEndpoints` (mark & sweep) — <https://github.com/moby/moby/blob/master/daemon/libnetwork/endpoint.go>
- moby, `networkCleanup` (`inDelete` tombstones) — <https://github.com/moby/moby/blob/master/daemon/libnetwork/store.go>
- moby, `InferReservedNetworks` / on-link routes — <https://github.com/moby/moby/blob/master/daemon/libnetwork/netutils/utils_linux.go>
- moby, bridge driver + `clearConntrackEntries` — <https://github.com/moby/moby/blob/master/daemon/libnetwork/drivers/bridge/bridge_linux.go>
- CNI host-local, disk backend (`O_EXCL`, `last_reserved_ip`) — <https://github.com/containernetworking/plugins/blob/main/plugins/ipam/host-local/backend/disk/backend.go>
- CNI host-local, `flock` — <https://github.com/containernetworking/plugins/blob/main/plugins/ipam/host-local/backend/disk/lock.go>
- CNI host-local docs (`dataDir`) — <https://cni.dev/plugins/current/ipam/host-local/>
- CNI host-local IP-leak report — <https://github.com/containernetworking/plugins/issues/498>
- netavark, `Cargo.toml` (the crate set) — <https://github.com/containers/netavark/blob/main/Cargo.toml>
- netavark, synchronous netlink socket — <https://github.com/containers/netavark/blob/main/src/network/netlink.rs>
- netavark, nftables firewall driver — <https://github.com/containers/netavark/blob/main/src/firewall/nft.rs>
- netavark, firewall state persistence + `flock` — <https://github.com/containers/netavark/blob/main/src/firewall/state.rs>
- netavark, `firewall-reload` (recovery by replaying setup) — <https://github.com/containers/netavark/blob/main/src/commands/firewall_reload.rs>
- † RootlessKit port drivers (`builtin`, `SCM_RIGHTS`, `IP_TRANSPARENT`) — <https://github.com/rootless-containers/rootlesskit/tree/master/pkg/port/builtin>
- † RootlessKit benchmarks (§8.7) — <https://github.com/rootless-containers/rootlesskit/blob/master/docs/network.md>
- † slirp4netns `main.c` — <https://github.com/rootless-containers/slirp4netns/blob/master/main.c>
- † libslirp `src/ip_icmp.c` — <https://gitlab.freedesktop.org/slirp/libslirp/-/blob/master/src/ip_icmp.c>

**Rust crates**
- `rtnetlink` (manifest: `default = ["tokio_socket"]`) — <https://github.com/rust-netlink/rtnetlink/blob/main/Cargo.toml>
- `netlink-sys` (manifest: `default = []`) — <https://github.com/rust-netlink/netlink-sys/blob/main/Cargo.toml>
- `netlink-packet-route` link attributes — <https://github.com/rust-netlink/netlink-packet-route/blob/main/src/link/attribute.rs>
- `netlink-packet-route` `InfoVeth` (and the no-`fill_info` note) — <https://github.com/rust-netlink/netlink-packet-route/blob/main/src/link/link_info/veth.rs>
- `netlink-packet-route` bridge-port flags (`Isolated`, `HairpinMode`) — <https://github.com/rust-netlink/netlink-packet-route/blob/main/src/link/link_info/bridge_port.rs>
- `nftables-rs` helper (`nft -j -f -`) — <https://github.com/nftables-rs/nftables-rs/blob/main/src/helper.rs>
- `nftnl-rs` (26 per-version bindgen outputs) — <https://github.com/mullvad/nftnl-rs>
- `rustables` — <https://gitlab.com/rustwall/rustables>
- `rust-iptables` (subprocess + version sniffing) — <https://github.com/yaa110/rust-iptables/blob/master/src/lib.rs>
- RUSTSEC-2025-0126 (`nftnl`, heap overflow) — <https://github.com/rustsec/advisory-db/blob/main/crates/nftnl/RUSTSEC-2025-0126.md>
- RUSTSEC-2025-0142 (`mnl`, unsound `cb_run`) — <https://github.com/rustsec/advisory-db/blob/main/crates/mnl/RUSTSEC-2025-0142.md>
- RUSTSEC-2021-0119 (`nix`, `getgrouplist`) — <https://github.com/rustsec/advisory-db/blob/main/crates/nix/RUSTSEC-2021-0119.md>

**Issue reports used only for undocumented behaviour (weakest tier, all `[LOW]`)**
- live-restore & iptables recreation cost — <https://github.com/moby/moby/issues/34906>
- live-restore & duplicated rules — <https://github.com/moby/moby/issues/31647>
- live-restore & firewall rules — <https://forums.docker.com/t/live-restore-does-not-restore-iptables-rules/136677>
- Docker 25 MAC address conflicts — <https://github.com/moby/moby/issues/47146>, <https://github.com/moby/moby/issues/47228>
