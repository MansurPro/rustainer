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
4026532363 net   1   82271 do-agent  0 /run/docker/netns/232afd7802e3   redis-server
4026532437 net  12 2590614 root      4 /run/docker/netns/6e4515e9aedd   gunicorn...
4026532504 net   2 2588141 root      2 /run/docker/netns/86cff02e041f   nginx
4026532578 net   2   82533 root      3 /run/docker/netns/1a55b560671c   certbot loop
4026532709 net   5 2585529 root      1 /run/docker/netns/c6a22fa7f63a   nginx
$ ls -la /run/docker/netns/
-r--r--r-- 1 root root 0 Mar  2 21:50 232afd7802e3
-r--r--r-- 1 root root 0 Aug 12 00:46 6e4515e9aedd
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
See §9.

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
`[R]` **HIGH** — **Use this.** It collapses "create pair, look up peer ifindex, move peer, rename peer" into **one** message, and it eliminates the window in which the container-side veth exists in the host netns — which is both a race (another process could see or grab it) and a leak (if we die between create and move, a stray veth is left in the host netns; see §9). Strictly better on correctness *and* fewer lines. Tradeoff: none that I found.

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

`[R]` **HIGH** — **Do not shell out to `ip`.** This is a trap, not a shortcut. Specific reasons, in order of how much they will hurt: (1) `ip` reports failures as English on stderr with exit code 1/2/255, so we cannot distinguish "already exists" from "permission denied" from "no such device" without parsing prose — and *idempotent, resumable* setup (§9) depends entirely on telling those apart; (2) `ip link set X netns <pid>` takes a pid, and we established in §2.3 that pids are racy; there is no way to pass an fd through the CLI; (3) `ip netns exec` requires the `/var/run/netns` convention we deliberately are not using (§1.2); (4) `[O]` MED — behaviour differs across versions: on `old-host`'s iproute2, `ip link set v1 netns /proc/<pid>/ns/net` failed with `Error: argument "/proc/1074772/ns/net" is wrong: Invalid "netns" value` while the pid form was accepted, so even the argument grammar is not stable; (5) fork+exec per operation, ~12 execs per container start. The one legitimate use of `ip` is as an oracle while developing — run `ip -d link show` to check our work, and `strace -e sendmsg ip link add ...` to compare our bytes against iproute2's.

---

## 3. The bridge model

### 3.1 What a Linux bridge actually is

`[F]` **HIGH** — A `bridge` device is an **in-kernel software IEEE 802.1D learning Ethernet switch**. It maintains a forwarding database (FDB) mapping MAC → port, learned from source addresses of received frames, aged out after `ageing_time` (default 300s; Docker leaves it at 30000 centiseconds = 300s). Frames to a known unicast MAC go to that one port; unknown-unicast, broadcast, and multicast are flooded to all forwarding ports. It operates at L2 and does not need an IP address to switch.
Source: <https://www.kernel.org/doc/html/latest/networking/bridge.html>, `ip-link(8)` bridge section.

`[O]` **HIGH** — Live on `host-a`, the FDB shows exactly this, learned vs static:
```
$ bridge fdb show br br-5e08a4608e57
02:00:00:e6:3f:5e dev br-5e08a4608e57 vlan 1 master br-5e08a4608e57 permanent   <- bridge's own MAC
02:00:00:ae:0a:b4 dev veth28c3b49 vlan 1 master br-5e08a4608e57 permanent       <- the veth's own MAC
02:00:00:dd:79:00 dev veth28c3b49 master br-5e08a4608e57                        <- LEARNED: container MAC
```
No `permanent` ⇒ dynamically learned. And `ip -d link show br-5e08a4608e57` reports `fdb_n_learned 3` — three container MACs learned across five ports.

`[F]` **HIGH** — Giving the bridge an IP address turns it into **a switch plus a router interface on that switch**. This is the key conceptual move: `172.18.0.1/16` on `br-5e08a4608e57` means (a) the host has an interface on the container L2 segment, (b) the kernel installs a connected route `172.18.0.0/16 dev br-…`, and (c) the address can serve as the containers' default gateway. Everything else follows from ordinary IP routing.

`[O]` **HIGH** — On `host-a`, the three bridges and their state:
```
4:  docker0            172.17.0.1/16   <NO-CARRIER,...,UP> state DOWN   (route: linkdown)
26: br-ac0ab0eb6175    172.19.0.1/16   <NO-CARRIER,...,UP> state DOWN   (route: linkdown)
67: br-5e08a4608e57    172.18.0.1/16   <BROADCAST,...,UP,LOWER_UP> UP   5 veth ports
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
-A POSTROUTING -s 172.18.0.0/16 ! -o br-5e08a4608e57 -j MASQUERADE
```
Read as: *any packet whose source is in the container subnet and which is leaving via any interface other than the bridge itself gets its source address rewritten to the address of the outgoing interface.* The `! -o <bridge>` exclusion is essential — without it, container-to-container traffic on the same bridge would also be masqueraded, so every container would see every peer as coming from `172.18.0.1` and container-to-container source IPs would be destroyed.

**Why this rule is load-bearing** `[F]` **HIGH**: the rest of the internet has no route to `172.18.0.0/16` (it is RFC 1918 private space — <https://www.rfc-editor.org/rfc/rfc1918>). A container's SYN would arrive at the destination with source `172.18.0.4`, and the SYN-ACK would be dropped or blackholed because there is no path back. MASQUERADE rewrites the source to the host's public `198.51.100.10`, conntrack remembers the mapping, and the reply is un-rewritten on the way back in. **Without this one rule, containers can send packets and never receive a reply — the failure looks like "DNS times out, everything times out", not like a permission error.**

`[O]` **HIGH** — The counters prove it is carrying the traffic: `ip saddr 172.18.0.0/16 oifname != "br-5e08a4608e57" counter packets 51369 bytes 3378872 masquerade`.

`[F]` **MED** — `MASQUERADE` vs `SNAT`: `MASQUERADE` looks up the outgoing interface's primary address *per connection*, so it survives a changing host address (DHCP, failover); `SNAT --to-source X` is static and marginally cheaper. Docker uses MASQUERADE.
`[R]` **HIGH** — Use `MASQUERADE`. On a laptop or a DHCP cloud VM, static SNAT breaks the moment the address changes, and we have no code watching for that.

### 3.3 `net.ipv4.ip_forward` — required, and what breaks without it

`[F]` **HIGH** — `ip-sysctl.txt`: `ip_forward`, *"0 - disabled (default); not 0 - enabled. Forward Packets between interfaces."* A packet arriving on the bridge destined for a non-local address is **dropped in the routing decision** if forwarding is off — before `filter/FORWARD`, before any of our rules. It is not a firewall drop; the kernel simply refuses to be a router.

`[O]` **HIGH** — On `host-a`: `net.ipv4.ip_forward = 1`, `net.ipv4.conf.all.forwarding = 1`, and `net.ipv6.conf.all.forwarding = 0`.

**Symptom without it** `[F]` **HIGH**: container-to-container on the same bridge **still works** (that is L2 switching, no routing involved), but everything else dies — egress, published ports, container-to-host-other-interface. So the failure mode is "containers can talk to each other but nothing reaches the internet", which people misdiagnose as DNS.

`[R]` **HIGH** — Read it, and only write if 0. Combined with §1.1's warning: **`ip_forward` must be the first sysctl we touch**, because writing it resets per-interface config. And note we must set it in the **host** netns (that is where forwarding between bridge and uplink happens), which means this is a genuinely global, persistent host mutation → §9.

`[R]` **MED** — Docker 28 added `ip-forward-no-drop` and pairs enabling forwarding with setting `filter/FORWARD` policy to DROP, precisely because flipping a host into a router silently is a security change the admin did not ask for. **We should do the same: if we enable `ip_forward` ourselves, we must also install a default-deny on FORWARD, or we have just made the user's host a promiscuous router.** Tradeoff: setting `FORWARD` policy to DROP can break an existing router/VPN setup on that host, which is why Docker made it opt-out-able. Log loudly when we change either.

### 3.4 `bridge-nf-call-iptables` — what it changes, and why it is contentious

`[F]` **HIGH** — Normally, frames **switched** by a bridge between two of its own ports never enter the IP layer, so `iptables` (an L3 hook set) never sees them. The `br_netfilter` module changes this: with `net.bridge.bridge-nf-call-iptables=1`, bridged frames are pushed through the **`iptables` `FORWARD`/`PREROUTING`/`POSTROUTING` hooks** as if they were routed.
Source: <https://www.kernel.org/doc/html/latest/networking/bridge.html> ("Bridge + netfilter"), and `br_netfilter` docs.

`[O]` **HIGH** — On `host-a`, *both* knobs exist and disagree, which is the whole story:
```
$ sysctl net.bridge.bridge-nf-call-iptables
net.bridge.bridge-nf-call-iptables = 1                 <- GLOBAL: on
$ ip -d link show br-5e08a4608e57 | grep -o 'nf_call_iptables [0-9]'
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
    link/ether 02:00:00:48:c3:be
    inet 172.18.0.4/16 brd 172.18.255.255 scope global eth0
$ nsenter -t 2588141 -n ip route
default via 172.18.0.1 dev eth0
172.18.0.0/16 dev eth0 proto kernel scope link src 172.18.0.4
$ nsenter -t 2588141 -n ip neigh
172.18.0.1 dev eth0 lladdr 02:00:00:e6:3f:5e STALE     <- the bridge's own MAC
172.18.0.6 dev eth0 lladdr 02:00:00:8b:f6:75 STALE     <- a peer container
172.18.0.3 dev eth0 lladdr 02:00:00:c0:f8:b0 STALE
```
That is the *entire* container network configuration. Three items: an address with a prefix, a kernel-generated connected route, and a default route. Nothing else.

`[F]` **HIGH** — **ARP/neighbour behaviour is completely ordinary and requires nothing from us.** The container ARPs for `172.18.0.1`; the request is broadcast, floods the bridge, and the *bridge device itself* answers because it owns that address. For a peer container, the request floods, the peer answers, and the bridge learns both MACs. No proxy ARP, no static neighbour entries, no `IFLA_BRPORT_PROXYARP` needed. `[O]` confirms: the container's neighbour table holds the bridge's real MAC `02:00:00:e6:3f:5e`, matching `ip link show br-5e08a4608e57`.

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
-A POSTROUTING -s 172.18.0.0/16 ! -o br-5e08a4608e57 -j MASQUERADE
-A POSTROUTING -s 172.17.0.0/16 ! -o docker0          -j MASQUERADE
-A POSTROUTING -s 172.19.0.0/16 ! -o br-ac0ab0eb6175  -j MASQUERADE
-A DOCKER -i br-5e08a4608e57 -j RETURN
-A DOCKER -i docker0 -j RETURN
-A DOCKER ! -i br-5e08a4608e57 -p tcp -m tcp --dport 80  -j DNAT --to-destination 172.18.0.4:80
-A DOCKER ! -i br-5e08a4608e57 -p tcp -m tcp --dport 443 -j DNAT --to-destination 172.18.0.4:443
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
-A DOCKER-FORWARD -i br-5e08a4608e57 -j ACCEPT

-A DOCKER-CT -o docker0          -m conntrack --ctstate RELATED,ESTABLISHED -j ACCEPT
-A DOCKER-CT -o br-5e08a4608e57  -m conntrack --ctstate RELATED,ESTABLISHED -j ACCEPT

-A DOCKER-BRIDGE -o docker0         -j DOCKER
-A DOCKER-BRIDGE -o br-5e08a4608e57 -j DOCKER

-A DOCKER -d 172.18.0.4/32 ! -i br-5e08a4608e57 -o br-5e08a4608e57 -p tcp --dport 443 -j ACCEPT
-A DOCKER -d 172.18.0.4/32 ! -i br-5e08a4608e57 -o br-5e08a4608e57 -p tcp --dport 80  -j ACCEPT
-A DOCKER ! -i docker0          -o docker0          -j DROP
-A DOCKER ! -i br-5e08a4608e57  -o br-5e08a4608e57  -j DROP

-A DOCKER-ISOLATION-STAGE-1 -i docker0         ! -o docker0         -j DOCKER-ISOLATION-STAGE-2
-A DOCKER-ISOLATION-STAGE-1 -i br-5e08a4608e57 ! -o br-5e08a4608e57 -j DOCKER-ISOLATION-STAGE-2
-A DOCKER-ISOLATION-STAGE-2 -o br-5e08a4608e57 -j DROP
-A DOCKER-ISOLATION-STAGE-2 -o docker0         -j DROP
```
`[O]` **HIGH** — and the one most people have never seen, `iptables -t raw -S`:
```
-A PREROUTING -d 172.19.0.2/32 ! -i br-ac0ab0eb6175 -j DROP
-A PREROUTING -d 172.19.0.3/32 ! -i br-ac0ab0eb6175 -j DROP
-A PREROUTING -d 172.18.0.2/32 ! -i br-5e08a4608e57 -j DROP
-A PREROUTING -d 172.18.0.3/32 ! -i br-5e08a4608e57 -j DROP
-A PREROUTING -d 172.18.0.4/32 ! -i br-5e08a4608e57 -j DROP
-A PREROUTING -d 172.18.0.5/32 ! -i br-5e08a4608e57 -j DROP
-A PREROUTING -d 172.18.0.6/32 ! -i br-5e08a4608e57 -j DROP
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
- `-A DOCKER -i br-5e08a4608e57 -j RETURN` — **not** taken; we came in on `eth0`. This early-RETURN per bridge is how Docker prevents traffic *originating in a container* from being DNAT'd by another container's publish rule.
- `-A DOCKER ! -i br-5e08a4608e57 -p tcp --dport 80 -j DNAT --to-destination 172.18.0.4:80` — **MATCH.**

**Note what this rule does *not* say: there is no `-d`.** It matches port 80 arriving on *any* interface that isn't the bridge. So the publish is genuinely "all host addresses", including `10.0.0.5` (the private VPC address) and `10.1.0.2` (the second NIC) — a broader exposure than most people picture when they type `-p 80:80`. `[O]` HIGH — verified: the rule text has no destination match.

DNAT rewrites the destination to `172.18.0.4:80` and records the translation in the conntrack entry. `[F]` **HIGH** — a DNAT in `PREROUTING` applies **only to the first packet of a flow**; every subsequent packet is translated by conntrack directly, never re-traversing the nat table. That is why `nat` rules are cheap and why the DNAT counters (`11878` packets on :80) are far lower than the byte counts would suggest for a busy web server — they count *connections*, roughly, not packets.

**4. Routing decision.** Destination is now `172.18.0.4`. `ip route` gives `172.18.0.0/16 dev br-5e08a4608e57 proto kernel scope link src 172.18.0.1`. Not a local address ⇒ **this must be forwarded**, which requires `net.ipv4.ip_forward=1` (`[O]`: it is 1). Output interface: `br-5e08a4608e57`.

**5. `filter/FORWARD`.** Policy is `DROP` (`[O]`: `-P FORWARD DROP`), so the packet must be explicitly accepted.
- `-A FORWARD -j DOCKER-USER` — empty on this host, falls through. **This is the user's hook and it is first for a reason** (§4.6).
- `-A FORWARD -j DOCKER-FORWARD`:
  - `-j DOCKER-CT`: `-o br-… -m conntrack --ctstate RELATED,ESTABLISHED -j ACCEPT`. Our packet is `NEW`, so no match. (This chain exists as a fast path: for every packet after the first, the flow is ESTABLISHED and gets accepted here, skipping all the per-container rules below. A real optimisation once you have hundreds of published ports.)
  - `-j DOCKER-ISOLATION-STAGE-1`: matches on `-i <bridge> ! -o <same bridge>`, i.e. traffic *leaving* a Docker bridge for elsewhere. We are *entering* a bridge from `eth0`, so no match.
  - `-j DOCKER-BRIDGE`: `-o br-5e08a4608e57 -j DOCKER`. Match. Jump to `filter/DOCKER`:
    - `-A DOCKER -d 172.18.0.4/32 ! -i br-5e08a4608e57 -o br-5e08a4608e57 -p tcp --dport 80 -j ACCEPT` — **MATCH. ACCEPTED.**
    Note this rule is far more specific than the nat one: it pins the destination IP *and* the input-interface-is-not-the-bridge *and* the output-interface-is-the-bridge. It is the actual authorization; the DNAT rule was only the translation.
    - Had it not matched, `-A DOCKER ! -i br-5e08a4608e57 -o br-5e08a4608e57 -j DROP` is the catch-all: **anything from outside aimed at this bridge that isn't an explicitly published port is dropped.** This is the "unpublished ports are not reachable" guarantee at L3.
- `ufw`'s FORWARD chains are **never reached** for this packet, because `DOCKER-FORWARD` already ACCEPTed. §4.6.

**6. `nat/POSTROUTING`.** `-s 172.18.0.0/16` — our source is still the *client's* `203.0.113.9`, not a container address, so the MASQUERADE rules do not match. **Important and frequently misunderstood: inbound published-port traffic is not masqueraded. The container sees the real client IP.** `[O]` HIGH — derivable directly from the rule's `-s` match.

**7. The bridge.** Frame is built with destination MAC `02:00:00:48:c3:be` (from the host's neighbour table for `172.18.0.4`), sent out `br-5e08a4608e57`. The bridge looks up the FDB, finds that MAC on port `veth94547db`, forwards it.

**8. veth → container.** `veth94547db` (host ifindex 162) transmits; its peer `eth0` (container ifindex 2) receives. `[F]` HIGH — a veth transmit is a direct `netif_rx` into the peer in the same softirq context; there is no queueing discipline, no copy of the payload, just an skb handed across. This is why veth is fast.

**9. Inside the container netns.** `eth0` receives, `172.18.0.4` is local, the container's own netfilter hooks run (the container has its own independent `nat` table — `[O]`: the DOCKER_OUTPUT/DOCKER_POSTROUTING DNS rules, §6), and nginx's listener on `0.0.0.0:80` accepts. `[O]` HIGH: `nsenter -t 2588141 -n ss -ltnp` → `LISTEN 0.0.0.0:80 users:(("nginx",pid=2588141,fd=6))`.

### 4.3 The reverse path

nginx replies: `172.18.0.4:80 → 203.0.113.9:54321`.

1. Container routing: default via `172.18.0.1` ⇒ out `eth0` ⇒ veth ⇒ bridge ⇒ host.
2. Host `raw/PREROUTING`: source is a container IP, not destination — the anti-spoof rules match on `-d`, so no match.
3. Host conntrack: the tuple is recognized as the **reply direction** of the existing flow. The recorded DNAT is **reversed**: source is rewritten from `172.18.0.4:80` back to `198.51.100.10:80`. **This happens in conntrack, not by any rule** — there is no `SNAT` rule for this and there does not need to be one. This is the piece people look for in `iptables-save` and cannot find.
4. `filter/FORWARD`: `DOCKER-CT`'s `-o br-… ESTABLISHED` rule doesn't match (we're going *out* `eth0`, not into a bridge). `DOCKER-FORWARD`'s `-i br-5e08a4608e57 -j ACCEPT` **does** — that final catch-all accepts anything originating from a Docker bridge.
5. `nat/POSTROUTING`: `-s 172.18.0.0/16 ! -o br-… -j MASQUERADE` — the source has *already* been un-DNAT'd to `198.51.100.10` by conntrack before POSTROUTING, so this does not match. Correct: we must not double-translate.
6. Out `eth0`. The client sees a reply from the address it contacted. `[R]` — this symmetry is entirely conntrack's doing; **if conntrack is disabled or the table is full, published ports break in bizarre partial ways.** Worth a `nf_conntrack_count` check in our diagnostics. `[O]`: `133 / 262144` on `host-a`.

**Egress (container → internet)** is the other direction and the MASQUERADE rule's actual job:
1. Container → default route → bridge → host.
2. Routing: destination is external ⇒ forward out `eth0` (needs `ip_forward`).
3. `filter/FORWARD` → `DOCKER-FORWARD` → `DOCKER-ISOLATION-STAGE-1` (`-i br-… ! -o br-…` **matches**) → `STAGE-2` (`-o br-5e08a4608e57 -j DROP`, `-o docker0 -j DROP` — neither matches, we're going out `eth0`) → RETURN → `DOCKER-BRIDGE` (no match, `-o` is `eth0`) → `-A DOCKER-FORWARD -i br-5e08a4608e57 -j ACCEPT`. **Accepted.**
4. `nat/POSTROUTING`: `-s 172.18.0.0/16 ! -o br-5e08a4608e57` **MATCHES** ⇒ MASQUERADE ⇒ source becomes `198.51.100.10`.

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
iifname != "br-5e08a4608e57" tcp dport 80  counter packets 11878 bytes 648090  dnat to 172.18.0.4:80
iifname != "br-5e08a4608e57" tcp dport 443 counter packets 23507 bytes 1339956 dnat to 172.18.0.4:443
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
Tradeoff: a helper process per published port is state that outlives us if we die (§9) and a supervision burden. Mitigate by making the helper die when its socket closes and by reconciling on startup.
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
-A POSTROUTING -s 172.18.0.0/16 ! -o br-5e08a4608e57 -j MASQUERADE
-A DOCKER ! -i br-5e08a4608e57 -p tcp -m tcp --dport 80 -j DNAT --to-destination 172.18.0.4:80
```
and the identical rules through `nft list table ip nat`:
```
chain PREROUTING {
    type nat hook prerouting priority dstnat; policy accept;
    fib daddr type local counter packets 14155948 bytes 761728165 jump DOCKER
}
chain POSTROUTING {
    type nat hook postrouting priority srcnat; policy accept;
    ip saddr 172.18.0.0/16 oifname != "br-5e08a4608e57" counter packets 51369 bytes 3378872 masquerade
}
chain DOCKER {
    iifname != "br-5e08a4608e57" tcp dport 80 counter packets 11878 bytes 648090 dnat to 172.18.0.4:80
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
1. **Atomicity.** nftables applies a whole transaction or none of it. This is the single biggest win, and it lands directly on the hardest problem in this document: §9, partial state after a crash. With iptables, publishing a port is N separate `setsockopt`/netlink operations, and dying at N-1 leaves a half-configured host that is *insecure* (a DNAT with no matching filter ACCEPT, or worse, a filter ACCEPT with no DNAT). With nftables it either happened or it didn't. **This converts our nastiest class of bug into a non-issue**, and no amount of careful ordering in iptables gets us the same guarantee.
2. **Named sets and maps.** §4.5's anti-spoof rules are one-per-container-IP and grow linearly. In nftables that is a single rule against a named set, and adding a container is a set-element insert — O(1) rule count, O(1) update, and a hash lookup instead of a linear scan. Same for published-port maps (`dnat to tcp dport map @portmap`). Docker had to *back out* an ipset-based version of this in 28.0.1 to avoid `xt_set` module dependencies; with native nft, sets are core functionality with no extra modules.
3. **Priority control.** We can place our chains where we choose. iptables-nft cannot (§5.2). Given that we must coexist with ufw, firewalld, and fail2ban, being able to pick a priority is real leverage — and it lets us be a *good* neighbour deliberately rather than by accident.
4. **One namespace of our own.** `table inet mydocker` is entirely ours. We can `nft delete table` it for a clean full reset — which makes reconciliation (§9) trivial: destroy and rebuild from our own state, atomically. With iptables we must surgically remove our rules from chains we share with others, matching on rule text.
5. **`inet` family.** One table handles IPv4 and IPv6, instead of maintaining parallel `iptables` and `ip6tables` rule sets. Halves the code and removes a whole class of "we forgot the v6 rule" bugs — which, per §4.4, is a real gap in the Docker setup on this very host.
6. **Direction of travel.** netavark has already made nftables the default and deprecated iptables; Docker has built the backend abstraction. Nobody is moving the other way.

**Tradeoffs, honestly:**
- **Older kernels/distros.** nftables needs ≥ 3.13 for basics and realistically ≥ 4.18 for comfortable `nat`/`inet` support. `[O]` MED — `old-host` runs 4.18 and `nft` works there, including `type nat hook postrouting` inside an unprivileged netns (§8). RHEL 7 (3.10) would be excluded; we do not care.
- **Admin familiarity.** Users know `iptables-save`. Our rules will not appear there. `[R]` — mitigate by making `mydocker network inspect` print the actual nft ruleset, and by documenting `nft list table inet mydocker` prominently. This is a genuine ergonomic cost and we should not pretend otherwise.
- **Coexistence with an iptables-based `FORWARD DROP` policy.** If ufw or another tool sets `filter/FORWARD` policy to DROP in *its* table, our `accept` in *our* table does not override it — both hooks run, and a DROP anywhere wins. `[F]` HIGH — this is the flip side of point 3: priority buys us ordering, not authority. **We must handle this explicitly**: detect a foreign DROP policy on FORWARD and warn (§4.6's warning covers it). Docker sidesteps it by living in the same table as ufw; we won't have that option. This is the strongest argument against native nft and it is worth stating plainly. It is still outweighed by atomicity.

`[R]` **MED — superseded by §12.3; read that first.** Neither moby nor netavark uses `libnftnl`; both drive nftables through *text/JSON to `nft`/`libnftables`*, and §12.3 recommends the `nftables` (nftables-rs) crate instead. The paragraph below is kept because its premise — that the expression encoding is too large to hand-roll — is correct and is why we take *some* library.

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
172.18.0.4	4a820bcb9df9
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
`[R]` **MED** — mitigate cheaply by making IPAM assignment **sticky per container name** (§10): if a restarted container reliably gets its old IP back, stale entries are correct entries and stage 2 lasts much longer.

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
-A DOCKER-ISOLATION-STAGE-1 -i br-5e08a4608e57 ! -o br-5e08a4608e57 -j DOCKER-ISOLATION-STAGE-2
-A DOCKER-ISOLATION-STAGE-2 -o br-5e08a4608e57 -j DROP
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

This is the architectural section of this document. Everything above describes how to *build* a container network; this describes what happens when we are killed halfway through building one, and what that requires of the design. It is the networking half of the problem D2 §9 states for the runtime core, and the conclusions have to agree with it.

### 9.1 The inventory: what persists, and what frees it

`[F]` **HIGH** — Every resource we create falls into exactly one of three classes, and the classification is what drives the whole design.

| Resource | Lifetime is owned by | Freed automatically when… | Class |
|---|---|---|---|
| The netns itself | any reference: a process, an open fd, **or a bind mount** | last reference drops | **B — freed only if we drop our reference** |
| Container-side `eth0` (veth end) | the netns | netns is freed (`network_namespaces(7)`, §1.6) | **A — self-healing** |
| Host-side veth end | its peer | netns is freed → pair destroyed (§1.6, verified) | **A — self-healing** |
| Addresses/routes inside the netns | the netns | netns is freed | **A — self-healing** |
| sysctls inside the netns | the netns | netns is freed | **A — self-healing** |
| The nsfs bind mount at `/run/mydocker/netns/<id>` | **nothing** | never | **C — leaks forever** |
| The bridge device + its IP | **nothing** | never | **C — leaks forever** |
| nft/iptables rules | **nothing** | never | **C — leaks forever** |
| Host sysctls (`ip_forward`, `bridge-nf-call-iptables`) | **nothing** | never; and they are *global*, not ours | **C — leaks forever, and shared** |
| IPAM allocation (a record in *our* store) | **nothing** | never | **C — leaks forever** |
| Published-port helper processes (§4.4) | themselves | they exit | **C — orphan process** |
| conntrack entries for the container's flows | the conntrack table | timeout (days, for `ESTABLISHED` TCP) | **C — and actively harmful, see §9.2** |
| Host port bound by a helper | the helper process | helper exits | **C — orphan bind** |

`[R]` **HIGH** — **The single most important line in this table is that class A is large.** Because a netns's death destroys every virtual device, address and route inside it, and because a veth pair dies with either end (§1.6, empirically verified), the *only* thing standing between us and automatic cleanup of the container's entire network configuration is our own bind mount. Class C is short and finite: **one mount, one bridge, one rule set, one IPAM record, maybe one helper process.** That is the whole cleanup surface, and it fits in a struct.

`[R]` **HIGH** — This is the exact networking analogue of D2 §9.2's conclusion for mounts (*"do as little as possible in the host mount namespace, and everything else inside the new one"*). Restated for us: **do as little as possible in the host network namespace.** Every operation we can push inside the container netns is an operation with free rollback. The operations that cannot move — bridge, host-side enslavement, NAT rules, host sysctls — are precisely the ones we must track.

### 9.2 How each leak manifests on the next run

`[F]/[C]` **MED–HIGH** — Ranked by how bad, which is *not* the same order as how visible.

**1. A stale DNAT rule pointing at a recycled IP. Worst, and it is a security bug, not a bug.**
Container A had `172.18.0.4` and `-p 8080:80`. We die during teardown after releasing the IP but before deleting the DNAT rule. Container B starts, IPAM hands out `172.18.0.4` again, B is a database with no published ports. Now `http://host:8080/` reaches **B's port 80**. Nothing logs anything; both containers behaved correctly. The user published nothing and got an exposure. `[R]` — this single scenario dictates the teardown ordering rule in §9.8: **the IP is released last, after every rule that names it is gone.**

**2. Stale conntrack entries defeating brand-new NAT rules. Second worst, because it is invisible and looks like a kernel bug.**
`[F]` **HIGH** — netfilter evaluates `nat` table rules **only when creating a conntrack entry** (§4.2). If an entry for a tuple already exists, the new DNAT rule is never consulted for that flow. moby's own code says so verbatim (`clearConntrackEntries`, `daemon/libnetwork/drivers/bridge/bridge_linux.go`):
> *"This is required because iptables NAT rules are evaluated by netfilter only when creating a new conntrack entry. When Docker latter adds NAT rules, netfilter ignore them for any packet matching a pre-existing conntrack entry. As such, we need to flush all those conntrack entries to make sure NAT rules are correctly applied to all packets. See: #8795, #44688 & #44742."*
Two ways it bites: packets that arrived between the helper's bind and the rule insert create entries that then bypass the rule; and UDP flows to a port that has since been remapped keep going to the old destination. `[C]` — Docker flushes by endpoint IP on setup/teardown, and **by port and UDP only** when ports change on a live endpoint, because flushing by IP would kill established flows to unmodified ports.
`[R]` **HIGH** — **We must flush conntrack too, at the same two points, with the same distinction.** This is not optional polish; without it, published ports intermittently fail in a way that is essentially undiagnosable from the rule set, because the rule set is *correct*.

**3. A bridge that already exists with the wrong properties. Most common, most confusing.**
`ip link add br-foo type bridge` returns `EEXIST`. The naive reactions are both wrong: bailing out means a single crashed run permanently breaks that network's name; blindly adopting means we inherit whatever MTU, subnet, `stp_state` and addresses the previous incarnation had — possibly a *different subnet*, at which case every container we then attach gets an address the bridge does not route. `[C]` Docker takes a third path: adopt, then **verify and reconcile** (`setupVerifyAndReconcileIPv4`, gated on `bridgeAlreadyExists`).

**4. Exhausted IPAM.** `[R]` — see §10; for a /16 per network this is a non-issue for addresses and a real issue for *subnets* (Docker's default local pool holds ~31 usable /16s).

**5. An orphaned host-side veth.** `[O]` **HIGH** — §1.6 proved this is self-healing *if the peer made it into a netns*. The only window is between `RTM_NEWLINK` and the move — and §2.2's single-message create-with-peer-already-in-the-target-netns **closes that window entirely**. There is no orphaned-veth failure mode left in our design. This is worth stating because half the internet's "docker leaked veths" folklore is about a sequence we are not using.

**6. An orphan helper process holding a host port.** Manifests as `EADDRINUSE` on the next `-p 8080:80` for a container that is not running. `[R]` — Docker 28's `-use-listen-fd` (§4.4) makes this *our* bind, so the fd dies with our process; the helper cannot outlive the socket.

**7. A leaked netns bind mount.** `lsns -t net` grows without bound, each entry pinning an IP-address-worth of kernel structures and, more annoyingly, holding the veth pair alive so that the *bridge* still shows a forwarding port for a container that no longer exists.

### 9.3 `[O]` A live production leak, found while writing this section

`[O]` **HIGH** — `host-a` (Ubuntu 24.10, 6.11.0-29, Docker 28.3.1, `dockerd` pid 70234 running since 2026-03-02, host up since 2026-02-28). This was not sought; it fell out of listing routes.

```
$ ip route | grep 172.19
172.19.0.0/16 dev br-ac0ab0eb6175 proto kernel scope link src 172.19.0.1 linkdown

$ docker network ls
NETWORK ID     NAME                DRIVER    SCOPE
5e08a4608e57   webapp_default   bridge    local
b71690695ed7   bridge              bridge    local
daf6e4530759   host                host      local
6a0da1cea444   none                null      local

$ docker network inspect ac0ab0eb6175
[]
Error response from daemon: network ac0ab0eb6175 not found

$ ip -d link show br-ac0ab0eb6175
26: br-ac0ab0eb6175: <NO-CARRIER,BROADCAST,MULTICAST,UP> mtu 1500 ... bridge forward_delay 1500 ...
$ ip -o addr show br-ac0ab0eb6175
26: br-ac0ab0eb6175  inet 172.19.0.1/16 brd 172.19.255.255 scope global br-ac0ab0eb6175
$ bridge link show | grep -c ac0ab0eb6175
0                                     # zero ports

$ iptables -t nat -S POSTROUTING | grep 172.19
-A POSTROUTING -s 172.19.0.0/16 ! -o br-ac0ab0eb6175 -j MASQUERADE
$ iptables -t raw -S PREROUTING | grep 172.19
-A PREROUTING -d 172.19.0.2/32 ! -i br-ac0ab0eb6175 -j DROP
-A PREROUTING -d 172.19.0.3/32 ! -i br-ac0ab0eb6175 -j DROP

$ grep -ac "172\.19" /var/lib/docker/network/files/local-kv.db
0
$ grep -ac "ac0ab0eb6175" /var/lib/docker/network/files/local-kv.db
0
```

Read that carefully, because every clause is a separate leak:
- A **bridge device** for a network Docker does not know exists, `UP`, with **zero ports**, holding **`172.19.0.1/16`** — so `172.19.0.0/16` is unusable by anything else on this host, and `ip route` advertises it.
- A **MASQUERADE rule** in `nat/POSTROUTING` naming that bridge.
- **Two `raw/PREROUTING` anti-spoof DROP rules**, one per container IP — `172.19.0.2` and `172.19.0.3` — for two containers that have not existed for months. (`[O]` counters: `packets 0 bytes 0` on all three, so nothing is actively broken; the rules are inert.)
- Docker's own boltdb store has **no record of any of it**. The store and the host disagree, and the host is the one that is wrong.

`[O]` **HIGH** — And the mirror image of the same disagreement, in the store this time: the kv store holds **six** `docker/network/v1.0/sandbox/<id>` records while `/run/docker/netns/` holds **five** bind mounts and `docker ps` shows **five** containers. `sandbox/c35b41bcf51c…` is a record with nothing behind it.

`[R]` **HIGH** — **The lesson is not "Docker is buggy".** It is that a mature, heavily-tested container engine, on an unremarkable host, running 186 days, accumulates host state its own store cannot account for **in both directions** — orphaned host resources *and* orphaned records. Docker's reconciliation runs at daemon start (§9.4) and this daemon has not restarted since March. **Any design whose only reconciliation point is "when the long-lived process starts" will drift exactly this way.** For a daemonless engine (D2 §7.1) the corresponding trap is the opposite one: we have no long-lived process, so we have *no* natural reconciliation point at all unless we build one.

`[O]` **MED** — Cause not determined; I could not run anything mutating to reproduce. The two candidates are a `docker network rm` (or `compose down`) that failed partway, and a daemon kill during network teardown. **What would settle it:** on the dev VM, `docker network create t; docker run -d --network t --name a nginx; kill -9 $(pidof dockerd)` at each of several points in `docker network rm t`, then diff `ip link` / `iptables-save` / `local-kv.db` against a clean baseline.

### 9.4 What Docker actually does about it

`[C]` **HIGH** — **Rule 1: the daemon flushes and rebuilds its own chains at every start.** `NewIptabler()` (`daemon/libnetwork/drivers/bridge/internal/iptabler/iptabler.go`) is literally:
```go
if ipt.config.IPv4 {
    removeIPChains(ctx, iptables.IPv4)
    if err := setupIPChains(ctx, iptables.IPv4, ipt.config); err != nil { return nil, err }
    iptables.OnReloaded(func() { ... setupIPChains(...) })   // and again on firewalld reload
}
```
and `removeIPChains` (`.../iptabler/network.go:411`) removes, best-effort with a warning per failure:
`DOCKER` (nat), `DOCKER-FORWARD`, `DOCKER-BRIDGE`, `DOCKER` (filter), `DOCKER-CT`, `DOCKER-INTERNAL`, `DOCKER-ISOLATION-STAGE-1`, `DOCKER-ISOLATION-STAGE-2`, plus two historical chain names.
`[F]` **HIGH** — **`DOCKER-USER` is not in that list.** This is the primary-source confirmation of §4.6's claim: the admin's chain is created if missing and never touched again. `[R]` — copy this exactly for `MYDOCKER-USER`.

`[C]` **HIGH** — **Rule 2: clean up after previous versions of yourself.** `deleteLegacyTopLevelRules()` explicitly deletes the ipset-based `FORWARD` rules that moby ≤ 28.0.0 wrote (`--match-set docker-ext-bridges-v4 dst`), because 28.0.1 backed that design out (§4.5). A reconciliation pass is also a **migration** pass, and the two are the same code.

`[C]` **HIGH** — **Rule 3: with nftables, cleanup is one statement.** `nftabler`'s cleaner (`.../nftabler/cleaner.go`) is:
```go
// Cleanup deletes all rules created by Nftabler; it's intended to be used
// during startup, to clean up rules created by an old incarnation of the daemon
// after switching to a different Firewaller implementation.
err := nftables.RunCmd(ctx, fmt.Appendf(nil, "delete table %s %s", family, dockerTable))
```
where `dockerTable = "docker-bridges"`. One command, atomic, total. Compare the eleven-chain surgical removal above. `[R]` **HIGH** — this is §5.4's point 4 confirmed by the reference implementation: **owning a whole table converts reconciliation from surgery into `delete table`.**

`[C]` **MED–HIGH** — **Rule 4: cross-backend cleanup.** Because Docker 28 can run either backend, whichever one starts cleans up the other's leftovers. If nftables is available, `nftabler` is constructed and handed an `iptabler.NewCleaner(...)` — *"Without seeing config (interface names, addresses, and so on), the iptabler's cleaner can't clean up network or port-specific rules that may have been added to iptables built-in chains. So, if cleanup is needed, give the cleaner to the nftabler. Then, it'll use it to delete old rules as networks are restored."* If iptables is chosen, `nftabler.Cleanup()` runs first. `[R]` — a lesson in the cost of living in someone else's chains: cleanup of *shared* chains needs the original config to reconstruct the rule text, so it can only be done lazily, as networks come back.

`[C]` **HIGH** — **Rule 5: IPAM is rebuilt from the endpoint records, not persisted.** `Controller.reservePools()` (`daemon/libnetwork/controller.go:752`) walks every network in the store and, if the IPAM driver advertises `RequiresRequestReplay`, re-issues `RequestPool()` for the network and `assignAddress()` for **every endpoint found in the store**. The built-in driver sets `RequiresRequestReplay: true` (`ipams/defaultipam/allocator.go`). Every failure is `log.Warnf` and continue — no failure aborts daemon start. See §10.

`[C]` **MED** — **Rule 6: reconcile an adopted bridge rather than trusting it.** The bridge driver's setup step list contains `{config.EnableIPv4 && bridgeAlreadyExists && !config.InhibitIPv4, "setupVerifyAndReconcileIPv4", setupVerifyAndReconcileIPv4}` — *"Ensure the bridge has the expected IPv4 addresses in the case of a previously existing device."*

`[C]` **MED** — **Rule 7: avoid name collisions by construction.** `netutils.GenerateIfaceName` generates a random name, checks it with `LinkByName`, and retries **three times** before giving up. Host veth names on `host-a` are `veth` + 7 hex (`veth94547db`). `[R]` — random names mean a stale veth can never block a new container, which matters more than it sounds: it converts a hard failure into an invisible one, and given that veths are self-healing (class A) that trade is correct.

### 9.5 What CNI requires, and why the requirement is the interesting part

`[F]` **HIGH** — CNI does not just permit idempotent teardown, it **mandates** it, and the wording is worth having verbatim (`SPEC.md`):
> *"Plugins MUST accept multiple `DEL` calls for the same (`CNI_CONTAINERID`, `CNI_IFNAME`) pair, and return success if the interface in question, or any modifications added, are missing."*
> *"`DEL` command invocations are always considered best-effort - plugins should always complete a `DEL` action without error to the fullest extent possible, even if some resources or state are missing."*
> *"The container runtime must ensure that _add_ is eventually followed by a corresponding _delete_. The only exception is in the event of catastrophic failure, such as node loss."*

`[F]` **HIGH** — And the spec grew two verbs precisely because that contract is not enough in practice: **`GC`** (the runtime tells the plugin the full set of attachments that *should* exist, and the plugin removes everything else) and **`STATUS`** (is the plugin ready, without changing anything). During GC, unlike during a normal chain, *"the runtime must continue executing remaining plugins even when errors occur."*

`[R]` **HIGH** — Three things to steal, in order of value:
1. **`DEL` returning success when the resource is already gone is the contract, not a convenience.** Every teardown primitive we write treats `ENOENT`/`ENODEV`/`EADDRNOTAVAIL`/"No such file or directory" as **success**, and every one of them must be callable twice.
2. **`GC` is the reconcile pass, and its interface is the right one**: not "find orphans", but *"here is the complete set of things that should exist; delete the rest."* That is a much easier function to write correctly than an orphan detector, because it needs no bookkeeping about what we did — only about what should be.
3. **Teardown never fails the caller.** Collect errors, report them all, return the resource to a retryable state. Exactly D2 §9.4's `delete --force` requirement.

### 9.6 netavark: what a *daemonless* engine does, which is our actual case

`[C]` **HIGH** — netavark (Podman's Rust network stack, Apache-2.0) is the closest existing thing to what we are building, and it solves the no-daemon reconciliation problem with a **write-ahead replay log on disk** (`src/firewall/state.rs`):
```
$config/firewall/
   firewall-driver          -> name of the firewall driver in use
   networks/$netID          -> the network's setup config
   ports/$netID_$conID      -> the port-forward config
   firewall-reload.lock     -> flock(LOCK_EX), held while reading or removing
```
The comment on the lock names the exact race: *"This file is required to ensure that remove_fw_config is not racing against the firewall reload service, i.e. without it would be possible that we read the config files and then during re-adding the rules the file got removed. This leaves a chance that the service will add rules that should not be added anymore."*
A separate `netavark firewall-reload` entry point replays that directory. `[R]` **HIGH** — **this is the design we should copy**, and note what it is: it is D2 §9.3(3)'s *"write the cleanup list to disk (`rollback.json`) as you acquire resources"*, applied to networking, by a project that had to solve it without a daemon. D2 rated that idea MED-confidence-worth-the-phase-1-complexity; the existence of a production implementation in our exact architecture raises it.

`[C]` **HIGH** — Two more netavark details worth stealing outright:
- **Teardown ignores `SIGTERM` and `SIGINT`** (`src/commands/teardown.rs`): `signal::signal(SIGTERM, SigHandler::SigIgn)` before doing any work, with the comment that it is best-effort anyway. A Ctrl-C during teardown cannot produce a half-torn-down host. It does nothing about `SIGKILL`, which is the point of §9.8.
- **`NetavarkErrorList`** — teardown accumulates errors and keeps going, never early-returns.
- **The bridge is deliberately never deleted**: *"Some interfaces may not be deleted (bridge interfaces, for example, will not be removed)."* An empty bridge is cheap; racing every concurrent container to decide who removes it is not.

### 9.7 What this implies for our lifecycle design

**Who owns cleanup.** `[R]` **HIGH** — **The supervisor, using the same rollback list as everything else. Networking teardown does not get its own path.** D2 §9.3(3) specifies a `Vec<Cleanup>` unwound in reverse, with variants including `ReleaseIp(addr)`; D2 §9.4 specifies that `delete` and `create`-rollback are *the same function at a different starting index*. Networking must be entries in that list — `UnmountNetns(path)`, `DeleteNftTable`/`DeleteNftElements(net, ip)`, `ReleaseIp(net, addr)`, `KillHelper(pid)` — and nothing more. Reasoning: a separate network-teardown path is a second implementation of the same logic, and D2 §9.4's argument applies verbatim — *"If they're separate implementations, they will diverge, and the divergence will be a leak."* The §9.3 production leak is what divergence looks like after six months.

**Must setup be idempotent?** `[R]` **HIGH** — **Yes, and it is nearly free if we choose primitives whose repetition is harmless.** The discipline is per-operation, not global:

| Operation | Retry semantics | How we make it idempotent |
|---|---|---|
| create bridge | `EEXIST` | treat as "adopt", then verify-and-reconcile properties (Docker's rule 6) |
| add bridge address | `EEXIST` | treat as success; verify it is the address we wanted, not just *an* address |
| create veth-with-peer-in-netns | `EEXIST` on either name | our names are derived from the container id, so `EEXIST` means a previous attempt — delete and retry once, or use a random host-side name (Docker's rule 7) and never collide |
| enslave to bridge | idempotent in the kernel | no special handling |
| nft ruleset | **transaction** | rebuild our whole table, or use `add`, which is create-or-noop for tables/chains/sets |
| netns bind mount | `EBUSY` if already mounted | `umount2(MNT_DETACH)` first, ignoring `EINVAL`/`ENOENT`, then mount |
| IPAM allocation | see §10 | `O_CREAT\|O_EXCL` lease file — `EEXIST` *is* the allocator |

`[R]` **HIGH** — And the reason this matters more than "it's tidy": §2.6's rejection of shelling out to `ip` rests on it. *"`ip` reports failures as English on stderr with exit code 1/2/255, so we cannot distinguish 'already exists' from 'permission denied' from 'no such device' without parsing prose — and idempotent, resumable setup depends entirely on telling those apart."* Idempotency is the requirement that makes typed errno access load-bearing rather than aesthetic.

**Must setup be resumable?** `[R]` **MED** — **No. Restartable, not resumable.** Resuming means persisting a step cursor and reasoning about every partial state; restarting means teardown-then-setup, using code we must write anyway. Given that teardown is idempotent and best-effort by construction, restart is strictly cheaper and has no worse outcome. The one exception is anything expensive to redo — and in networking there is nothing expensive. Tradeoff: a container that fails to attach retries from zero, costing ~15 syscalls. Accept it.

**Do we need a reconcile-on-startup pass?** `[R]` **HIGH** — **Yes, and being daemonless makes it *more* necessary, not less.** §9.3 shows a daemon that reconciles at start drifting for 186 days because it never restarted. We have no start event at all. Concretely:
- **A `mydocker net gc` verb**, CNI-`GC`-shaped: *given the set of containers that should exist, delete every host resource attributable to us that is not in it.* Attribution is the whole trick, and it is why §5.4's "one table of our own" and §1.2's "our own netns directory" matter — everything we own is under `table inet mydocker`, `/run/mydocker/netns/`, and our lease directory. Anything there whose container is gone is garbage. **No bookkeeping required, only a live-set.**
- **Run it opportunistically**, not only on demand: at the start of `mydocker run`, if a cheap staleness check trips. `[R]` MED — the cheap check is comparing the count of entries in `/run/mydocker/netns/` against the count of live state dirs; equal means skip. This gives us a reconciliation point without a daemon and without a timer.
- **`[R]` LOW** — a systemd unit (`mydocker net gc` on boot) is the *correct* answer for the boot case, since a reboot clears every class-A and class-C kernel resource but leaves our on-disk records claiming they exist. D2 §6.4 already needs a boot_id check for exactly this reason on the container side; reuse it. Marked LOW because I have not checked how our state records will represent boot identity.

**Does networking teardown belong with container teardown?** `[R]` **HIGH** — **Same place, same list, but a specific position in it: networking is torn down *before* the netns reference is dropped, and the IP is released last.** See §9.8. The one part that genuinely differs from container teardown is that networking touches *shared* state (a bridge and a rule set used by other containers), so its teardown must be safe under concurrency in a way that `rmdir`ing a cgroup is not — which is §10's problem.

### 9.8 Crash consistency: is there a least-damaging order?

`[R]` **HIGH** — Yes, and it follows from four rules. `SIGKILL` can land between any two syscalls; we cannot prevent damage, only bound it.

**Rule 1 — Write the intent before the mutation.** You cannot clean up what you have no record of having started. Every class-C acquisition is preceded by a record of it, in the container's state dir (D2 §9.3(3); netavark's `firewall/` directory, §9.6). Cost: a handful of small writes per container start. `[R]` MED — do **not** `fsync` these; D2 §6.5's reasoning applies unchanged (a crash that loses the write is a crash after which the resource may not exist either, and the reconcile pass is the backstop). The record is for *our* recovery, not for durability against power loss.

**Rule 2 — Make the irreversible, shared, global things rare and early.** Host sysctls are the worst class: they are global, they are not ours, and §1.1 established that writing `net.ipv4.ip_forward` *resets every other IPv4 config parameter to defaults*. `[R]` **HIGH** — touch host sysctls **once, at network creation**, never per container, always read-then-write-only-if-needed, and `ip_forward` **first** among them. A crash mid-`docker run` must not be able to reset an admin's `rp_filter` tuning.

**Rule 3 — Order acquisitions so that a crash leaves the host closed, not open.** Two orderings matter:
- **Anti-spoof before address.** §4.5's `raw/PREROUTING -d <ip> ! -i <bridge> -j DROP` is what makes an unpublished container port unreachable from the LAN. Install it **before** the container's address exists. Crash in between ⇒ a DROP rule for an address nobody has, which is inert. The reverse order gives a live container address with no anti-spoof rule — exactly the pre-Docker-28 hole.
- **DNAT before filter-ACCEPT is harmless; the reverse is too.** A DNAT with no matching `filter` ACCEPT means the packet is translated then dropped: broken, but closed. A filter ACCEPT with no DNAT means traffic to the host's own port, which nothing is listening on: also closed. `[R]` — so this pair does not actually need an ordering rule, which is worth stating because §5.4 asserted it does. **Correction to §5.4's phrasing:** the danger it names ("a filter ACCEPT with no DNAT... or worse") is real for iptables in general but the *specific* pair is fail-closed either way. The genuine atomicity win from nftables is elsewhere: the anti-spoof set, the port map and the accept rule for a *set* of containers must agree, and a partial update to a shared map is what actually misroutes.

**Rule 4 — Order teardown so that no name or address is reusable until every reference to it is gone.** This is the rule that prevents §9.2's exposure, and it is the reverse of intuition (which says "free the scarce resource first"):
```
1. kill / close the published-port helper           (stops new inbound at the top)
2. delete our nft elements naming this container    (one transaction: portmap entries,
                                                     anti-spoof set element, accept rule)
3. flush conntrack for this container's IP and UDP ports
4. umount2(nsfs, MNT_DETACH) + unlink               <-- kills netns => veth, addrs, routes  (§1.6)
5. delete the lease file for the IP                 <-- LAST. only now may it be reused.
6. remove the container's network record
   (bridge: NOT deleted. left for the next container. §9.6)
```
`[R]` **HIGH** — Steps 2 and 5 are the load-bearing pair. Between them the address is unusable-but-unreferenced, which is safe; inverting them opens the window in §9.2. Step 3 must follow step 2 (flushing before the rules are gone lets a packet immediately re-create the entry). Step 4 is where the bulk of the resources actually die, and §1.6 already argued it is the single most important teardown action.

`[R]` **HIGH** — **And the honest bound on all of this: `SIGKILL` between steps 4 and 5 leaks a lease file, which is the *cheapest* possible residue** — a zero-byte file that the GC pass reclaims, that costs nothing while it exists, and that cannot cause a misroute. That is the property we are optimizing for: not "no leak", which is unachievable, but **"the leak that a kill produces is always the harmless one."** Contrast with the natural order (free the IP first), where a kill in the same window leaves a DNAT rule aimed at an address the allocator will hand out in thirty seconds.

`[R]` **HIGH** — **Consistency check against D2 §9.** D2 concludes: RAII guards for the failure path only; typed per-phase errors so the caller knows how far it got; an explicit ordered rollback list, persisted; `delete` and rollback are one function; `delete --force` succeeds against arbitrary corruption. Nothing above contradicts any of that — §9.7 puts networking *inside* that machinery rather than beside it, and §9.8 only specifies where in the ordered list the networking entries go. The one thing this section adds that D2 does not have is **rule 4**: for network resources, unlike mounts and cgroups, *release order is a security property*, because the resource being released (an address) is immediately reissued to someone else. D2's table rates "leaked IP from the pool" as merely "a slow exhaustion"; **it is also a misroute, and that is the worse half.** Flag back to D2.

### 9.9 Summary

| Question | Answer |
|---|---|
| What leaks? | netns bind mount, bridge + its address, nft rules, IPAM record, helper process, conntrack entries, host sysctls. Veths and everything inside the netns are self-healing (§1.6). |
| Who cleans up? | The supervisor, via D2 §9.3's single persisted rollback list. No separate network teardown path. |
| Idempotent setup? | Yes — per-operation, by choosing primitives where retry is a no-op. Requires typed errnos ⇒ requires netlink, not `ip` (§2.6). |
| Resumable setup? | No. Restartable: teardown-then-retry, reusing teardown. |
| Reconcile pass? | Yes. CNI-`GC`-shaped (`mydocker net gc`), run opportunistically on a cheap staleness check, plus a boot-time run. Attribution comes free from owning `table inet mydocker`, `/run/mydocker/netns/` and the lease dir. |
| Crash ordering | Intent-before-mutation; host sysctls once and early; anti-spoof before address; **release the IP last**. |
| Residual leak after `SIGKILL` | A zero-byte lease file. By construction. |

---

## 10. IPAM

### 10.1 What Docker actually allocates from

`[F]` **HIGH** — The built-in pools, verbatim from `daemon/libnetwork/ipamutils/utils.go`:
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
`NetworkToSplit{Base, Size}` means "chop `Base` into subnets of length `Size`". So local scope is **31 × /16 out of 172.17–172.31, then 4096 × /20 out of 192.168.0.0/16** — 4127 networks maximum, each holding 65 534 or 4094 usable addresses. Global scope (Swarm overlay) is 10.0.0.0/8 in /24s. `[C]` — the widely-quoted `{"base":"172.17.0.0/12","size":16}` in `daemon.json` is the *equivalent user-facing spelling*, not the literal built-in list.
`[O]` **HIGH** — `host-a` runs with no `/etc/docker/daemon.json` at all and has allocated `172.17.0.0/16` (docker0), `172.18.0.0/16` (`webapp_default`) and `172.19.0.0/16` (the orphan, §9.3) — first-fit, in order.

`[F]` **HIGH** — **The scarce resource is subnets, not addresses.** A /16 per network means address exhaustion inside a network is a non-event (65 534 containers). Running out of *networks* at 31 is entirely plausible on a CI box that creates a compose project per branch — and each leaked network like §9.3's permanently consumes one.

`[F]` **HIGH** — **Subnet selection avoids host routes with two heuristics, and the history of the second is the useful part.** `netutils.InferReservedNetworks` (`daemon/libnetwork/netutils/utils_linux.go`) reserves:
1. every `nameserver` address from `/etc/resolv.conf`, as a /32;
2. every **on-link** IPv4 route — `route.Scope == SCOPE_LINK && route.Dst != nil && !route.Dst.IP.IsUnspecified()`.

The comment explaining why it is *only* on-link routes is a lesson we would otherwise learn the hard way:
> *"That 2nd heuristic was originally not limited to on-links -- all non-default routes were checked. This proved to be not ideal at best and highly problematic at worst: VPN software and appliances doing split tunneling might push a small set of routes for large, aggregated prefixes… For full tunneling, especially when implemented with OpenVPN, the situation is even worse as the host might end up with the two following routes: 0.0.0.0/1 and 128.0.0.0/1. They are functionally indistinguishable from a default route, yet the Engine was treating them differently. With those routes, there was no way to use dynamic subnet allocation at all."*
> *"A subnet covered by the default route can be used, or not… The type of route says little about the availability of subnets it covers, except for on-link routes as they specifically define what subnet the current host is part of."*
Narrowed in moby PR #42598, first released in v23.0. `[F]` — no heuristic for IPv6, *"as there's no such thing as on-link routes for IPv6."*

`[R]` **HIGH** — **Copy the heuristic and the narrowing.** "Avoid subnets the host already routes" is the obvious idea and the obvious version of it is wrong: an on-link route (`scope link`, i.e. `RT_SCOPE_LINK`) means *"this host is physically part of this subnet"* and is the only route type that reliably implies a conflict. Everything else is somebody's opinion about where packets go. And note the failure mode it protects against is nasty: pick an overlapping subnet and the *host* loses connectivity to part of its own network, at container start, with no error.
`[C]` **MED** — Docker's documented fallback when the heuristic misses is "edit `default-address-pools`" or "use a static subnet". `[R]` — ours should additionally **fail loudly at network-create time** rather than silently, and name the conflicting route.

`[O]` **HIGH** — `host-a`'s routes make the heuristic concrete: on-link routes are `10.0.0.0/16 dev eth0 … scope link`, `10.1.0.0/20 dev eth1 … scope link`, `198.51.100.0/20 dev eth0 … scope link` (plus Docker's own). With Docker's default local pools none of these can collide; had the host been on `172.20.0.0/14`, first-fit would have walked past it.

### 10.2 Where the state lives, and what survives a restart

`[O]` **HIGH** — On `host-a`, one file: `/var/lib/docker/network/files/local-kv.db`, a boltdb, 131 072 bytes. Its key space, read out of the raw file:
```
docker/network/v1.0/network/<netid>/            -- network config: ipamType, ipamV4Config, ipamV4Info, ipamOptions
docker/network/v1.0/endpoint/<netid>/<epid>/    -- per-endpoint, carries the assigned address
docker/network/v1.0/endpoint_count/<netid>/
docker/network/v1.0/bridge/<netid>/             -- bridge driver's own network record
docker/network/v1.0/bridge-endpoint/<epid>/     -- bridge driver's own endpoint record
docker/network/v1.0/sandbox/<sandboxid>/
```
`[O]` **HIGH** — Addresses appear **only inside endpoint records** (`172.18.0.2/16` … `172.18.0.6/16`, five of them, matching five containers). There is **no free/used bitmap key, no `bitseq`, no `ipam/` subtree** in the file. Grep for the orphan network's subnet returns zero hits (§9.3).

`[F]` **HIGH** — That is by design, and it is the interesting architectural choice. The allocator is `addrset.AddrSet` — *"a set of IP addresses"* backed by `map[netip.Prefix]*bitmap.Bitmap` — held purely **in memory**, and the driver registers itself with `Capability{RequiresRequestReplay: true}`. libnetwork's own docs define that flag as *"whether the ipam driver needs to receive the replay of the `RequestPool()` and `RequestAddress()` requests on daemon reload"*, useful for *"ipam drivers which decide not to persist the pools allocated to local scope networks."*

`[F]` **HIGH** — So on daemon restart, `Controller.reservePools()` (§9.4 rule 5) rebuilds the bitmap by walking the store: for each network, `ipamAllocate()` re-reserves the pool; then for each endpoint found in the store, `ep.assignAddress(ipam, …)` re-reserves that exact address. `RequestPool` is contractually idempotent — *"Multiple identical calls must return the same result."* Every step logs a warning on failure and continues.

`[R]` **HIGH** — **The design principle worth extracting: the allocator is a cache, and the endpoint records are the truth.** There is exactly one source of truth for "which addresses are in use", and it is the same records that answer "which containers exist". A separate allocator database would be a second source of truth, and §9.3 shows what happens when two records of the same fact drift. Docker got this right and it is the most important thing in this section.

### 10.3 The minimum honest approach for us

`[R]` **HIGH** — **Do not build an allocator. Make the filesystem the allocator.**

```
/var/lib/mydocker/networks/<net>/
    config.json                 subnet, gateway, mtu, bridge name
    leases/172.18.0.2           O_CREAT|O_EXCL, content = container id
    leases/172.18.0.3
```
Allocation is: iterate addresses in the subnet from `.2` upward; for each, `open(path, O_CREAT|O_EXCL|O_WRONLY, 0644)`; the **first success is the allocation**, `EEXIST` means taken, keep going. Release is `unlink`, ignoring `ENOENT`.

Why this and not a bitmap:
1. **`O_CREAT|O_EXCL` is atomic across processes on every local filesystem.** It *is* a compare-and-swap. There is no lock to take, no window, and no way for two concurrent `mydocker run` to get the same address — which is the entire hard part of IPAM (§10.4). This is the same primitive D2 §6.6 already chose for container-id uniqueness (`mkdir`) and name uniqueness (`symlink`); using it a third time is consistency, not cleverness.
2. **The state is self-describing and greppable.** `ls /var/lib/mydocker/networks/*/leases/` answers "what is allocated" with no parser. Debugging IPAM at 2am is reading a directory.
3. **Reconciliation is trivial** and is exactly §9.7's GC: a lease whose content names a container that no longer exists is garbage. No bookkeeping, only the live set.
4. **There is no allocator state to rebuild on startup**, so there is no replay path, so there is no drift. Docker needs `reservePools()` because it caches in RAM; we do not cache.

Ceiling, named honestly: `[R]` — **this is O(n) `open` syscalls to find a free address in a densely-packed /24, and a linear scan of a directory to list.** For a /24 with 200 containers, worst case ~200 `open`s at ~1 µs each — irrelevant. For a /16 that is *sparsely* used it is O(1), because we scan from `.2` and hit a free slot immediately. It degrades only when a network is both large and nearly full, which is not a state a single-host engine reaches. **Upgrade path if it ever matters: keep a hint file with the last-allocated address and start the scan there.** Mark the shortcut in code with its ceiling.

`[R]` **MED** — **Sticky addresses, which §6.5 asked for.** Write the last-used address into the container's state dir. On restart, try `O_CREAT|O_EXCL` on *that* lease first; fall back to a scan on `EEXIST`. Cost: one extra `open`. Buys: §6.5's stage-2 `/etc/hosts` injection stays correct across restarts, which is the difference between that shortcut lasting a week and lasting a year.

`[R]` **HIGH** — **Reserve `.1` for the gateway and never hand out the network or broadcast address.** `[C]` — Docker's convention throughout: `172.17.0.1`, `172.18.0.1`, `172.19.0.1` are all the bridge (`[O]` verified on `host-a`). Start allocation at `.2`. `[R]` LOW→MED — also skip `.11` if we ever ship the embedded resolver on a `127.0.0.11` model; not applicable, the resolver address is loopback-scoped. Ignore.

`[R]` **HIGH** — **Subnet selection: first-fit over a fixed pool list, minus `InferReservedNetworks`.** Use `172.17.0.0/16 … 172.31.0.0/16` split into /16s. Skip `192.168.0.0/16` for now — it is far more likely to collide with a home LAN, and 15 networks is plenty before we have a real user. Read on-link routes with `RTM_GETROUTE` filtered on `rtm_scope == RT_SCOPE_LINK` (`netlink-packet-route` gives this directly), plus loopback nameservers from `/etc/resolv.conf`, and reject any candidate that overlaps. **Fail loudly and name the conflicting route** if nothing is left.

### 10.4 Concurrency: two `mydocker run` at once

D2 §6.6 settled the general rule: **`mkdir`/`symlink`-style atomic creation instead of check-then-act; one `flock(LOCK_EX)` per container; no global lock, ever.** IPAM is the first thing in this project that is genuinely *cross*-container, so it needs an answer that does not violate that rule.

`[R]` **HIGH** — **Address allocation needs no lock at all.** `O_CREAT|O_EXCL` (§10.3) is the atomicity. Two `mydocker run` racing for the same subnet both scan from `.2`; one wins each address; neither blocks. This is strictly better than a lock: no lock ordering, no deadlock, no lock held across a syscall that can fail, and — the property that matters most — **a process `SIGKILL`ed mid-allocation holds nothing**. A `flock` would also be released on death (D2 §6.6 makes exactly that argument for choosing `flock` over a pidfile), but a lock held across the whole allocate-and-configure sequence would serialize container starts for no benefit.

`[R]` **HIGH** — **Network *creation* does need one lock, and only that.** Electing a subnet is inherently check-then-act: read the host's routes, read the other networks' subnets, pick a free one. Two concurrent `mydocker run` that both auto-create the same named network, or that both auto-elect a subnet, must not both pick `172.18.0.0/16`. So: `flock(LOCK_EX)` on `/var/lib/mydocker/networks/.lock`, held **only** across `{elect subnet, mkdir the network dir, write config.json, create and address the bridge}`. Released before any per-container work. That is a few milliseconds, once per network, and it does not violate D2 §6.6's "no global lock" because it is not on the hot path — `mydocker run` on an existing network never takes it.
`[R]` MED — the alternative that avoids even this lock is `mkdir(networks/<name>)` as the election gate plus retry-on-`EEXIST`-with-the-next-subnet. It works, and it is the same trick again, but the retry loop has to distinguish "someone else created this network" from "someone else took this subnet", and the resulting code is longer than `flock`. **Take the lock here; the lazy answer is the shorter code, and this time that is the lock.**

`[R]` **HIGH** — **The nft ruleset needs no lock, because the transaction is the lock.** Two concurrent container starts each apply their own `nft` transaction adding an element to a set and a rule to our chain. nftables commits or rejects each atomically, and the kernel serializes them. `[F]` HIGH — this is §5.4's atomicity argument paying a second dividend it was not claimed for: it removes a lock we would otherwise need. With iptables, by contrast, `iptables -I` is a read-modify-write of the whole table and **two concurrent invocations can lose a rule** — which is why `iptables` grew `--wait` (an `flock` on `/run/xtables.lock`) and why every tool that shells out to it must pass `-w`. `[R]` — one more reason to target nftables.

`[R]` **HIGH** — **The bridge is created concurrently-safely by `EEXIST`.** Both racers issue `RTM_NEWLINK` with `NLM_F_CREATE|NLM_F_EXCL`; one gets `EEXIST` and adopts (§9.7). The address add is the same. No lock needed on the fast path.

`[R]` **MED** — **What we consciously give up.** There is no atomicity *across* the allocation and the configuration: a process killed between claiming `172.18.0.5` and configuring `eth0` leaves a lease with no container. That is by design — it is the harmless residue §9.8 optimizes for, and the GC pass reclaims it. The alternative (a two-phase commit over the lease) is more machinery than the failure justifies.

---

## 11. The minimal networking ladder

The proposed progression was:
`share the host netns (no isolation)` → `new netns with loopback only` → `veth pair to host, manual routing` → `veth + bridge + NAT for egress` → `published ports (DNAT)` → `DNS`.

It is right in the large and wrong in one specific place, at the bottom.

### 11.1 The correction: stage 0 is not a rung

`[R]` **HIGH** — **"Share the host netns" is not the first step toward the others; it is an orthogonal mode that must exist permanently.** `--network=host` means *omit `CLONE_NEWNET` from the clone flags* — one bit in an integer. It shares no code with anything above it, teaches nothing about networking, and is never superseded: Docker still ships it, it is the correct answer for high-throughput workloads, and §8.6 concluded it is one of the two honest answers for rootless users who need kernel-speed networking. Putting it on the ladder implies we would later "grow past" it. We won't.

`[R]` **HIGH** — The real ladder starts at what D1 §12 handed us: *"an empty netns with `lo` up is a legitimate MVP target; it makes the container look network-broken but the isolation is correct."* Corrected ladder, with `--network=host` and `--network=none` as permanent modes rather than stages:

| Stage | What it is | What the user gets | What it costs us | Host state created |
|---|---|---|---|---|
| **mode H** | omit `CLONE_NEWNET` | full host networking, zero isolation, zero overhead | **1 line** | none |
| **1** | new netns, `lo` up | correct isolation; the container is network-*silent*, not network-*broken* | ~20 lines: `unshare`/`clone`, one `RTM_NEWLINK` with `IFF_UP` (§1.1) | **1 netns bind mount** |
| **2** | + veth pair, addresses both ends, manual routes | container↔host IP connectivity; `curl http://172.20.0.1:8000` works | ~150 lines: the netlink codec, the `VETH_INFO_PEER` message (§2.2), one socket per netns (§2.6) | + nothing new (veth is class A, §9.1) |
| **3** | + bridge, `ip_forward`, MASQUERADE | **the internet.** `apt-get`, `pip install`, reaching a database | + bridge creation/enslave (2 messages), + one nft table with a masquerade rule, + a **host sysctl write** | + bridge, + nft table, + **global sysctl** |
| **4** | + published ports (DNAT, anti-spoof, filter accept, helper for v6/loopback) | inbound. A container is a *server*. | + per-container nft set/map elements, + `raw/PREROUTING` (§4.5), + a helper process for the v6→v4 and loopback cases (§4.4), + conntrack flushing (§9.2) | + rules naming a specific IP, + a bound host port, + conntrack entries |
| **5** | + DNS | container-to-container by name; working resolution on systemd-resolved hosts | §6.5 stage 1 is ~30 lines (rewrite `resolv.conf`); stage 2 ~20 (`/etc/hosts`); stage 3 is a DNS server | + two bind-mounted files per container (D3) |

`[R]` **MED** — Two orderings inside this differ from the proposal:
- **Stage 5's first two thirds belong at stage 3, not stage 5.** §6.5's stage 1 (filter loopback nameservers out of the host `resolv.conf`) is thirty lines and it is the difference between `apt-get update` working and not working on Ubuntu/Fedora/Arch, where `/etc/resolv.conf` points at `127.0.0.53` and is meaningless in a container netns (§6.3). Shipping egress without it ships a container that can reach `1.1.1.1` and cannot resolve `deb.debian.org`. **Bundle `resolv.conf` rewriting into stage 3.** Only the resolver (§6.5 stage 3) is genuinely stage 5.
- **Stage 2 is optional for the user and mandatory for us.** No one wants a container reachable only from its own host by a hardcoded /30. But it is the stage where the entire netlink layer gets written and debugged against one veth pair instead of against a bridge, a NAT table and a DNAT rule simultaneously. `[R]` — **build it, ship it behind nothing, and treat it as a test fixture rather than a feature.**

### 11.2 The three questions, answered

**Which stage is the first genuinely useful one?**

`[R]` **HIGH** — **Stage 3 (veth + bridge + NAT egress, with `resolv.conf` rewriting folded in).** Not stage 1, not stage 2, and the reasoning is about what images actually do. A container image's first act is almost always to talk *out*: `apt-get`, `pip`, `npm`, a health check, a database connection, telemetry. A container that cannot reach the network is not a container you can run real images in; it is a chroot with extra steps. Stage 3 is the first point at which `mydocker run -it alpine sh` followed by `apk add curl` works, and that is the demo that means the engine exists.

`[R]` **HIGH** — But be precise about what stages 1 and 2 are worth, because "not useful to a user" ≠ "not worth shipping":
- **Stage 1 is worth shipping and is the correct MVP**, exactly as D1 §12 said. It is *complete and correct* at what it does: the isolation is real, `--network=none` is a mode Docker ships, and a container with a working loopback is genuinely usable for CPU-bound work and for anything that talks over a bind-mounted socket. It is a shippable product, not a stub. Its honest label is "we have namespaces, we do not have networking".
- **Stage 2 is not shippable as a feature and is the most valuable stage for us.** It is where §2's entire netlink budget ("a weekend for the happy path, ~a week to be solid") is spent.
- **Stage 4 is where users stop asking.** Egress makes the engine usable; published ports make it *replace* Docker for someone's side project.

**Which stage introduces the most host-state risk?**

`[R]` **HIGH** — **Stage 4, by a wide margin — but the single most dangerous *operation* is at stage 3.** Separating those two claims matters:

*Stage 4 carries the most risk*, because it is the only stage that simultaneously: (a) writes rules naming a **specific container IP**, which is the ingredient in §9.2's misroute; (b) **binds a host port**, creating an orphan-able exclusive resource; (c) creates **conntrack entries** whose staleness silently defeats correct rules (§9.2); (d) mutates a ruleset shared with `ufw`/`firewalld`/`fail2ban`, where our ACCEPT can silently override the admin's intent (§4.6) — the one leak whose blast radius is "the user's server was exposed and nothing said so"; and (e) is per-container, so every start and stop touches host state, N times a day rather than once per network.

*But the single most dangerous operation is stage 3's `net.ipv4.ip_forward` write*, for the reason §1.1 established from `ip-sysctl.txt`: *"This variable is special, its change resets all configuration parameters to their default state."* One careless blind write silently discards an admin's `rp_filter`, `accept_local`, `arp_ignore` and `route_localnet` tuning across every interface on the host. It is a single `write(2)` that can degrade a machine's security posture in a way no log line reports and no `git diff` shows. `[R]` — read-then-write-only-if-zero, first among sysctls, once per network, never per container.

`[R]` **MED** — Stage 3 also introduces the first *shared* host object, the bridge, and therefore the first "who deletes it?" question. §9.6's answer stands: nobody does.

**Where does the OCI boundary sit — and should networking be a hook?**

`[F]` **HIGH** — A hooks-based design is fully expressible, and here is what it would be. `config.json`:
```json
{ "linux": { "namespaces": [ {"type": "network"} ] },
  "hooks": {
    "createRuntime": [ {"path": "/usr/bin/mydocker-net", "args": ["mydocker-net","add"]} ],
    "poststop":      [ {"path": "/usr/bin/mydocker-net", "args": ["mydocker-net","del"]} ] } }
```
`createRuntime` is the right hook and only that one: it runs **after** the namespaces exist and **before** `pivot_root`, and **in the runtime's namespace**, so the hook can reach `/proc/<pid>/ns/net` (§0). The container `state` JSON arrives on the hook's **stdin** — `ociVersion`, `id`, `status`, **`pid`**, `bundle`, `annotations` — which is exactly the handle needed.

`[F]` **HIGH** — Three properties of the hook boundary decide the question, from `runtime.md`'s lifecycle:
1. *"If any `createRuntime` hook fails, the runtime MUST generate an error, stop the container, and continue the lifecycle at step 12."* — **setup failure is fatal. Good.**
2. *"If any `poststop` hook fails, the runtime MUST log a warning, but the remaining hooks and lifecycle continue as if the hook had succeeded."* — **teardown failure is, by spec mandate, ignored.** Everything in §9 about leaks becomes not merely possible but *specified* behaviour.
3. **A hook has no return channel.** stdin carries state *in*; the only thing that comes back is an exit code and stderr. So a hook that allocates `172.18.0.5` cannot tell the runtime what it allocated. `mydocker inspect` would have to read the IP back out of the container's netns, or the hook would have to write it to a side-channel file — at which point the "clean process boundary" has an undocumented shared-state edge running through it.

`[R]` **HIGH** — **Build networking in. Do not make it a hook. Keep §0's CNI-shaped internal trait.** This confirms §0's `[R] MED` and raises it to HIGH on the strength of points 2 and 3. Specifically:
- Point 3 is the decisive one for *design*: an allocation that cannot be reported is not an interface, it is a side effect.
- Point 2 is the decisive one for *correctness*: §9.8 spends its whole argument on teardown ordering, and a boundary whose contract is "teardown failures are ignored" throws that away. We would end up re-implementing reconciliation *above* the hook anyway, which is the boundary paying for itself twice.
- The process isolation a hook buys is isolation we do not want (§0), and the JSON serialization is a cost with no matching benefit when both sides are our code.

`[R]` **MED** — **But do *support* hooks, for other people's plugins.** Honour `createRuntime`/`poststop` in our `config.json` handling when we become OCI-compatible, and keep our internal networking trait shaped like CNI's ADD/DEL/CHECK/GC so that a future "shell out to real CNI plugins" is a wrapper rather than a rewrite. The four verbs are the right factoring regardless of whether they cross a process boundary — and §9.5 showed that CNI's `GC` verb is precisely the reconcile pass §9.7 needs, which is a good sign the shape is right.

`[R]` **HIGH** — And the boundary that *does* stay firm, restating §0: **the runtime core does not do networking.** `runc` does not; neither should ours. The netns is created by the process that clones the container (D1 §12: *"netns plumbing must be done by the parent and must complete before the child drops capabilities and installs seccomp"*), and everything else — IPAM, bridge, veth, rules, DNS files — happens in the layer above, in the supervisor, between `create` and `start`. That layering is what makes the exec-fifo rendezvous (D2 §3.2) useful for something other than ceremony: the container is frozen at the fifo while we plumb its network.

---

## 12. Rust crates for networking

All figures from the crates.io API and the published `.crate` tarballs, read 2026-09-05. "unsafe" counts are occurrences of the token `unsafe ` across `*.rs` in the published crate; they are a proxy for audit surface, not a defect count.

### 12.1 `rtnetlink` is async by construction — and it collides with D2 §8

`[F]` **HIGH** — This needs stating first because it decides the rest of the netlink question.

`rtnetlink` 0.23.0 (2026-08-18, `rust-netlink/rtnetlink`, MIT, 25.4M lifetime / 4.26M last-90d downloads) has **no synchronous API at all.** Its only constructors are in `src/connection.rs`:
```rust
#[cfg(feature = "tokio_socket")]
pub fn new_connection() -> io::Result<(Connection<RouteNetlinkMessage>, Handle,
                                       UnboundedReceiver<(NetlinkMessage<...>, SocketAddr)>)>
pub fn new_connection_with_socket<S>() -> io::Result<(Connection<RouteNetlinkMessage, S>, ...)>
    where S: AsyncSocket
```
Both return a `netlink_proto::Connection`, which **is a `Future`** that the caller must drive; the generic form is bounded on `S: AsyncSocket`. `Cargo.toml`: `default = ["tokio_socket"]`, with `smol_socket` as the only alternative. There is no `S: Socket` escape hatch and no blocking mode. **Using `rtnetlink` means running an async executor in whatever process calls it.**

`[F]` **HIGH** — **This is a direct collision with D2 §8, whose central conclusion is a structural one**, quoted:
> *"The container creation path must run in a process that has never had a second thread. Not 'a single-threaded runtime'. Not 'we're careful'. **Never had a second thread**, because a thread that has exited can still have left a lock held at the moment it was cancelled, and because you cannot audit every transitive dependency for thread spawning."*
and the enforcement rule:
> *"put the pull path in a separate process, not just a separate module… Alternative considered and rejected: one binary with `#[tokio::main]` on `pull` only and plain `fn main` on `supervise` — this **works** but it is a discipline invariant rather than a structural one… **Structural beats disciplined.**"*

`[F]` **MED** — Is the collision fatal in the narrow technical sense? Not necessarily: `tokio` is pulled with `features = ["rt"]` only, so a `Builder::new_current_thread()` runtime is possible, and a current-thread runtime spawns no worker threads (the blocking pool is created lazily on first `spawn_blocking`). So one *could* run `rtnetlink` on a single-threaded runtime in the supervisor without ever creating a second thread. `[MED]` because I reasoned this from tokio's feature layout rather than verifying thread counts empirically. **What would settle it:** on the dev VM, `Builder::new_current_thread().enable_all().build()`, run one `rtnetlink` link-add, then read `/proc/self/status`'s `Threads:` — D1 §11.2 already recommends asserting `Threads == 1` before `clone()`, so the assertion is our test.

`[R]` **HIGH** — **But "not necessarily fatal" is exactly the shape of argument D2 §8.3 rejects, and it should be rejected here too.** It is a discipline invariant: it holds until someone enables a tokio feature, adds a dependency that constructs a multi-thread runtime, or calls `spawn_blocking`. D2 rates getting this wrong as *"the biggest [rewrite-risk decision] in the project"* with a failure signature of *"nondeterministic deadlocks in the container's init path that reproduce once in a thousand runs."* **Do not spend that budget on a netlink convenience layer.**

`[R]` **HIGH** — **The escape routes, in order of preference. Route 1 is what we should do.**

**Route 1 — `netlink-packet-route` + `netlink-packet-core` for the codec, `netlink-sys::Socket` for the socket, synchronous, no executor.** This is the recommendation. The evidence:
- `netlink-packet-route` 0.33.0 depends on `bitflags, libc, log, netlink-packet-core, zerocopy` — **no socket, no futures, no tokio, nothing async.** It is a pure encoder/decoder. 47 572 LOC across 293 files, **0 occurrences of `unsafe`** (it uses `zerocopy` derives in 53 files instead).
- `netlink-packet-core` 0.9.0: 2 719 LOC, **0 `unsafe`**, deps `anyhow`-free and async-free.
- `netlink-sys` 0.9.0 with `default-features = false`: deps are `bytes, libc, log`. `Socket` is an ordinary blocking `RawFd` wrapper — `new`, `bind`, `bind_auto`, `send`, `recv`, `recv_from`, and typed setsockopt helpers including **`set_ext_ack`** and **`set_netlink_get_strict_chk`**, which §2.1 called *"the single highest-leverage line in our netlink code."* 1 941 LOC, 22 `unsafe` — all of it the libc `socket`/`bind`/`sendto`/`recvmsg`/`setsockopt` calls we would otherwise write ourselves. The `tokio`/`smol`/`mio` sockets are separate types behind separate features; not enabling them means not compiling them.

**Route 2 — a separate process.** `mydocker-net add|del`, a distinct binary that may use whatever it likes, invoked by the supervisor. D2 §8.3 already uses this pattern for the pull path, so the machinery exists. `[R]` — this is the fallback if we ever want `rtnetlink`'s builders badly enough, and it is genuinely acceptable; the cost is a process boundary with no return channel for the allocated IP, which is §11.2's argument against hooks arriving by another door. Prefer route 1.

**Route 3 — async confined to a "networking phase".** Rejected. It is exactly the discipline invariant above, and it puts the executor in the supervisor — the process that calls `clone()`. D2 §8.3's table says of the supervisor's own event loop: *"Introducing tokio here means the supervisor is multithreaded — and the supervisor is the process that calls `clone()`. **Hard no.**"*

`[C]` **HIGH** — **And route 1 is not theoretical: it is what netavark does.** Podman's production Rust network stack (`containers/netavark` 2.1.0, Apache-2.0) declares `netlink-packet-route = "0.33.0"`, `netlink-packet-core = "0.9.0"`, `netlink-sys = "0.9.0"` — and **does not depend on `rtnetlink` at all.** It writes its own ~150-line synchronous wrapper (`src/network/netlink.rs`):
```rust
pub struct Socket<P: NetlinkFamily> {
    socket: netlink_sys::Socket,
    sequence_number: u32,
    buffer: [u8; NLMSG_GOODSIZE],     // 8192
    _protocol: PhantomData<P>,
}
impl<P> Socket<P> {
    pub fn new() -> NetavarkResult<Socket<P>> {
        let mut socket = wrap!(netlink_sys::Socket::new(P::PROTOCOL), "open")?;
        socket.set_netlink_get_strict_chk(true)?;   // "Needs to be enabled for dump filtering to work"
        ...
```
netavark *does* depend on tokio — for the DHCP-proxy gRPC service (`tonic`, `tokio-stream`), a separate binary path. The netlink core is sync. **The most directly comparable production project in our language made exactly this split, independently.**

`[R]` **MED — a refinement of §2.6, stated as a refinement and not a reversal.** §2.6 recommends *"write it ourselves"* for netlink, on four grounds: it is a weekend; it is the learning; the alternatives drag in tokio; the message set is frozen. Grounds 1, 2 and 4 survive intact. **Ground 3 does not: `netlink-packet-route` drags in nothing.** So the honest split is finer than §2.6 drew it:
- **Write ourselves: the socket layer.** Open/bind/seq/ack/`NLMSG_ERROR`/ext-ack parsing/dump-continuation/one-socket-per-netns. ~150 lines, netavark's file is the existence proof of the size, and this is where the *understanding* lives — it is the part that teaches you what netlink is.
- **Adopt: the TLV codec.** `netlink-packet-route` already has `InfoVeth::Peer(LinkMessage)` — literally §2.2's *"one genuinely fiddly encoding"*, the embedded zeroed `ifinfomsg` inside `VETH_INFO_PEER`, implemented and round-trip tested. It also has `bridge.rs`, `netkit.rs`, and every attribute we listed in §2.6's message table. Hand-rolling `rta_len`-unaligned-but-position-aligned TLV emission (§2.1) is where hand-written encoders break first, by §2.6's own account.
- `[R]` LOW→MED — **an honest middle path if the learning goal outweighs the code:** hand-roll the codec first *as an exercise*, with `netlink-packet-route` in `dev-dependencies` as the round-trip oracle (encode with ours, decode with theirs, and vice versa). §2.6 already budgets for *"round-trip tests against a real kernel"*; this is a cheaper oracle that catches encoding bugs without a kernel. Then decide with the code in front of us. This is a genuine judgement call between two defensible answers and I am not going to pretend otherwise.

### 12.2 The rest of the netlink field

`[F]` **HIGH** — **`netlink-packet-core` 0.9.0** (2026-08-18). The `nlmsghdr` framing, `NetlinkMessage<T>`, `NetlinkPayload::{InnerMessage, Error, Done, Noop, Overrun}`, the `Nla`/`NlaBuffer`/`Emitable`/`Parseable` traits, and `ErrorMessage` including ext-ack TLV parsing. 2 719 LOC, **0 `unsafe`**. Not optional if we take `netlink-packet-route`; it is its framing layer. **Adopt.**

`[F]` **HIGH** — **`netlink-sys` 0.9.0** (2026-08-18). 22 `unsafe` in 1 941 LOC, all libc calls. Default features are `[]` — sync only. Covers the setsockopts §2.1 wants: `set_ext_ack`, `set_netlink_get_strict_chk`, `set_cap_ack`, `set_no_enobufs`, `set_rx_buf_sz`, `set_listen_all_namespaces`, `add_membership`. Notably it does **not** expose an fd-based "open in another netns" helper — that is ours, and per §2.6(4) it needs a dedicated OS thread since `setns` is per-thread. **Adopt, with `default-features = false`.**

`[F]` **HIGH** — **`neli` 0.7.4** (2026-01-28, `jbaublitz/neli`, **BSD-3-Clause**, 40.5M lifetime / 10.1M last-90d). The genuine alternative to the `rust-netlink` family, and the one that is **sync by default** — `tokio` is an optional, non-default feature. 9 497 LOC, 38 `unsafe`, plus a `neli-proc-macros` companion crate. Deps include `derive_builder`, `getset`, `linux-raw-sys`.
`[R]` **MED** — **Read as reference, do not adopt.** Two reasons. (a) It is a **general** netlink library with `NETLINK_ROUTE` as one application; its rtnetlink attribute coverage is thinner than `netlink-packet-route`'s 293 files, and specifically I did not find a `VETH_INFO_PEER` equivalent — we would hand-roll the one hard message anyway, losing the main reason to take a codec crate. (b) Maintenance is **one person**, consistently, since 2018. That is not a criticism — the release cadence (0.7.0 → 0.7.4 between June 2025 and Jan 2026) is healthy — but it is a bus factor of one on our most load-bearing dependency, versus `rust-netlink`, which is a GitHub *organisation* maintained by the nmstate/Red Hat networking people and consumed by netavark, `netlink-packet-*` and dozens of others. `[LOW]` on the "Red Hat" attribution specifically — inferred from the maintainer set and downstream consumers, not from a governance document.

`[F]` **HIGH** — **`netlink-proto` 0.13.0.** The async request/response state machine underneath `rtnetlink`. `default = ["tokio_socket"]`; its `Connection` is a `Future` regardless of feature, because it is built on `futures-channel`/`futures-util`. **This crate is the reason `rtnetlink` cannot be sync.** Not usable for us. **Skip.**

`[F]` **HIGH** — **`genetlink` 0.3.0** (2026-08-18, 887k lifetime). Generic netlink (`NETLINK_GENERIC`) — the family-id resolution dance plus `genlmsghdr`. Needed only for subsystems that live behind generic netlink: `nl80211`, WireGuard, `ethtool`, `devlink`, thermal. **Container bridge networking uses none of them.** It also depends on `netlink-proto`, so it carries the same async constraint. **Skip; revisit only if we ever add a WireGuard network driver.**

`[F]` **HIGH** — **What `nix` and `rustix` cover, and what they don't.** Both provide the *syscalls* around netlink, not netlink itself:
- `nix` 0.31.3 (2026-05-11, 772M lifetime): `sched::{clone, unshare, setns, CloneFlags}`, `mount`, `sys::socket` (including `AF_NETLINK` via `SockProtocol`/raw), `sys::signal`, `unistd`. D1 §11.1 already assesses it for the runtime core; netavark uses it with `features = ["net","sched","signal","socket","user"]`.
- `rustix` 1.1.4 (2026-02-22, 1.10B lifetime): `thread::{unshare, move_into_link_name_space, move_into_thread_name_spaces}` — i.e. `unshare(2)` and `setns(2)` — plus `net`, `fs`, `mount`. Its selling point is a `linux_raw` backend that makes syscalls directly without libc, which sidesteps a class of post-`fork` hazards D2 §8.1 cares about.
- **Neither knows what an `RTM_NEWLINK` is.** They give us `socket(AF_NETLINK, SOCK_RAW, NETLINK_ROUTE)` and `setns`; every byte after that is ours or a codec crate's. `[R]` — pick one and use it for the syscalls (D1 §11.1's call, not this document's); it does not affect the netlink decision either way.

### 12.3 nftables and iptables: what the reference implementations actually do

`[F]` **HIGH — and this corrects §5.4.** §5.4 recommends *"link `libnftnl` via the `nftnl` crate rather than hand-rolling the nftables netlink protocol"*, on the grounds that the expression encoding is large, intricate and still evolving. The premise is right. The conclusion is not what either production implementation reached.

**What moby does.** `daemon/libnetwork/internal/nftables` has **two** backends behind build tags, and both take **text**:
- `nft_exec_linux.go` (`//go:build !cgo || static_build || !libnftables`) — `exec.LookPath("nft")`, then `nft -f -` with the ruleset on stdin. This is the **default build**.
- `nft_cgo_linux.go` (`//go:build cgo && !static_build && libnftables`) — `#cgo pkg-config: libnftables`, then `C.nft_run_cmd_from_buffer(h.handle, cCmd)`. Note **`libnftables`**, the high-level library behind the `nft` binary, which parses the same text syntax — *not* `libnftnl`, which is the low-level message builder.
Docker generates that text from Go templates (`reload.nft.gotmpl`, `incremental_update.nft.gotmpl`). It never builds an nftables expression programmatically.

**What netavark does.** `nftables = "0.6.3"` — the `nftables-rs` crate — which serializes a typed Rust model to nftables' **JSON** syntax and shells out to `nft -j -f -`, and parses `nft -j list ruleset` back into the same model.

`[R]` **HIGH** — **So the distinction §5.4 needed and did not draw is: `libnftnl` (message building) vs `libnftables`/`nft` (text or JSON front-end).** Nobody shipping container networking uses the former. **Revise §5.4's `[R] MED`:** do not take `nftnl`. Take `nftables` (nftables-rs), which is §5.4's own "legitimate MVP shortcut" (`nft -f -` is atomic, so the shortcut has no atomicity ceiling) with the string-building replaced by a typed model and a real parser for the read path — and the read path is what §9.7's GC pass needs.

`[F]` **HIGH** — **`nftables` (nftables-rs) 0.6.3** (2025-08-15, `nftables-rs/nftables-rs`, **MIT OR Apache-2.0**, 2.73M lifetime / 551k last-90d). 3 993 LOC, **0 `unsafe`**. Deps: `serde`, `serde_json`, `schemars`, `strum`, `thiserror`, `serde_path_to_error`. `tokio`/`async-process` are optional and mutually exclusive; **the default path is sync `std::process::Command`.** Typed errors (`NftExecution`, `NftOutputEncoding`, `NftInvalidJson`, `NftFailed{stdout, stderr}`) — which answers §2.6's objection to shelling out ("errors as English prose") *partly*: we get structured *failure classification* but the kernel's reason is still a string. That is materially better than `iptables`' exit-code-and-prose and materially worse than an errno.
Costs, honestly: a runtime dependency on the `nft` binary; one `fork`+`exec` per operation; JSON round-tripping; and `nft`'s JSON schema is versioned by the `nft` you happen to have installed. `[R]` — **Adopt.** The atomicity that §5.4 built its whole case on is preserved (`nft -f -` is one transaction), the reconciliation story is preserved (`delete table inet mydocker` — §9.4 rule 3), and we skip a C dependency and 194 `unsafe` blocks.

`[F]` **MED–HIGH** — **`nftnl` 0.9.4** (2026-08-11, `mullvad/nftnl-rs`, MIT/Apache-2.0, 794k lifetime / 59k last-90d). Wraps `libnftnl` through `nftnl-sys` 0.6.2. **125 `unsafe` in 3 393 LOC, plus 69 more in `nftnl-sys`'s 8 797** — the audit surface is "the whole C API". Requires `libnftnl` + `libmnl` present at build and run time, selected by a **25-entry feature matrix** (`nftnl-1-0-7` … `nftnl-1-3-0`) that must match the host's library version. Its own README is unusually candid:
> *"This library currently has quite rough edges… Understanding how to use libnftnl and implementing this crate has mostly been done by reading the source code for the nftables program and attaching debuggers to the `nft` binary. Since the implementation is mostly based on trial and error, there might of course be a number of places where the underlying library is used in an invalid or not intended way. Large portions of libnftnl are also not covered yet."*
It is real production code (Mullvad's VPN client) and actively released. `[R]` — **Read as reference. Do not adopt.** It buys us no-fork-exec and typed errno, at the price of a versioned C dependency, 194 `unsafe` blocks, and a maintainer's own warning that coverage is partial. Revisit only if `fork`+`exec` per container start ever shows up in a profile, which at ~2 execs per container start it will not.

`[F]` **HIGH** — **`rustables` 0.8.8** (2026-06-23, `gitlab.com/rustwall/rustables`, **GPL-3.0-or-later**, 47.8k lifetime / 20.2k last-90d). A pure-Rust fork of `nftnl-rs` that speaks nf_tables netlink directly — 5 098 LOC, 13 `unsafe`, no C dependency. Technically the most appealing option on paper. `[R]` **HIGH — do not adopt, and the reason is not technical: the licence.** The original was MIT/Apache-2.0; the fork is GPL-3.0-or-later. Linking it makes our engine GPL-3.0, which for a container runtime — a category where MIT and Apache-2.0 are universal and where downstream embedding is the normal use — is a decision that should be made deliberately by a human, at the project level, not smuggled in by a dependency choice. Add to that 47.8k lifetime downloads (three orders of magnitude below `nftables`) and roughly annual releases, and it is not a bet to take now. §5.4's *"too thin to bet on"* stands, and the licence is the stronger reason. **Skip.**

`[F]` **HIGH** — **`iptables` 0.6.0** (2025-08-07, `yaa110/rust-iptables`, MIT, 934k lifetime / 379k last-90d). 641 LOC, 0 `unsafe` — because it does nothing but build argv and run the `iptables` binary: *"This crate uses iptables binary to manipulate chains and tables."* Modelled on `coreos/go-iptables`. `[R]` **HIGH — skip, on two independent grounds.** (a) §5.4 already decided to target nftables natively, so an iptables crate is the wrong subsystem. (b) Even for iptables it is §2.6's trap in crate form: exit codes and stderr prose instead of errnos, plus `iptables -I` is a read-modify-write of the whole table, so concurrent invocations can lose rules unless every call passes `-w` (§10.4).

### 12.4 Is shelling out an acceptable MVP shortcut, or a trap?

`[R]` **HIGH** — **It depends entirely on which binary, and the answer is not uniform. Three different verdicts:**

| Binary | Verdict | Why |
|---|---|---|
| `ip` (iproute2) | **Trap.** Never. | §2.6's five reasons, of which the load-bearing one is that idempotent setup (§9.7) requires distinguishing `EEXIST` from `EPERM` from `ENODEV`, and `ip` gives us prose. Plus the pid-not-fd race (§2.3), plus ~12 execs per container start, plus §2.6's `[O]` that even the argument grammar differs across versions. |
| `nft` | **Legitimate, and it is what Docker ships.** | `nft -f -` is one atomic transaction, so the property §5.4 built its case on is preserved rather than sacrificed. `nft -j` gives a parseable read path. 1–2 execs per container start. Use `nftables-rs` so it is a typed model rather than string formatting. Ceiling: `nft` must be installed; errors are classified but not errno'd. |
| `pasta` (rootless) | **Correct, per §8.7.** | A long-lived process with a stable CLI, not a per-operation API. Different category entirely. |

`[R]` **HIGH** — **The rule that generalizes:** shelling out is a trap when the tool is a *per-operation API whose errors we must branch on*, and legitimate when it is a *transaction submitter* or a *long-lived process*. `ip` is the first; `nft -f -` and `pasta` are the second and third. That is the distinction §2.6 and §5.4 each reached separately for their own tool; it is worth writing down once.

### 12.5 Verdict table

| Crate | Version (date) | Verdict | One-line reason |
|---|---|---|---|
| `netlink-packet-route` | 0.33.0 (2026-08-18) | **adopt** | Pure sync codec, 0 `unsafe`, and it already implements `InfoVeth::Peer(LinkMessage)` — §2.2's hardest encoding. |
| `netlink-packet-core` | 0.9.0 (2026-08-18) | **adopt** | Framing + ext-ack parsing for the above; 0 `unsafe`; not optional if you take the route crate. |
| `netlink-sys` | 0.9.0 (2026-08-18), `default-features = false` | **adopt** | Blocking `Socket` with `set_ext_ack` and `set_netlink_get_strict_chk`; 22 `unsafe` we would otherwise write; async sockets are separate types behind separate features. |
| *(the socket/ack/seq/dump loop)* | — | **write ourselves** | ~150 lines, it is the part that teaches netlink, and netavark's `src/network/netlink.rs` is the existence proof of the size. |
| `rtnetlink` | 0.23.0 (2026-08-18) | **skip** | No sync API exists: every constructor returns a `Future` or is bounded on `AsyncSocket`. Collides structurally with D2 §8. netavark skips it too. |
| `netlink-proto` | 0.13.0 (2026-08-18) | **skip** | The reason `rtnetlink` is async. Not separable. |
| `neli` | 0.7.4 (2026-01-28) | **read as reference** | Sync by default and healthy, but thinner rtnetlink coverage (no `VETH_INFO_PEER`) and a bus factor of one on our most load-bearing dependency. |
| `genetlink` | 0.3.0 (2026-08-18) | **skip** | Generic netlink; bridge networking never touches it. Also drags in `netlink-proto`. |
| `nix` / `rustix` | 0.31.3 / 1.1.4 | **adopt (one of)** | `unshare`/`setns`/`socket`/`mount`. Neither speaks netlink; the choice is D1 §11.1's, not ours. |
| `nftables` (nftables-rs) | 0.6.3 (2025-08-15) | **adopt** | Typed model → `nft -j -f -`: keeps §5.4's atomicity, adds a parseable read path for §9.7's GC, MIT/Apache, 0 `unsafe`, sync by default. What netavark uses. |
| `nftnl` (+`nftnl-sys`) | 0.9.4 / 0.6.2 (2026-08-11) | **read as reference** | 194 `unsafe`, a versioned C dependency behind a 25-entry feature matrix, and a README that says coverage is partial. Neither moby nor netavark uses it. **Revises §5.4.** |
| `rustables` | 0.8.8 (2026-06-23) | **skip** | **GPL-3.0-or-later** — a project-level licensing decision, not a dependency choice. Plus 47.8k lifetime downloads. |
| `iptables` | 0.6.0 (2025-08-07) | **skip** | Wrong subsystem per §5.4, and it is §2.6's shell-out trap in crate form. |
| `ipnet` | 2.12.1 (2026-08-02) | **adopt** | Prefix arithmetic for §10 (subnet election, host-route overlap, address iteration). 561M lifetime downloads; not worth writing. netavark uses it. |
| `fs2` *(or `rustix::fs::flock`)* | — | **adopt or stdlib** | `flock(LOCK_EX)` for §10.4's network-creation lock. netavark uses `fs2`; `rustix` and `nix` both expose `flock` and we will already have one of them. **Prefer the one we already have.** |
| conntrack flushing (§9.2) | — | **write ourselves** | It is `NETLINK_NETFILTER`/`NFNL_SUBSYS_CTNETLINK` `IPCTNL_MSG_CT_DELETE` with a filter — a handful of messages on a socket we already have. The `conntrack` crate (0.1.5) is a read-oriented, low-adoption alternative; shelling out to `conntrack -D` is the §12.4 trap. |
| `pasta`/`passt` *(binary, rootless)* | — | **shell out** | §8.7's conclusion, unchanged: a long-lived process with a stable CLI, ~5000 LoC, seccomp-sandboxed, Podman 5's default. |

---

## 13. Handoff

**To D1 (kernel isolation — complete):**
- D1 §12 handed us *"`CLONE_NEWNET` gives you exactly one device, `lo`, and it is **down**"* and *"an empty netns with `lo` up is a legitimate MVP target."* Both confirmed and used: §1.1 turned the first into the highest-value-per-byte line in this document, and §11.1 makes the second **stage 1** of the ladder — a shippable product, not a stub.
- D1's constraint that *"netns plumbing must be done by the parent and must complete before the child drops capabilities and installs seccomp"* is what fixes networking's position in the lifecycle: between `create` and `start`, with the child parked on the exec fifo. §11.2.
- One item flows **back** to D1: §2.6(4) — **a netlink socket is permanently bound to the netns it was created in**, so per-netns work needs a socket opened after `setns`, and `setns` is per-thread. D1 §11.2's *"assert `Threads == 1` before cloning"* and this requirement coexist only if the netns thread is created **after** the clone, or in a different process. Worth a line in D1's ordering rules if it is ever revised.
- D1's rootless finding that *"sysfs needs a new netns"* is why `--network=host` plus rootless is a combination we should test early; it is the one mode where the netns we do not create is still load-bearing.

**To D2 (runtime architecture):**
- **The netlink-crate async question is settled and it does not force a change to §8.** `rtnetlink` has no sync API and would put an executor in the supervisor; **we do not use it.** `netlink-packet-route` + `netlink-packet-core` + `netlink-sys` (default features off) are sync with zero async dependencies, and netavark proves the combination in production. D2 §8's rule — *"the process that calls `clone()` never has more than one thread, ever, and never links a runtime that could create one"* — survives intact. See §12.1.
- **§9.7 puts networking inside D2 §9.3's rollback machinery, not beside it.** The `Cleanup` enum needs the variants `UnmountNetns(path)`, `DeleteNftElements(net, ip)`, `ReleaseIp(net, addr)`, `KillHelper(pid)`. D2 §9.4's *"`delete` and `create`'s rollback should be the same code"* applies unchanged.
- **One correction to D2 §9.1's leak table.** It rates *"a leaked IP from the pool"* as *"a slow exhaustion."* It is also a **misroute**: a stale DNAT rule plus a reissued address sends traffic for container A's published port into container B (§9.2). Release order is a security property, not a housekeeping one, and §9.8's rule 4 — **release the IP last** — belongs in D2's ordering rules.
- **D2 §9.3(3)'s persisted rollback list, rated MED, has a production precedent in our exact architecture.** netavark's `$config/firewall/{networks,ports}/` replay directory plus `firewall-reload.lock` is that idea, shipped, by a daemonless engine (§9.6). Raise the confidence.
- **§10.4's concurrency answer is consistent with D2 §6.6 and adds one lock.** Address allocation needs **no** lock (`O_CREAT|O_EXCL` on a lease file *is* the compare-and-swap, same trick as `mkdir` for ids and `symlink` for names). Network *creation* needs one `flock(LOCK_EX)`, held for milliseconds, off the hot path. D2 §6.6's *"no global lock, ever"* stands.
- **A new requirement D2 does not have: a reconcile verb.** §9.7 wants `mydocker net gc`, CNI-`GC`-shaped, run opportunistically on a cheap staleness check plus once at boot. Being daemonless removes our only natural reconciliation point, and §9.3 shows a *daemon* drifting for 186 days anyway.
- **Process topology, rootless:** §8.2's `[R]` stands — a network helper (`pasta`) must be launched from a process already inside the container's userns, not from a sibling. That is a constraint on D2's process tree.
- §4.4's recommendation to **bind the published-port socket in our process and pass the fd** (Docker 28's `-use-listen-fd`) makes `EADDRINUSE` surface synchronously in `mydocker run` and makes the orphan-helper leak structurally impossible. That is an fd-passing requirement on D2 §5.2's `SCM_RIGHTS` machinery, which already exists for the console socket.

**To D3 (images / layers / storage — in progress):**
- **`/etc/resolv.conf` and `/etc/hosts` are generated per container on the host and bind-mounted over the image's copies** (§6.2, verified on `host-a`). They are not layers, they are not in the image, and edits inside the container do not survive a restart. **That is a mount-lifecycle item, and it lands in your rollback list, not ours** — two bind mounts per container that leak exactly like any other host-namespace mount.
- Please put them **inside the container's mount namespace** if the design allows. D2 §9.2's best property — *"everything the child mounts after `unshare(CLONE_NEWNS)` is free to roll back"* — applies to these two files as much as to the rootfs, and it deletes two entries from the leak list.
- Per-container generated files need a home: Docker uses `/var/lib/docker/containers/<id>/{resolv.conf,hosts}`. Whatever you choose for the container state dir, these two go there.
- No other networking↔storage coupling was found. The netns bind mount lives in `/run`, not `/var/lib`, deliberately (§1.2) — it must not survive a reboot.

**To D5 (cgroups / capabilities / seccomp / rootless — in progress):**
- **§8.1 is the wall, verified with kernel source and an empirical `EPERM`:** the gate is `CAP_NET_ADMIN` **in the user namespace owning the target netns**. Everything works inside `unshare -Urn` — bridges, veths, addresses, enslaving, nft `nat` chains, network sysctls — and moving one veth end to the host netns is `EPERM`, unconditionally. Not a distro policy; the ownership model.
- **§8.5 corrects a claim you will otherwise inherit: "rootless containers can't bind port 80" is wrong.** `ip_unprivileged_port_start` is per-netns *and* the `CAP_NET_BIND_SERVICE` check is evaluated against the socket's netns and its owning userns. nginx binds `:80` inside a rootless container with no change at all. Only the **host-side** port < 1024 fails.
- **§8.6: do not build a setuid helper.** `lxc-user-nic` is the existence proof that it works and the reason not to.
- **Capabilities the network path needs, and when:** `CAP_NET_ADMIN` in the host netns for bridge/veth/enslave/rules; `CAP_NET_BIND_SERVICE` only for host ports < 1024; **`CAP_NET_RAW` is not needed by us** — it is needed *inside* the container for `ping`, and §8.2 shows the cheaper fix is widening `net.ipv4.ping_group_range` **inside the container netns**, which is free and per-namespace. `[R]` — set it; a container where `ping` fails is a container users think is broken.
- **A rootless state-path constraint that is really yours:** §8.1's `[O]` that uid-0-in-a-userns does **not** grant DAC override over files owned by uids outside the mapping — `mkdir /var/run/netns` fails as root-in-userns. Rootless state must live under `$XDG_RUNTIME_DIR/mydocker/`, never `/run/mydocker/`. That lands on our netns bind-mount path directly.
- **§9.8 rule 2 is a security item for you:** writing `net.ipv4.ip_forward` *resets every other IPv4 config parameter to its default*, per `ip-sysctl`. A blind write silently discards an admin's `rp_filter`/`accept_local`/`arp_ignore` tuning host-wide. Read-then-write-only-if-zero, once per network, first among sysctls.
- **§4.6 is the biggest security-surprise in this document and it may belong in your threat model:** `ufw deny 80` does not stop `-p 80:80`, for two independent reasons (wrong chain, and Docker inserts ahead of ufw). Our answer is a `MYDOCKER-USER` chain we never write into **plus a runtime warning** when we detect a foreign firewall manager — Docker's failure here is a documentation-only fix for a problem that wants a warning.

---

## 14. Open questions

Ordered roughly by how much a wrong guess would cost. **`old-host` was unreachable while this section was written** (`old-host.example.edu` → NXDOMAIN, 2026-09-05), so every experiment below is written for the future dev VM unless it says otherwise. Everything on `host-a` is read-only, permanently.

1. **Does a current-thread tokio runtime really create no threads?** §12.1 rates MED and it is the one thing that could reopen the `rtnetlink` question. **Settles it:** on the dev VM, build a binary that does `Builder::new_current_thread().enable_all().build()`, runs one `rtnetlink` link-add, and then reads `Threads:` from `/proc/self/status` before and after — D1 §11.2 already wants that assertion, so this is a test we owe ourselves regardless. Also check `ls /proc/self/task | wc -l` after `spawn_blocking` is *not* called. If it is genuinely 1, route 3 in §12.1 becomes defensible; the recommendation still stands on D2 §8.3's structural-beats-disciplined argument, but the tone would soften.

2. **What actually produced the orphaned `br-ac0ab0eb6175`?** (§9.3.) The single most valuable thing to reproduce, because it is the failure mode §9 exists to prevent and because Docker's behaviour is our baseline. **Settles it:** on the dev VM — `docker network create t && docker run -d --network t --name a nginx`, snapshot `ip -d link`/`iptables-save`/`local-kv.db`, then `kill -9 $(pidof dockerd)` at each of: during `docker network rm t`, during `docker rm -f a`, during `docker compose down`. Diff after each. Then restart dockerd and diff again to see what reconciliation reclaims and what it does not.

3. **Is `nft -f -` genuinely atomic under a mid-transaction `SIGKILL`, and what does a syntax error leave behind?** §5.4 and §12.3 both rest on this and neither verified it. **Settles it:** on the dev VM (or any unprivileged netns, this needs no root), feed `nft -f -` a two-statement ruleset whose second statement is garbage and check `nft list tables` — the first statement must not have landed. Then `kill -9` the `nft` process mid-apply with a large generated ruleset and check again. I drafted this exact test for `old-host` and could not run it.

4. **Idempotency errnos, measured rather than assumed.** §9.7's table asserts `EEXIST` for a duplicate link, `EEXIST` for a duplicate address, `ENODEV` for deleting a missing link, `EADDRNOTAVAIL` for deleting a missing address, and that enslaving twice is a no-op. Those are the branch conditions of every setup path we write, and I inferred them. **Settles it:** inside `unshare -Urn` on any host — create the same veth pair twice, add the same address twice, delete a nonexistent link and address, enslave twice, and `nft add table` twice; record every errno. This needs no privilege and takes two minutes; it was blocked only by `old-host` being down.

5. **Which conntrack entries must be flushed, exactly, and via which interface?** §9.2 establishes *that* stale conntrack entries defeat new NAT rules, from moby's own comment and issues #8795/#44688/#44742. It does not establish our implementation. **Settles it:** read `daemon/libnetwork/iptables/conntrack.go` in moby for `DeleteConntrackEntries`/`DeleteConntrackEntriesByPort` — it goes over netlink, and the message shape is what we need. Then on the dev VM: publish a UDP port, send a packet before the rules exist, add the rules, and confirm the flow is misrouted until the flush.

6. **Does our `O_CREAT|O_EXCL` lease scheme hold on every filesystem we care about?** §10.3 depends on `O_EXCL` being atomic. It is on local filesystems; it is famously **not** on NFS before NFSv3-with-proper-locking. **Settles it:** decide whether `/var/lib/mydocker` on NFS is in scope (it should not be), and document it. If it must be, the fallback is `link(2)`-based locking. Cheap to answer, expensive to discover late.

7. **How much does one `fork`+`exec` of `nft` per container start actually cost?** §12.3 asserts it will never show up in a profile. Unmeasured. **Settles it:** on the dev VM, time 100 sequential `mydocker run --rm alpine true` with and without the nft step, and compare against the ~15 ms floor of clone+mount+pivot_root. If `nft` is more than ~10% of container start, `nftnl` comes back into play.

8. **The `bridgeAlreadyExists` reconcile: what does Docker actually change?** §9.4 rule 6 names `setupVerifyAndReconcileIPv4` but I did not read it. It decides our own adopt-vs-fail policy for a pre-existing bridge, which is §9.2's third-most-common leak. **Settles it:** read `daemon/libnetwork/drivers/bridge/setup_verify_linux.go` in moby.

9. **Is `netlink-packet-route`'s `InfoVeth::Peer(LinkMessage)` byte-identical to iproute2's encoding?** §12.1 recommends adopting it precisely because it implements §2.2's hard message. Unverified against the wire. **Settles it:** `strace -e trace=sendmsg -x ip link add v0 type veth peer name v1 netns /proc/N/ns/net` and diff the hex against what the crate emits for the same request. §2.6 already recommends this as the general development oracle; this is its first use.

10. **The `rustables` licence question is a project decision, not a research one.** §12.3 marks GPL-3.0-or-later as disqualifying on the assumption that the engine will be MIT or Apache-2.0. **Settles it:** pick the project licence. Until then the verdict is "skip", which costs nothing.

11. **Unresolved from §5.3, still unresolved:** netavark's exact iptables-driver removal timeline. The claim came from a GitHub discussion, not a release note. **Settles it:** netavark's release notes or the `firewall-driver` option documentation. Low stakes — the direction of travel is not in doubt.

12. **Unresolved from §6.1, still unresolved:** whether "avoid colliding with a container that binds `0.0.0.0:53`" is the *stated* rationale for Docker's DNAT-to-random-port resolver dance. **Settles it:** the commit message or design note for libnetwork's resolver. §6.5 stage 3 recommends skipping the dance entirely, so this only matters if that recommendation is revisited.

13. **`ip_forward = 1` in a fresh netns on `old-host` (§1.1), mechanism unconfirmed.** Still open, and now unreproducible until that host or an equivalent 4.18 box is available. **Settles it:** read `devinet_init_net()` in `net/ipv4/devinet.c` for that kernel, and repeat the observation on a host whose own `ip_forward` is 0.

---

## 15. Sources

Consolidates every URL cited in §0–§14. **✗** marks a body citation for which I give the canonical location but did **not** myself open the document; **✓** marks one I read or fetched directly while writing §9–§14. Unmarked entries were cited in §0–§8 by the earlier pass with a working URL.

### Primary — specifications

- OCI Runtime Spec, namespaces: <https://github.com/opencontainers/runtime-spec/blob/main/config-linux.md#namespaces>
- OCI Runtime Spec, POSIX platform hooks: <https://github.com/opencontainers/runtime-spec/blob/main/config.md#posix-platform-hooks>
- ✓ OCI Runtime Spec, **lifecycle and hook-failure semantics** — `runtime.md` steps 3–13, including *"If any `createRuntime` hook fails, the runtime MUST generate an error, stop the container"* and *"If any `poststop` hook fails, the runtime MUST log a warning, but the remaining hooks and lifecycle continue as if the hook had succeeded"*: <https://raw.githubusercontent.com/opencontainers/runtime-spec/main/runtime.md> (§11.2)
- ✓ CNI Specification — `DEL` idempotency (*"Plugins MUST accept multiple `DEL` calls…"*), best-effort teardown, the runtime's obligation to eventually call delete, and the `GC`/`STATUS` verbs: <https://github.com/containernetworking/cni/blob/main/SPEC.md> (§0, §9.5)
- RFC 1918, private address space: <https://www.rfc-editor.org/rfc/rfc1918>

### Primary — man pages

- `namespaces(7)`: <https://man7.org/linux/man-pages/man7/namespaces.7.html>
- `network_namespaces(7)` — physical devices migrate back, veths are destroyed: <https://man7.org/linux/man-pages/man7/network_namespaces.7.html>
- `veth(4)` — deleting either end deletes both: <https://man7.org/linux/man-pages/man4/veth.4.html>
- `netlink(7)`: <https://man7.org/linux/man-pages/man7/netlink.7.html>
- `rtnetlink(7)`: <https://man7.org/linux/man-pages/man7/rtnetlink.7.html>
- `unshare(2)`: <https://man7.org/linux/man-pages/man2/unshare.2.html>
- `setns(2)`: <https://man7.org/linux/man-pages/man2/setns.2.html>
- `ip-netns(8)` — *"By convention a named network namespace is an object at /var/run/netns/NAME"*: <https://man7.org/linux/man-pages/man8/ip-netns.8.html>
- ✗ `lxc-user-nic(1)` — the setuid helper, `/etc/lxc/lxc-usernet`, `/run/lxc/nics` (§8.6): <https://linuxcontainers.org/lxc/manpages/man1/lxc-user-nic.1.html>
- ✗ `slirp4netns(1)` — defaults, `--api-socket`, the `tcp_rmem` BUGS entry (§8.2): <https://raw.githubusercontent.com/rootless-containers/slirp4netns/master/slirp4netns.1.md>
- ✗ `passt(1)` / `pasta(1)` — the no-NAT design, the `splice()` bypass, the 65520 MTU, the seccomp syscall counts (§8.3): <https://passt.top/passt/about/>

### Primary — kernel source and documentation

- `Documentation/networking/ip-sysctl.txt` — `ip_forward` *"resets all configuration parameters to their default state"*, `route_localnet`, `ping_group_range`: <https://www.kernel.org/doc/Documentation/networking/ip-sysctl.txt>
- ✓ `Documentation/networking/ip-sysctl.rst` (current rendering) — `ip_unprivileged_port_start` as a per-namespace sysctl (§8.5): <https://www.kernel.org/doc/html/latest/networking/ip-sysctl.html>
- `net/core/rtnetlink.c` — the `CAP_NET_ADMIN`-in-the-owning-userns gate: <https://git.kernel.org/pub/scm/linux/kernel/git/torvalds/linux.git/plain/net/core/rtnetlink.c> and pinned <https://raw.githubusercontent.com/torvalds/linux/v6.11/net/core/rtnetlink.c>
- `net/core/net_namespace.c`: <https://raw.githubusercontent.com/torvalds/linux/v6.11/net/core/net_namespace.c>
- `net/ipv4/devinet.c` — loopback address insertion on `NETDEV_UP`: <https://git.kernel.org/pub/scm/linux/kernel/git/torvalds/linux.git/plain/net/ipv4/devinet.c>
- `drivers/net/veth.c` — no capability check in `veth_newlink()`: <https://git.kernel.org/pub/scm/linux/kernel/git/torvalds/linux.git/plain/drivers/net/veth.c>
- `include/uapi/linux/netlink.h` — `NLM_F_*`, `NLMSGERR_ATTR_*`, `NETLINK_EXT_ACK`: <https://git.kernel.org/pub/scm/linux/kernel/git/torvalds/linux.git/plain/include/uapi/linux/netlink.h>
- Linux bridge documentation: <https://www.kernel.org/doc/html/latest/networking/bridge.html>

### Primary — iproute2 source

- `lib/libnetlink.c` — `addattr_l`, the unaligned-`rta_len`/aligned-cursor rule: <https://git.kernel.org/pub/scm/network/iproute2/iproute2.git/plain/lib/libnetlink.c>
- `ip/link_veth.c` — the `VETH_INFO_PEER` encoding: <https://git.kernel.org/pub/scm/network/iproute2/iproute2.git/plain/ip/link_veth.c>
- `ip/ipaddress.c`, `ip/iproute.c`: <https://git.kernel.org/pub/scm/network/iproute2/iproute2.git/plain/ip/ipaddress.c>, <https://git.kernel.org/pub/scm/network/iproute2/iproute2.git/plain/ip/iproute.c>

### Primary — moby / Docker source (read directly for §9 and §10, `master` as of 2026-09-05)

- ✓ `daemon/libnetwork/ipamutils/utils.go` — the built-in local and global address pools (§10.1)
- ✓ `daemon/libnetwork/ipams/defaultipam/allocator.go` — `RequestPool`, `Capability{RequiresRequestReplay: true}` (§10.2)
- ✓ `daemon/libnetwork/internal/addrset/addrset.go` — the in-memory bitmap allocator (§10.2)
- ✓ `daemon/libnetwork/controller.go` — `reservePools()`, `doReplayPoolReserve()`: pool + endpoint-address replay at daemon start (§9.4, §10.2)
- ✓ `daemon/libnetwork/netutils/utils_linux.go` — `InferReservedNetworks`, `queryOnLinkRoutes`, `GenerateIfaceName`, and the comment explaining why only on-link routes are reserved (§9.4, §10.1)
- ✓ `daemon/libnetwork/drivers/bridge/bridge_linux.go` — firewall-backend selection and cross-backend cleanup, `setupVerifyAndReconcileIPv4`, `clearConntrackEntries` / `clearConntrackEntriesForPorts` and their comments citing moby #8795/#44688/#44742 (§9.2, §9.4)
- ✓ `daemon/libnetwork/drivers/bridge/internal/iptabler/iptabler.go` and `.../network.go` — `NewIptabler` → `removeIPChains` then `setupIPChains`; the chain list, which **excludes `DOCKER-USER`**; `deleteLegacyTopLevelRules` (§9.4)
- ✓ `daemon/libnetwork/drivers/bridge/internal/nftabler/nftabler.go` and `.../cleaner.go` — `table docker-bridges`, verdict maps, and `delete table` as the whole cleanup (§9.4)
- ✓ `daemon/libnetwork/internal/nftables/nft_exec_linux.go` and `nft_cgo_linux.go` — the two backends, `nft -f -` by default and `libnftables` via cgo (§12.3)
- Base: <https://raw.githubusercontent.com/moby/moby/master/> + path
- ✗ `daemon/libnetwork/ipam.md` (historic libnetwork docs) — the `RequiresRequestReplay` definition quoted in §10.2 was read at <https://github.com/moby/libnetwork/blob/master/docs/ipam.md>
- ✓ moby PR #42598 — narrowing subnet-overlap detection to on-link routes, released in v23.0: <https://github.com/moby/moby/pull/42598>

### Primary — netavark (Podman's Rust network stack, read directly for §9 and §12)

- ✓ `Cargo.toml` — `netlink-packet-route`/`netlink-packet-core`/`netlink-sys`/`nftables`/`nix`/`fs2`/`ipnet`, and **no `rtnetlink`** (§12.1, §12.5)
- ✓ `src/network/netlink.rs` — the ~150-line synchronous `Socket<P>` wrapper, `set_netlink_get_strict_chk(true)` (§12.1)
- ✓ `src/firewall/state.rs` — the `$config/firewall/{firewall-driver,networks/$netID,ports/$netID_$conID}` replay log and `firewall-reload.lock` with the comment naming the race (§9.6)
- ✓ `src/commands/teardown.rs` — `SIGTERM`/`SIGINT` ignored during teardown, `NetavarkErrorList`, *"bridge interfaces… will not be removed"* (§9.6)
- Base: <https://raw.githubusercontent.com/containers/netavark/main/> + path

### Secondary — vendor documentation and release notes

- Docker: port publishing, `gateway_mode`, the v6-host→v4-container proxy behaviour, the pre-28 localhost/L2 leak: <https://docs.docker.com/engine/network/port-publishing/>
- Docker: packet filtering and firewalls, `DOCKER-USER`, *"packets are diverted before it reaches the INPUT and OUTPUT chains that ufw uses"*: <https://docs.docker.com/engine/network/packet-filtering-firewalls/>
- Docker: networking overview, the `127.0.0.11` resolver and its lack of an IPv6 equivalent: <https://docs.docker.com/engine/network/>
- Docker Engine 28 release notes — `FirewallBackend` in `/info`, the filter-FORWARD restructuring: <https://docs.docker.com/engine/release-notes/28/>
- Docker Engine 28 hardening blog — the `raw/PREROUTING` anti-spoof rules, moby #14041/#22054/#48815: <https://www.docker.com/blog/docker-engine-28-hardening-container-networking-by-default/>
- Fedora change: netavark nftables by default: <https://fedoraproject.org/wiki/Changes/NetavarkNftablesDefault>
- netavark discussion #1332 (the source of the `[LOW]` iptables-removal-timeline claim in §5.3, still unresolved — §14 item 11): <https://github.com/containers/netavark/discussions/1332>
- netavark firewalld integration: <https://github.com/containers/netavark/blob/main/docs/netavark-firewalld.7.md>
- ✗ Podman v5.0.0 release notes — pasta as the default rootless network tool, listed under Breaking Changes (§8.3): <https://github.com/containers/podman/releases/tag/v5.0.0>
- ✗ RootlessKit `docs/network.md` — the iperf3 driver benchmark table quoted in §8.7: <https://raw.githubusercontent.com/rootless-containers/rootlesskit/master/docs/network.md>
- ✗ RootlessKit `docs/port.md` — the port-driver benchmark table and the driver menu quoted in §8.4 and §8.7: <https://raw.githubusercontent.com/rootless-containers/rootlesskit/master/docs/port.md>
- ✗ RootlessKit `pkg/port/builtin/` — the accept-and-relay protocol and `bicopy()` described in §8.4: <https://github.com/rootless-containers/rootlesskit/tree/master/pkg/port/builtin>
- ✗ slirp4netns `main.c` and libslirp `src/ip_icmp.c` — the fork/`setns`/`TUNSETIFF`/`SCM_RIGHTS` sequence and the ping-socket fallback described in §8.2: <https://github.com/rootless-containers/slirp4netns>, <https://gitlab.freedesktop.org/slirp/libslirp>

### Crate metadata (crates.io API and published `.crate` tarballs, read 2026-09-05)

All version numbers, release dates, download counts, dependency lists, licences and `unsafe`/LOC counts in §12 come from:
- ✓ `https://crates.io/api/v1/crates/<name>` for version, dates and downloads
- ✓ `https://static.crates.io/crates/<name>/<name>-<version>.crate` for the source actually published (this is what the `unsafe` and LOC figures were counted over)
- ✓ Repository `Cargo.toml` and `src/` for feature layout: `rust-netlink/{rtnetlink,netlink-sys,netlink-proto,netlink-packet-route,netlink-packet-core,genetlink}`, `jbaublitz/neli`, `mullvad/nftnl-rs`, `gitlab.com/rustwall/rustables`, `nftables-rs/nftables-rs`, `yaa110/rust-iptables`

### Empirical hosts

- **`host-a`** = `HOST-A`, Ubuntu 24.10, kernel `6.11.0-29-generic`, Docker `28.3.1`, `iptables v1.8.10 (nf_tables)`, `nftables v1.1.0`, `dockerd` pid 70234 up since 2026-03-02, host up since 2026-02-28. **Live production. Every command run against it for §9–§12 was read-only**: `ip route`, `ip -d link show`, `ip -o addr show`, `bridge link show`, `docker network ls/inspect`, `iptables -t {nat,raw,filter} -S`, `nft list {tables,table,chain}`, `ps`, `uptime -s`, `ls`, and `grep -a` against `/var/lib/docker/network/files/local-kv.db`. No mutation of any kind; no `docker run`, no rule change, no link change, no sysctl write, no service touched.
- **`old-host`** = `OLD-HOST`, Rocky Linux 8.8, kernel `4.18.0-425.3.1.el8`, unprivileged. Source of the `[O]` claims in §1, §2, §5 and §8. **Unreachable on 2026-09-05** (`old-host.example.edu` → NXDOMAIN), which is why §14 items 3, 4 and 13 are open rather than answered.

