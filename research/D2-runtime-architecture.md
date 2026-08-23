# D2 — Runtime architecture, reference implementations & the Rust substrate

Research agent D2. R&D only, no code written.

**Claim tags:** `[F]` Fact (Linux / OCI spec requirement, primary source cited) · `[C]` Convention (what a project happens to do, source cited) · `[R]` Recommendation (ours, tradeoff stated) · `[O]` Observation (empirically verified, host + kernel + command named). Confidence `HIGH` / `MED` / `LOW`.

**Hosts used for `[O]` claims**

| Alias | Identity | Kernel | Notes |
|---|---|---|---|
| `host-a` | `HOST-A`, Ubuntu 24.10 | `6.11.0-29-generic` | root available, cgroup v2 unified, live Docker 28.3.1 / containerd 1.7.27 / runc 1.2.5, **production, read-only discipline observed** |
| `old-host` | `OLD-HOST`, Rocky Linux 8.8 | `4.18.0-425.3.1.el8` | no root, cgroup v1 no delegation, podman 4.9.4 / runc 1.1.12 / conmon 2.1.10 / apptainer 1.4.0 |

`[O]` HIGH — versions on `host-a` via `uname -a; docker version; runc --version; containerd --version`: kernel 6.11.0-29-generic, Docker Engine 28.3.1 (Go 1.24.4), containerd 1.7.27, runc 1.2.5 (spec 1.2.0, Go 1.23.7, libseccomp 2.5.5), docker-init 0.19.0.
`[O]` HIGH — versions on `old-host` via `uname -a; podman --version; runc --version; conmon --version; apptainer --version`: kernel 4.18.0-425.3.1.el8, podman 4.9.4-rhel, runc 1.1.12 (spec 1.2.0+dev), conmon 2.1.10, apptainer 1.4.0-1.el8 (`singularity` is the same binary/version).

---

# 1. OCI Runtime Specification — mandate vs. latitude

## 1.1 The lifecycle state machine

`[F]` HIGH — The spec defines exactly four status values: `creating`, `created`, `running`, `stopped`. (runtime-spec `runtime.md`, State section.)

- `creating` — the container is being created (`create` in flight).
- `created` — "the runtime has finished the create operation … and the container process has neither exited nor executed the user-specified program".
- `running` — the user-specified program is executing (or has executed and the runtime cannot tell it exited).
- `stopped` — the container process has exited.

`[F]` HIGH — The spec's numbered lifecycle is 13 steps (runtime-spec `runtime.md` "Lifecycle"):

```
 1. create <id> <bundle>            invoked
 2. container runtime environment MUST be created per config.json
 3. prestart hooks          (DEPRECATED)   fail -> stop, goto 12
 4. createRuntime hooks                    fail -> stop, goto 12
 5. createContainer hooks                  fail -> stop, goto 12
 6. start <id>                     invoked
 7. startContainer hooks                   fail -> stop, goto 12
 8. runtime MUST run process.args
 9. poststart hooks                        fail -> log warning, continue
10. container process exits
11. delete <id>                    invoked
12. container MUST be destroyed by undoing the steps performed during create
13. poststop hooks                         failures logged as warnings, lifecycle continues
```

`[F]` HIGH — The state machine transitions are therefore: `creating → created` on successful `create`; `created → running` on `start`; `running → stopped` on process exit; `stopped → (gone)` on `delete`. There is **no** `paused` status in the spec — `runc pause` is a runc extension, and runc models it as a fifth internal state.

`[C]` HIGH — runc's internal state set is larger than the spec's: `stoppedState`, `createdState`, `runningState`, `pausedState`, `loadedState` (`libcontainer/state_linux.go`, `container_linux.go`). `runc state` maps `paused` out to the JSON `status` field even though the spec does not define it. So the spec's four states are a *reporting* contract, not an implementation constraint.

## 1.2 The operations

`[F]` HIGH — Only five operations are specified, and **the spec explicitly disclaims mandating a CLI**: "Note: these operations are not specifying any command-line APIs, and the parameters are inputs for general operations." (runtime-spec `runtime.md`, Operations.)

| Op | MUST | Error condition |
|---|---|---|
| **state** | "MUST return the state of a container as specified in the State section" | container does not exist |
| **create** | "MUST create a new container … MUST apply all config.json properties except `process`" | id not unique; properties cannot be applied |
| **start** | "MUST run the user-specified program as specified by `process`" | container not in `created` state; `process` not set |
| **kill** | "MUST send the specified signal to the container process" | container neither `created` nor `running` — "MUST have no effect on the container and MUST generate an error" |
| **delete** | "Deleting a container MUST delete the resources that were created during the `create` step" | container not `stopped` |

`[F]` HIGH — `create` must apply everything *except* running the process. This is the entire reason the create/start split exists in the spec at all — see §4.

## 1.3 `config.json` — required vs optional

`[F]` HIGH (runtime-spec `config.md`):

**Top level**
- `ociVersion` — **REQUIRED**, SemVer v2.0.0 format.
- `root` — **REQUIRED** on Linux. Subfields: `path` **REQUIRED**, `readonly` OPTIONAL (default false).
- `process` — OPTIONAL at the top level, but **"REQUIRED when `start` is called"**. This is the spec's way of allowing a `create`-only bundle.
- `mounts` — OPTIONAL array. Entry: `destination` **REQUIRED**; `source`, `options`, `type`, `uidMappings`, `gidMappings` OPTIONAL.
- `hostname`, `domainname`, `hooks`, `annotations` — all OPTIONAL.
- `linux` — OPTIONAL ("MAY be set"). Everything namespace/cgroup/seccomp related lives here, and **the spec does not require any of it**.
- `windows` — REQUIRED only if the target platform is windows. `freebsd`/`solaris`/`vm`/`zos` OPTIONAL.

**`process` subfields**
- `cwd` — **REQUIRED** (string).
- `args` — marked OPTIONAL, but "at least one entry is REQUIRED" on non-Windows. (On Windows `commandLine` substitutes.)
- `user.uid`, `user.gid` — **REQUIRED** on POSIX. `umask`, `additionalGids` OPTIONAL.
- `terminal`, `consoleSize`, `env`, `capabilities`, `rlimits`, `apparmorProfile`, `oomScoreAdj`, `selinuxLabel`, `noNewPrivileges`, `scheduler`, `ioPriority`, `execCPUAffinity` — all OPTIONAL.

**`hooks[*]` entry**
- `path` — **REQUIRED**, "MUST be absolute". `args`, `env` OPTIONAL, `timeout` OPTIONAL and "MUST be greater than zero".

`[R]` HIGH — **The minimal legal `config.json` for us to accept is: `ociVersion`, `root.path`, `process.cwd`, `process.args[0]`, `process.user.{uid,gid}`.** Everything else can be defaulted. Build the parser so those five are hard errors and everything else has a default. Tradeoff: being permissive means we silently run containers whose author expected e.g. `linux.namespaces` to be honoured — so log at WARN every field we parse but do not implement, and refuse (not ignore) anything in the *security* dimension (seccomp, caps, readonlyPaths). Ignoring a security field silently is the one unacceptable failure.

## 1.4 The hooks — what each is actually for

`[F]` HIGH (runtime-spec `config.md` POSIX-platform Hooks, and `runtime.md` Lifecycle):

| Hook | Runs in | Runs when | Actual purpose |
|---|---|---|---|
| `prestart` | **runtime** namespace | after env created, before `createRuntime` | **DEPRECATED.** Legacy CNI hook point. Historically the only hook, so it accumulated everything. |
| `createRuntime` | **runtime** namespace | after `prestart`, still during `create` | The real "set up host-side resources for this container now that its namespaces exist" hook. Network plumbing (moving a veth into the netns via the reported `pid`), device setup. The container's mount namespace exists but the rootfs pivot has **not** happened. |
| `createContainer` | **container** namespace (mount ns), before pivot_root | after `createRuntime` | Manipulate the container's *mount namespace* — e.g. add a mount the runtime doesn't know about. Runs inside the container's mount ns but the rootfs is not yet the root. |
| `startContainer` | **container** namespace | after `start`, before `execve` of user process | Last chance to touch the container from inside, with the rootfs pivoted. Used for `ldconfig`, injecting binaries. |
| `poststart` | **runtime** namespace | after user process started | Notify external systems that the container is live. Failure → **log a warning**, the container keeps running. |
| `poststop` | **runtime** namespace | during `delete`, after container destroyed | Teardown of whatever `createRuntime` set up. Failures logged as warnings; "the lifecycle continues". |

`[F]` HIGH — Hook stdin receives the container **state** JSON. "For hooks executed in the runtime namespace, it is the pid as seen by the runtime. For hooks executed in the container namespace, it is the pid as seen by the container." (runtime-spec `runtime.md`.)

`[R]` MED — **Do not implement hooks in phase 1.** They are OPTIONAL in the spec, and the two that are structurally hard (`createContainer`, `startContainer`) require executing a host binary *inside* a partially-constructed container namespace — which means a whole extra fork/setns path and a whole extra failure mode. Implement `createRuntime` + `poststop` first when you do (those two are the pair that networking needs), and design the create path so there is an explicit, named point where a hook *would* be called. Tradeoff: no CNI compatibility until then, which is fine because we own our own networking (D4).

## 1.5 What the spec explicitly does NOT mandate — read this list twice

`[F]` HIGH, all verbatim from runtime-spec `runtime.md`:

1. **Error reporting.** "In cases where the specified operation generates an error, this specification does not mandate how, or even if, that error is returned or exposed to the user."
2. **Warning reporting.** Same wording for warnings.
3. **Visibility across runtime instances.** "Whether other entities using the same, or other, instance of the runtime can see that container is out of scope."
4. **Validation.** "The runtime MAY validate `config.json` against this spec, either generically or with respect to local system."
5. **Which `process` properties are applied at `create` time.** "The remaining `process` properties MAY be applied by this operation."
6. **Extra state fields.** "The state MAY include additional properties."
7. **ID reuse.** "This MUST be unique across all containers on this host. There is no requirement that it be unique across hosts" … "Once a container is deleted its ID MAY be used by a subsequent container."
8. **The CLI.** "these operations are not specifying any command-line APIs".

`[F]` HIGH — Consequences the spec therefore leaves entirely to us: **where and how state is persisted**, **whether there is a daemon**, **whether there is a supervisor process**, **how stdio is wired**, **how exit codes are reported**, **locking/concurrency between concurrent invocations**, **the on-disk layout**, **the bundle's provenance**. None of that is spec.

`[R]` HIGH — This is liberating and should shape the whole project: **OCI compatibility is a serialization + lifecycle-semantics contract, not an architecture.** We can be spec-compliant with a design that looks nothing like runc. The only externally-visible obligations are: parse `config.json`, honour the four states, honour the five operations' MUST/error semantics, and (if we want tool interop) implement the *de facto* runc CLI shape — which is convention, not spec.

`[C]` HIGH — The *de facto* CLI (what containerd's shim, CRI-O, and Podman actually invoke) is: `runc --root DIR create --bundle DIR --pid-file F --console-socket S ID`, `runc --root DIR start ID`, `runc --root DIR state ID`, `runc --root DIR kill ID SIG`, `runc --root DIR delete [--force] ID`, plus `exec`, `list`, `ps`, `events`, `update`, `pause`/`resume`. Verified indirectly: `[O]` HIGH on `host-a`, `sudo runc --root /run/docker/runtime-runc/moby list` and `... state <id>` both work against containers created by containerd's shim, proving the `--root` + subcommand shape is the real interop surface.

---

# 2. The real Docker chain, boundary by boundary

## 2.1 Empirical ground truth (`host-a`, kernel 6.11.0-29)

`[O]` HIGH — `ps -eo pid,ppid,pgid,sid,comm,args` on `host-a`:

```
    912       1     912     912 containerd       /usr/bin/containerd
  70234       1   70234   70234 dockerd          /usr/bin/dockerd -H unix:///var/run/docker.sock --containerd=/run/containerd/containerd.sock
2585480       1 2585480     912 containerd-shim  /usr/bin/containerd-shim-runc-v2 -namespace moby -id 88ecb99... -address /run/containerd/containerd.sock
2588176   70234   70234   70234 docker-proxy     /usr/bin/docker-proxy -proto tcp -host-ip 0.0.0.0 -host-port 80 ...
```

Three things fall out of that table immediately:

`[O]` HIGH — **The shim's PPID is 1, not containerd's 912.** Verified for all five live shims. `grep PPid /proc/2585480/status` → `PPid: 1`.
`[O]` HIGH — **But the shim's SID is 912 = containerd's session.** PGID is its own PID. So containerd's shim-spawn does `setpgid(0,0)` but **not** `setsid()`; the shim ends up orphaned into PID 1 by a fork-and-exit, not by session detachment.
`[O]` HIGH — **`docker-proxy` is a direct child of `dockerd` (PPID 70234), same PGID and SID.** dockerd deliberately keeps its userspace port-forwarders in its own process group — they die with dockerd, unlike shims.

`[O]` HIGH — Container init is a child of the shim: `grep -E "^(Name|Pid|PPid|NSpid)" /proc/2585529/status` →
```
Name: nginx     Pid: 2585529    PPid: 2585480
NStgid: 2585529 1      NSpid: 2585529 1      NSpgid: 2585529 1      NSsid: 2585529 1
```
So the container's init is PID 1 *inside* its PID namespace, is the session leader and process-group leader inside it, and its host-side parent is the shim. Since it was actually `clone()`d by `runc init` (which then exited), this parentage exists **only** because the shim set `PR_SET_CHILD_SUBREAPER` — see §5.4.

`[O]` HIGH — Start-time archaeology: `ps -p 912/70234/82248 -o lstart=` → containerd started `Sat Feb 28 13:00:43 2026`, dockerd started `Mon Mar 2 15:45:42 2026`, oldest shim `Mon Mar 2 21:50:56 2026`. **dockerd is younger than containerd**, i.e. dockerd was restarted while containerd and its shims kept running. That is the dockerd↔containerd decoupling working in production, observed rather than asserted.

`[O]` HIGH — `sudo docker info | grep -i live` → `Live Restore Enabled: false`, and there is no `/etc/docker/daemon.json`. So on this host dockerd *would* stop containers on its own shutdown; the decoupling above is dockerd's process being replaced, not live-restore.

## 2.2 What the shim actually holds — the fd table

`[O]` HIGH — `sudo ls -l /proc/2585480/fd` on `host-a` (shim for the `nginx:stable` frontend container). The complete, meaningful set:

| fd | Target | What it is |
|---|---|---|
| 0 | `/dev/null` | shim's own stdin |
| 1 | `/dev/null` | shim's own stdout |
| 2, 4, 7 | `/run/containerd/io.containerd.runtime.v2.task/moby/<id>/log` (a **FIFO**, `prwx------`) | shim's *own* log, drained by containerd |
| 3 | `anon_inode:inotify` | file watching |
| 5, 8 | `anon_inode:[eventpoll]` | Go netpoller epoll fds |
| 6 | `anon_inode:[eventfd]` | netpoller wakeup |
| **9** | `socket:[22485381]` | **the ttrpc listening socket** — confirmed by `ss -xlp`: `LISTEN /run/containerd/s/c362e1b7... users:(("containerd-shim",pid=2585480,fd=9))` |
| 10, 22 | sockets | accepted ttrpc connections from containerd |
| 11, 13 | `pipe:[22485389]`, `pipe:[22485390]` | **the container's stdout/stderr pipes** — these are exactly the two entries listed in runc's `state.json` `external_descriptors` |
| 12, 14, 15, 16 | `/run/docker/containerd/<id>/<id>-stdout` (FIFO) | shim's write + read ends of the *docker-side* stdout FIFO |
| 17, 19, 20, 21 | `/run/docker/containerd/<id>/<id>-stderr` (FIFO) | same for stderr |

`[O]` HIGH — `sudo readlink /proc/2585480/cwd` → `/run/containerd/io.containerd.runtime.v2.task/moby/<id>`. **The shim `chdir`s into the bundle.** That is why `runc` can be invoked with relative bundle paths and why the bundle directory cannot be removed while the shim lives.

`[O]` HIGH — the shim's namespaces are all the host's: `readlink /proc/2585480/ns/*` returns the same inode numbers as `/proc/1/ns/*` for cgroup/ipc/mnt/net/pid/time/user/uts. **The shim is not in the container.** Contrast the container init (2585529) which differs in mnt/uts/ipc/pid/cgroup/net but **shares** `user:[4026531837]` and `time:[4026531834]` with the host.

`[O]` HIGH — the shim's cgroup is `/system.slice/containerd.service` (`cat /proc/2585480/cgroup`), while the container init's is `/system.slice/docker-<id>.scope` (`sudo cat /proc/2585529/cgroup`). **The shim is deliberately NOT in the container's cgroup.** Consequence: a container that hits its memory limit gets OOM-killed without taking its supervisor with it, and the shim's own memory is not accounted to the container. `[R]` HIGH — copy this. A supervisor inside the supervised cgroup is a design bug: the OOM killer will eventually pick it, and then you lose the exit status of the thing you were supervising.

`[O]` HIGH — `sudo cat /sys/fs/cgroup/system.slice/docker-<id>.scope/cgroup.procs` → `2585529 2585606 2585607 2585608 2585609` — the nginx master + 4 workers, and nothing else. Confirms the shim's absence.

## 2.3 Boundary-by-boundary: what it buys, what breaks without it, does it apply to us

### Boundary 1: `docker` CLI → `dockerd` (HTTP/REST over `/var/run/docker.sock`)

**Buys:** (a) a remote-capable API — the same protocol works over TCP/TLS and over SSH; (b) the CLI needs no privilege — socket group membership is the whole authorization model; (c) the CLI is a *client*, so it can be a different version, a different language, or absent entirely (compose, portainer, CI); (d) **the CLI is not in the container's process ancestry**, so `Ctrl-C` on `docker run` does not kill the container unless the CLI chooses to forward the signal.

`[C]` HIGH — `attach` and `exec start` use HTTP hijacking: the connection is upgraded off HTTP and becomes a raw bidirectional byte stream. When the container has no TTY, the stream is **multiplexed with an 8-byte frame header**: `moby/api/pkg/stdcopy/stdcopy.go` defines `stdWriterPrefixLen = 8`, `stdWriterFdIndex = 0`, `stdWriterSizeIndex = 4`, and `StdType` values `Stdin=0, Stdout=1, Stderr=2, Systemerr=3`. With a TTY there is a single unframed stream, because a pty has only one output side. `[O]` HIGH — fetched from `https://raw.githubusercontent.com/moby/moby/master/api/pkg/stdcopy/stdcopy.go` via `curl -sL`.

**Breaks without it:** remote control; multi-client attach; version skew tolerance; the ability to `docker logs` a container whose creating CLI is long gone.

**Applies to us?** `[R]` HIGH — **No, not initially, and the reason matters.** Every benefit above is a *multi-client / remote* benefit. Our stated scope is single-machine, root-first, learning-oriented, no Kubernetes. The one benefit that is *not* about remoting — "the CLI is not in the container's ancestry" — we can get for free with a fork/exec supervisor (§7). Adding a REST API in phase 1 buys us an HTTP server, a versioned API surface, a socket permission model, and a second serialization format, in exchange for nothing we need. **But** see §7.4: the *state model* must be designed as if multiple concurrent clients exist, because two `mydocker` invocations at once is a real scenario even with no daemon.

### Boundary 2: `dockerd` → `containerd` (gRPC over `/run/containerd/containerd.sock`)

**Buys:** (a) container lifecycle survives dockerd restarts and dockerd crashes — this is the reason the split was made in 2016–17; (b) containerd is a reusable component with a CRI plugin, so Kubernetes can skip dockerd entirely; (c) the content-addressed image store and snapshotter are separated from the Docker API surface; (d) `namespaces` (`moby`, `k8s.io`, `default`) let independent consumers share one containerd without seeing each other's containers.

`[O]` HIGH — the namespace shows up everywhere on disk: `/run/containerd/io.containerd.runtime.v2.task/moby/<id>` and `/var/lib/containerd/io.containerd.runtime.v2.task/moby/`, and in the shim argv `-namespace moby`.

**What dockerd still owns after the split** `[O]` HIGH from `docker info` and the filesystem on `host-a`: the REST API; `docker-proxy` userspace port forwarding (4 processes for :80/:443 v4+v6, `-use-listen-fd`); libnetwork (`/run/docker/libnetwork/*.sock`, `/run/docker/netns/*` — **dockerd owns the network namespaces, not containerd**); the json-file logger; volumes (`/var/lib/docker/volumes`); buildkit; and the *container* concept as opposed to the *task* concept.

`[O]` HIGH — dockerd is the **reader** of the container's stdout/stderr FIFOs and the writer of the json log. `sudo ls -l /proc/70234/fd` shows, for the same container id: fd 43 = `.../<id>-stdout` opened **read-only**, fd 45 = `.../<id>-stderr` read-only, fd 42 = `/var/lib/docker/containers/<id>/<id>-json.log` opened **write-only**. That is the whole `docker logs` pipeline in three file descriptors: shim writes FIFO → dockerd reads FIFO → dockerd appends framed JSON to disk.

`[R]` HIGH — **This is the boundary whose *rationale* is real but whose *mechanism* is heavier than we need.** "Restart the management layer without killing containers" is genuinely valuable and we should preserve the *property*. But the property does not require a second daemon — it requires that **nothing in the container's supervision path is owned by the management layer**. If we have no daemon at all, the property is free. Note the design smell that this split introduced and we should avoid: **the logging reader lives in a different process from the writer, so if dockerd is down, the container's output blocks once the FIFO buffer (64 KiB default) fills.** `[F]` MED — a FIFO with no reader and a full buffer blocks the writer (`pipe(7)`); confidence MED because I did not measure the 64 KiB pipe capacity on this host, and it is tunable via `F_SETPIPE_SZ`. What would settle it: `fcntl(fd, F_GETPIPE_SZ)` on the shim's FIFO fd — not run, because it needs a process that holds the fd.

### Boundary 3: `containerd` → `containerd-shim-runc-v2` (ttrpc over a per-container Unix socket)

This is the boundary worth understanding in depth, because it is the one people cargo-cult.

`[C]` HIGH — the shim protocol (containerd `docs/runtime-v2.md`): "Each shim MUST implement a `start` subcommand. This command will launch new shims." The short-lived `start` invocation writes the ttrpc address to stdout and exits; the real shim daemon it forked is left running. Modern containerd (2.3+) replaces the flag-based handshake with a `BootstrapParams`/`BootstrapResult` protobuf pair on stdin/stdout. There is also a mandatory `delete` subcommand: "This command allows containerd to delete any container resources created, mounted, and/or run by a shim when containerd can no longer communicate over rpc," and it "MUST ensure that filesystem is also unmounted."

`[O]` HIGH — the address the shim published is on disk in the bundle: `sudo cat .../<id>/address` → `unix:///run/containerd/s/c362e1b74925b3370d2b9b57b6deedbe5d4b6d1715fa4caec70fb2d23d1a802e`, and `ss -xlp` shows exactly that path LISTENing on the shim's fd 9. So **containerd's reconnect path after a containerd restart is: read `address` from the bundle dir, dial it.** No process-tree walking, no pidfile of the shim itself.

`[O]` HIGH — the bundle dir is the shim's entire persistent handoff:
```
address            unix:///run/containerd/s/c362e1b7...        (how to talk to me)
runtime            runc                                        (which OCI runtime)
shim-binary-path   /usr/bin/containerd-shim-runc-v2
options.json       {"binary_name":"runc","root":"/var/run/docker/runtime-runc","systemd_cgroup":true}
init.pid           2585529                                     (host pid of container init)
config.json        9907 bytes                                  (the OCI spec)
log                FIFO                                        (shim log -> containerd)
log.json           0 bytes
rootfs/            empty dir                                   (mount target; empty because
                                                                overlay is mounted by dockerd
                                                                and passed as Root.Path instead)
work ->            /var/lib/containerd/io.containerd.runtime.v2.task/moby/<id>   (symlink to persistent side)
```
`[O]` HIGH — `sudo runc --root /run/docker/runtime-runc/moby state <id>` reports `"rootfs": "/var/lib/docker/overlay2/ba89fa61.../merged"` while `.../bundle/rootfs` is empty. Confirms that in the Docker path, **dockerd mounts overlay and hands runc an absolute `root.path` elsewhere**, rather than the shim mounting into `bundle/rootfs`. (In the pure containerd/CRI path the shim does the mounting — the `docs/runtime-v2.md` text about the shim being "responsible for mounting the filesystem into the `rootfs/` directory of the bundle" describes that path.)

**So: what does the shim actually exist for?** Five concrete jobs, each with a named failure mode:

1. **Hold the container's stdio open across the container's whole life.** `[O]` fds 11/13 (pipes to the container) + 12–21 (FIFOs to dockerd). Without it, the pipes' read end is owned by whichever process created them. If that is `runc`, `runc` must stay alive for the container's whole lifetime — which is exactly runc's "foreground" mode, and the runc docs say why that's bad: "any bug in the long-running foreground `runc` process (such as a memory leak) or a stray OOM-kill sweep could result in your container being killed **through no fault of the user**." `[C]` HIGH, runc `docs/terminals.md`.
2. **Own the pty master for a `terminal: true` container.** runc's `create` sends the master over a `--console-socket` with `SCM_RIGHTS` and then exits. Somebody must be the recipient and hold it. `[C]` HIGH, runc `docs/terminals.md`: "After `runc` exits, the only process with a copy of the pseudo-terminal master file descriptor is whoever read the file descriptor from the socket." Without a shim, that fd's owner is the CLI, and the pty dies with the CLI — **which closes the container's controlling terminal and sends SIGHUP to the container**.
3. **Be the reaping parent, so exit status is not lost to PID 1.** `[C]` HIGH, containerd `docs/runtime-v2.md`: "The shim process takes responsibility as a sub-reaper to cleanup exited containers or setns(2) processes." See §5.4 for the empirical proof of the mechanism.
4. **Be restartable-independently and killable-independently.** containerd can restart; the shim keeps the container. Conversely the shim can be `SIGKILL`ed and containerd notices (ttrpc connection closes) and invokes `shim delete` to clean up. `[C]` HIGH: "If a bundle is still on disk but containerd cannot connect to a shim, the delete command is invoked."
5. **Publish the exit event.** `[C]` HIGH — mandatory events with ordering guarantees: `TaskCreate` → `TaskStart` → `TaskExit` → `TaskDelete`, and "a call to `Start` is required to have published the async event `TaskStartEventTopic` before the shim can publish the `TaskExitEventTopic`."

**What breaks in practice — real failure modes, not hypotheticals:**

`[C]` MED — containerd issue #13849: "containerd-shim-runc-v2 remains alive with zero children after container PID 1 death, blocking all daemon API calls." The reported shape: shim parked in `futex_wait_queue`, no children, `TaskExit` never emitted, so Docker reports the container as still running and every API call against it (`stop`/`kill`/`rm -f`/`inspect`/`logs`) hangs or fails; containers eventually show `Exited (255)`. Confidence MED — GitHub issue, not a post-mortem; I did not reproduce it (production host).
`[C]` MED — containerd issue #10727: killing the shim does not necessarily kill the container process, and containerd may then start a *second* shim and a *second* container process. Confidence MED, same reason.
`[C]` MED — containerd issue #3334: shims left behind after their containers are removed.

`[R]` HIGH — **The lesson is not "shims are bad", it is: the exit status of a supervised process is state that exists in exactly one place — the reaping parent's memory — and if that process dies, the state is gone.** Every one of those bugs is a variant of "the single in-memory holder of the exit status became unreachable". Design implication for us in §6.5: **write the exit status to disk before publishing it anywhere**, and make the on-disk record, not a live process, the source of truth for `mydocker ps`.

**Applies to us?** `[R]` HIGH — **The *functions* apply; the *separate binary and RPC protocol* do not.** We need something that holds stdio, holds the pty, reaps, and records the exit status. We do **not** need that thing to be reachable by RPC from a daemon, because we have no daemon. Concretely: a `mydocker supervise` subcommand of the same binary, fork/exec'd and orphaned, with no listening socket in phase 1 (§7).

### Boundary 4: shim → `runc` (fork/exec of a separate binary, per operation)

**Buys:** (a) the shim is Go and does not have to solve the Go-runtime-vs-`setns` problem — it delegates to a binary that has solved it in C; (b) runtime pluggability — `options.json`'s `binary_name` is literally a string, so swapping `runc`→`crun`→`youki` is a config change (`[O]` HIGH, `options.json` on `host-a` contains `"binary_name":"runc"`); (c) the dangerous privileged code is in a process that exits, so a bug in it cannot corrupt long-lived state; (d) `runc` is a *stateless CLI over a state directory*, so any process with the right `--root` can query or kill a container — `[O]` HIGH, I ran `runc --root /run/docker/runtime-runc/moby list` as an unrelated process and got correct output for all 5 live containers.

**Breaks without it:** you must implement namespace creation inside the supervisor. In Go that is near-impossible without the C trick. In Rust it is entirely possible (§4.4).

**Applies to us?** `[R]` MED — **This is the boundary we should collapse, and it is the single biggest simplification available to us.** We own the runtime core by project charter, we are writing Rust (no Go thread problem), and property (d) — statelessness over a state dir — is a property of the *state model*, not of the process split. We keep (d) by writing state to disk; we lose (b) runtime pluggability, which we do not want anyway; we lose (c) "the dangerous code exits", which we partially recover by doing the dangerous work in a **child that either `execve`s the user process or dies**, never in the long-lived supervisor. Confidence MED not HIGH because collapsing this boundary means our supervisor process *is* the process that called `clone(CLONE_NEWPID|...)`, and that couples the supervisor's lifetime to the creation path in a way runc's design deliberately avoids. §7.2 resolves this with a three-process shape that keeps the collapse but not the coupling.

### Boundary 5: `runc init` → kernel

Owned by D1 (syscalls). My part: *why the boundary is a separate process at all* — §4.

---

# 3. Why runc splits `create` and `start`

## 3.1 The spec reason

`[F]` HIGH — The split is **mandated**. `create` "MUST apply all config.json properties except `process`"; `start` "MUST run the user-specified program". A conforming runtime cannot fuse them. (runtime-spec `runtime.md`.)

`[F]` HIGH — The *purpose* of the gap is the hook window: steps 3–5 (`prestart`, `createRuntime`, `createContainer`) all happen between "namespaces exist" and "user process runs". Without the gap there is no moment at which an external tool can put a veth into the container's netns *knowing the netns exists* and *knowing the user process has not started yet*. That is the whole reason CNI works.

`[C]` HIGH — Second, non-spec reason: **it separates "did the container fail to be constructed" from "did the user program fail"**. `runc create` returning non-zero means the sandbox couldn't be built. `runc start` succeeding and the container exiting 127 means the binary wasn't there. Higher layers depend on that distinction. Docker's `Exited (127)` vs "cannot start container" are different UX paths.

## 3.2 The mechanism: `exec.fifo`

`[C]` HIGH — runc `libcontainer/container_linux.go`:

```go
func (c *Container) createExecFifo() (retErr error) {
    rootuid, err := c.config.HostRootUID()
    fifoName := filepath.Join(c.stateDir, execFifoFilename)
    if err := unix.Mkfifo(fifoName, 0o622); err != nil { ... }
    if err := os.Chmod(fifoName, 0o622); err != nil { ... }   // umask defence
    return os.Chown(fifoName, rootuid, rootgid)
}
```
Mode `0622` = `rw--w--w-`: the owner reads, **everyone can write**. That is deliberate: the *container's* init (possibly an unprivileged uid inside a userns) must be able to open it for writing.

`[C]` HIGH — the fd is passed to `runc init` as an **`O_PATH`** descriptor via `_LIBCONTAINER_FIFOFD`:
```go
fifo, err := os.OpenFile(fifoName, unix.O_PATH|unix.O_CLOEXEC, 0)
```
`O_PATH` because a real `O_WRONLY` open would *itself* block at open time, in the parent. `O_PATH` gets you a reference to the fifo without opening it; the child later re-opens it through `/proc/self/fd/N`.

`[C]` HIGH — `libcontainer/standard_init_linux.go`, the last thing before `execve`: reopen the `O_PATH` fd for writing and write one byte — "*Wait for the FIFO to be opened on the other side before exec-ing the user process*". Then, critically: "*Close the O_PATH fifofd fd before exec because the kernel resets dumpable in the wrong order*" — a CVE-2016-9962 regression guard.

`[C]` HIGH — the parent side, `runc start` → `container.exec()`:
```go
func (c *Container) exec() error {
    path := filepath.Join(c.stateDir, execFifoFilename)
    if err := handleFifo(path, c.initProcess.pid()); err != nil { return err }
    return c.postStart()
}
func readFromExecFifo(execFifo io.Reader) error {
    var buf [1]byte
    n, err := execFifo.Read(buf[:])
    if n == 0 {
        return errors.New("exec fifo is empty: container init did not signal execve readiness (process died before writing, or fifo already consumed)")
    }
}
```

`[O]` HIGH — **The FIFO rendezvous semantics, verified on `host-a` (kernel 6.11.0-29)** with `mkfifo -m 0622 /tmp/f2` and a Python script in `/tmp`: a thread calling `os.open("/tmp/f2", O_WRONLY)` was still blocked in `open()` at t=1.000 s; a blocking `os.open(..., O_RDONLY)` at t=1.000 s returned immediately, and *both* the writer's `open()` and the reader's `read()` completed at t=1.001 s, with the reader receiving `b'\x00'`. So:
- `open(O_WRONLY)` on a FIFO blocks until a reader opens. **This is the whole synchronization primitive.**
- The container's init is parked in `open()`, not in `read()` or a `wait()`. It holds no lock, consumes no CPU, and needs no message protocol.

`[R]` HIGH — **Adopt the FIFO rendezvous verbatim.** It is 10 lines, it is kernel-provided, it survives the parent exiting, it works across a userns boundary, and it costs one inode. The alternatives are worse: a socketpair dies when the create-side process exits; `SIGSTOP`/`SIGCONT` is racy and visible to the container; an eventfd cannot be reopened by path. One Rust-specific note: Go's `os.File` on a FIFO opened `O_NONBLOCK` still gives you a *logically blocking* `Read` via the netpoller. **In Rust, open the read side blocking (`O_RDONLY` with no `O_NONBLOCK`) and `read()` blocking.** `[O]` MED — my first attempt used `O_RDONLY|O_NONBLOCK` and `read()` returned `EAGAIN` because the write raced; the blocking version worked first try. Confidence MED on this being the *only* correct choice — a nonblocking read plus a poll loop is also correct, just longer.

`[C]` HIGH — **The FIFO's existence is also runc's `created` vs `running` discriminator**:
```go
// The presence of exec fifo helps to distinguish between
// the created and the running states.
if _, err := os.Stat(filepath.Join(c.stateDir, execFifoFilename)); err == nil {
    return c.state.transition(&createdState{c: c})
}
return c.state.transition(&runningState{c: c})
```
`[R]` MED — This is clever but slightly fragile (it conflates "fifo file exists" with "start has not been called"). `[R]` HIGH — **Prefer an explicit `status` field in our state file, written at each transition, with the FIFO as the mechanism only.** Tradeoff: one extra `fsync`-less atomic rewrite per transition, versus a state machine you can debug by reading one file.

## 3.3 `nsexec.c` — the two-stage design and its sync protocol

`[C]` HIGH — `libcontainer/nsenter/README.md`: the whole reason the C exists is *"This provides us the ability to `setns` on existing namespaces and avoid the issues that the Go runtime has with multiple threads."* And: *"Because `nsexec()` must be run before the Go runtime in order to use the Linux kernel namespace, you must `import` this library into a package if you plan to use `libcontainer` directly."* The C function runs from a `__attribute__((constructor))`, i.e. before `main`, i.e. before the Go runtime has spawned a single thread.

`[C]` HIGH — three stages, implemented with `setjmp`/`longjmp` and `clone(CLONE_PARENT)` (runc `libcontainer/nsenter/nsexec.c`):

```
STAGE_PARENT (0)   runc's own process. Retains full privilege. Orchestrates.
STAGE_CHILD  (1)   clone()d from parent with CLONE_PARENT. unshare()s namespaces
                   (notably CLONE_NEWUSER). Then clones stage 2 and exits.
STAGE_INIT   (2)   grandchild. The actual container init. setsid, setuid/setgid,
                   then longjmps back and lets the Go runtime boot -> `runc init`.
```

`[C]` HIGH — the sync alphabet on the netlink init pipe:
```
SYNC_USERMAP_PLS     0x40   stage1 -> parent: "write my uid_map/gid_map, I can't"
SYNC_USERMAP_ACK     0x41   parent -> stage1: done
SYNC_RECVPID_PLS     0x42   stage1 -> parent: "here is stage2's pid"
SYNC_RECVPID_ACK     0x43   parent -> stage1
SYNC_GRANDCHILD      0x44   parent -> stage2: proceed
SYNC_CHILD_FINISH    0x45   child/grandchild -> parent: done
SYNC_TIMEOFFSETS_PLS 0x46   stage1 -> parent: write timens offsets
SYNC_TIMEOFFSETS_ACK 0x47
```

**Why three stages, precisely:**

`[F]` HIGH — Reason A, the PID namespace: `setns(2)`/`unshare(2)` with `CLONE_NEWPID` do **not** move the calling process into the new PID namespace; they affect only *subsequently created children*. The nsexec comment says it: *"the PID namespace only applies to children... setns(2) or unshare(2) don't change the PID namespace of the calling process."* This is `pid_namespaces(7)` behaviour, and it means **the process that creates a PID namespace can never be PID 1 in it.** A fork is unavoidable. This is a **kernel constraint. Nobody escapes it.**

`[F]` HIGH — Reason B, the user namespace: *"if we unshare(2) the user namespace *before* we clone(2), then all hell breaks loose. The parent no longer has permissions to do many things (unshare drops all capabilities in your old namespace)."* Writing `/proc/<pid>/uid_map` for a userns requires `CAP_SETUID`/`CAP_SETGID` **in the parent userns**. So somebody outside the new userns must do the write. Hence: stage 1 unshares the userns and asks stage 0 (still privileged) to write its maps. This is a **kernel constraint** too — though it only binds when you actually use a user namespace.

`[C]` HIGH — Reason C, `CLONE_PARENT`: both clones use `CLONE_PARENT`, so stage 2's *parent* is stage 0, not stage 1. That lets stage 1 exit immediately without orphaning stage 2 to init and without stage 0 needing subreaper semantics for it. `[C]` HIGH — runc still reaps stage 1 explicitly: `firstChildProcess.Wait()` in `getChildPid` (`libcontainer/process_linux.go`), because stage 1 becomes a zombie the moment it exits.

`[C]` MED — Bootstrap data is passed to the C as a **netlink-style TLV** stream on a socketpair (`_LIBCONTAINER_INITPIPE`). Attribute types (`libcontainer/message_linux.go`): `InitMsg=62000`, `CloneFlagsAttr=27281`, `NsPathsAttr=27282`, `UidmapAttr=27283`, `GidmapAttr=27284`, `SetgroupAttr=27285`, `OomScoreAdjAttr=27286`, `RootlessEUIDAttr=27287`, `UidmapPathAttr=27288`, `GidmapPathAttr=27289`, `TimeOffsetsAttr=27290`. The only comment explaining the numbers is *"The number is randomly chosen to not conflict with known netlink types."* `[R]` HIGH — the *actual* engineering reason is that C needs a trivially-parseable binary TLV and `nlmsghdr` + `nlattr` is a TLV format the C side already has headers for; there is no networking involved. Confidence on that being the reason: MED (it is inference from the code shape, not a quoted rationale). **Irrelevant to us**: we have one language on both sides of that boundary, so this whole encoding disappears.

## 3.4 The decisive question: Go artifact vs kernel-imposed

| runc design element | Why it exists | Does Rust inherit it? |
|---|---|---|
| A separate `runc init` **re-exec of self** | Go's runtime is multithreaded from `main`; `setns(2)` is **per-thread**, so you cannot reliably move a Go program into a namespace. `[F]` HIGH `setns(2)` man page: it operates on the calling *thread*. | **NO.** Rust's `main` is single-threaded unless you make it otherwise. This is the biggest inherited-complexity saving. |
| `nsexec.c` in **C** with `__attribute__((constructor))` | Must run before the Go runtime spawns threads. `[C]` HIGH nsenter/README. | **NO.** Rust has no runtime to pre-empt. Direct syscalls from `main` are fine. |
| The netlink TLV bootstrap encoding | C↔Go boundary needs a language-neutral wire format. | **NO.** Same language both sides; pass a struct, or don't pass anything because it's the same process. |
| `setjmp`/`longjmp` between stages | Consequence of "the C constructor must return control to Go exactly once, on the right stage". | **NO.** Artifact of the constructor trick. |
| A **fork** to become PID 1 of a new PID namespace | `pid_namespaces(7)`: `CLONE_NEWPID` affects children only. | **YES. Unavoidable, in every language.** |
| A **privileged process outside the new userns** to write `uid_map`/`gid_map` | `user_namespaces(7)`: writing the map needs caps in the parent userns. | **YES, when using a userns.** Root-first means we can defer this; rootless makes it mandatory. |
| Applying cgroups to the **stage-1 pid, before it forks stage 2** | Race: if you cgroup the final init, its own children created during setup are outside the cgroup for a window. runc comment: *"Do this before syncing with child so that no children can escape the cgroup."* | **YES, the ordering constraint is real.** But cgroup v2 gives us a cleaner option: create the cgroup, and have the *child itself* write its pid to `cgroup.procs` before it does anything else — or use `clone3(CLONE_INTO_CGROUP)` and be in the cgroup from birth. See §4.5. |
| Two processes (`create` leaves init parked; `start` unparks it) | **Spec-mandated.** | **YES.** |
| `runc` exiting after `create` (detach) | Convention, driven by the shim design. | **NO** — our supervisor can be the create-side process and just stay. |
| `PR_SET_CHILD_SUBREAPER` in the supervisor | Needed because the process that `clone()`d init exits. | **Conditionally NO** — if our supervisor is itself the direct parent of init, we don't need it. But we *do* need it for `exec`-ed processes and for grandchildren. §5.4. |

`[R]` HIGH — **The Rust simplification, stated plainly: we need exactly two process boundaries, not four.**
1. A **supervisor** process (single-threaded at the point it forks) that stays for the container's lifetime.
2. A **container init** child, `clone()`d with the namespace flags, which sets everything up and `execve`s the user program.

runc has: `runc create` (stage 0) → stage 1 → stage 2 → `runc init` Go re-exec → user program, plus a shim above it. That's five. Three of the five exist to work around Go.

`[R]` MED — One caveat, and it is the reason I don't say "two boundaries, done": **when you add a user namespace, you need the map-writer outside the userns, and when you add `CLONE_NEWPID` you need the fork.** If the supervisor is the map-writer *and* the parent of init, that's still two processes — the supervisor writes stage-2's maps directly. So the two-process shape holds even for rootless. What forces a third is if you want the *user-namespace-creating* process to also be inside a *different* set of namespaces (e.g. joining an existing netns by path while creating a new userns). `[R]` — cross that bridge when rootless lands; note it as a shape the design must not preclude.

## 3.5 crun and youki

Deferred to §11 (architecture families), where I have the source-level detail. Summary of my own reading of the question: crun (C, no runtime threads) still keeps the create/start split because the *spec* mandates it, but does not need any of the constructor/re-exec machinery. youki (Rust) is the direct evidence for whether the Rust simplification is real.

---

# 4. The creation sequence — who does what, and when (ordering constraints only)

D1 owns the syscalls. I own the *ordering constraints that come from the process structure*.

`[C]` HIGH — runc's parent-side order, from `libcontainer/process_linux.go` `initProcess.start()`:

```
 1. cmd.Start()                       fork/exec `runc init` (= stage 0 spawns stage 1)
 2. comm.closeChild()                 parent drops the child ends of the pipes
 3. manager.Apply(p.pid())            <-- CGROUP APPLIED TO STAGE-1 PID
 4. tryResetCPUAffinity(p.pid())
 5. intelRdt Apply
 6. copy bootstrapData -> initSock    the netlink TLV
 7. getChildPid()                     read {PidFirstChild, Pid} JSON back
 8. waitForChildExit(childPid)        reap stage 1; retarget cmd.Process to stage 2
 9. getPipeFds(childPid)              record /proc/<pid>/fd/{0,1,2} targets BEFORE
                                      the container can move them  --> state.json
10. goCreateMountSources()            goroutine servicing procMountPlease
11. createNetworkInterfaces()
12. parseSync() loop:
      procSeccomp        -> receive seccomp notify fd, forward to listener
      procMountPlease    -> serve mount source requests
      procHooks          -> run Prestart + CreateRuntime hooks HERE
      procReady          -> set rlimits, record created-at, WRITE state.json,
                            reply procRun
13. syncSockParent.Shutdown(SHUT_WR)
14. assert seenProcReady
```

`[C]` HIGH — the cgroup-timing comment: *"Do this before syncing with child so that no children can escape the cgroup."*

`[C]` HIGH — step 9 matters and is easy to miss. `external_descriptors` in `state.json` records what fds 0/1/2 pointed at *at creation time*, precisely because the container may later replace them. `[O]` HIGH — verified on `host-a`: `state.json` for the nginx container contains
```json
"external_descriptors": ["/dev/null", "pipe:[22485389]", "pipe:[22485390]"]
```
and those two pipe inodes are exactly the shim's fds 11 and 13. That is how `runc exec` later knows what to wire a new process's stdio to when no explicit fds are given.

`[C]` HIGH — hook placement: `procHooks` (prestart + createRuntime) fires **after** step 3, i.e. **after cgroup limits are applied**. So a `createRuntime` hook runs under the container's cgroup limits even though it runs in the runtime's namespaces. Non-obvious, and a real source of "my CNI plugin got OOM-killed" reports.

`[C]` HIGH — the error path. On any sync failure runc does, in order: `syncSockParent.Shutdown()` (tells the child to die) → `p.wait()` (reap the zombie) → deferred `manager.Destroy()` (remove cgroup dirs) → `intelRdtManager.Destroy()` → `p.terminate()` (SIGKILL + wait). And it checks `manager.OOMKillCount()` before terminating so it can annotate the error `"container init was OOM-killed"` instead of reporting a mystery. `[R]` HIGH — **copy that last trick.** "Container creation failed" with no reason is the single most common unhelpful container error; reading the cgroup's OOM counter on failure is nearly free and turns it into a diagnosis.

`[R]` HIGH — **Our ordering constraints, distilled:**
1. Create the cgroup **before** forking anything that will hold the container.
2. Put the child in the cgroup **before** it forks anything of its own. With cgroup v2 this is either (a) child writes its own pid to `cgroup.procs` as its first act, or (b) `clone3(CLONE_INTO_CGROUP)`. Option (b) is strictly better and removes the race entirely — `[F]` MED, `clone3(2)` with `CLONE_INTO_CGROUP` requires kernel ≥ 5.7; `[O]` HIGH `clone3` is present on `host-a` (kernel 6.11): `syscall(435, 0, 0)` returned `-1 EINVAL` (22), not `ENOSYS` (38), proving the syscall number is implemented. Confidence MED on `CLONE_INTO_CGROUP` specifically because I did not exercise the flag.
3. Record the *pre-container* identity of fds 0/1/2 at create time; you cannot recover it later.
4. Write the state file at the moment the child says "ready", not before and not after.
5. On any failure after the fork: kill the child, reap it, then unwind resources in reverse creation order, and read the cgroup's OOM counter before reporting.

---

# 5. Detached execution and I/O mechanics — the quiet-breakage zone

## 5.1 Detaching: double-fork vs setsid vs orphan-into-init

Three distinct mechanisms, routinely conflated:

`[F]` HIGH — **fork-and-exit (orphaning)**: parent forks, child continues, parent exits. The child is reparented to the nearest ancestor marked `PR_SET_CHILD_SUBREAPER`, else to PID 1. This severs the process from the *original parent's* lifetime.
`[F]` HIGH — **`setsid()`**: creates a new session and process group, and **detaches from the controlling terminal**. `setsid(2)` fails with `EPERM` if the caller is already a process-group leader — which is why the idiom is `fork(); if (child) setsid();`.
`[F]` HIGH — **double fork**: fork, child `setsid()`s, child forks again and exits, grandchild continues. The second fork guarantees the surviving process is **not a session leader**, which means it can never acquire a controlling terminal by opening a tty. That is the only thing the second fork buys.

`[O]` HIGH — containerd does **fork-and-exit + setpgid, but NOT setsid**. On `host-a`, all five shims have `PPID=1`, `PGID=<own pid>`, `SID=912` — and 912 is containerd's own pid and session. So the shim is orphaned into init and is a process-group leader, but remains in containerd's session. `[R]` MED — for us, `setsid()` in the supervisor is *cheaper and more correct* than inheriting a session: it guarantees no controlling terminal, so a stray `SIGHUP` on the invoking terminal cannot reach the supervisor. Tradeoff: a `setsid()`'d supervisor cannot be `Ctrl-C`'d, which is what you want for a detached container and *not* what you want for a foreground one. So: `setsid()` only on the detached path.

`[C]` HIGH — runc's own detach story (`utils_linux.go`): `detach := r.detach || (r.action == CT_ACT_CREATE)`. **`runc create` is unconditionally detached.** And on the detached path `runner.run()` returns `0` immediately after console setup and pid-file writing — *"Detached mode always returns 0 (success) regardless of process outcome."* So **`runc create`'s exit code tells you nothing about the container's exit code, by construction.**

## 5.2 The console socket + `SCM_RIGHTS` protocol

`[C]` HIGH — runc `docs/terminals.md` + `utils_linux.go` `setupIO()`. Three modes:

**(a) detached + `terminal: true` → `--console-socket` required.**
```
caller: socket(AF_UNIX, SOCK_STREAM) ; bind(path) ; listen()
caller: exec runc create --console-socket path ...
runc:   openpty()  -> (master, slave)
runc:   dup slave onto container init's 0/1/2 ; ioctl(TIOCSCTTY) in the container
runc:   connect(path) ; sendmsg(SCM_RIGHTS, [master])
runc:   exit
caller: recvmsg() -> master fd.  "After runc exits, the only process with a copy of
        the pseudo-terminal master file descriptor is whoever read the file
        descriptor from the socket."
```
`[C]` HIGH — runc *enforces* the pairing in `checkTerminal()`:
```
"cannot allocate tty if runc will detach without setting console socket"
"cannot use console socket if runc will not detach or allocate tty"
```

**(b) foreground + `terminal: true`** — runc creates a socketpair internally, receives the master itself, and copies bytes between the host's tty and the master for the container's whole life. *"All `stdio` is buffered through the foreground `runc` process (irrespective of which terminal mode you are using)."*

**(c) `terminal: false`** — detached: `inheritStdio(process)`, the container gets runc's *actual* fds 0/1/2. Foreground: `setupProcessPipes()`, pipes with runc copying.

`[R]` HIGH — **Implement (a) and (c)-detached only. Skip foreground-copying entirely in phase 1.** Reasoning: mode (b) requires a byte-copying loop with `epoll` in the supervisor, plus tty raw-mode save/restore on the host, plus `SIGWINCH` handling — all of it re-implementable later, none of it needed for `mydocker run -d nginx`. Tradeoff: no interactive `mydocker run -it` until phase 2. That is an acceptable, honest gap; it is *not* acceptable to fake it by handing the container the CLI's real tty (see the security note below).

`[C]` HIGH — **why detached pass-through with a real terminal is dangerous** (runc `docs/terminals.md`): *"a malicious container could take advantage of TTY-specific `ioctls` like `TIOCSTI` to fake input into the **host** shell."* Also: output interleaves indistinguishably, and stdin is *"non-deterministically split and given to either the container or the shell"* because both block on `read(2)` of the same fd. And: *"using detached pass-through will result in the container hanging if the `stdout` or `stderr` is a pipe"* (runc issue #1721).
`[R]` HIGH — **Never give a container a file descriptor that refers to the invoking terminal.** If the caller's stdout is a tty and the container is detached, redirect to a log file or a pty we own. This is not a nicety: `TIOCSTI` keystroke injection into the host shell is a full host compromise. (Modern kernels can disable `TIOCSTI` via `dev.tty.legacy_tiocsti`, but do not rely on it.)

## 5.3 How `attach` and `logs` actually work

`[O]` HIGH — Docker's, on `host-a`, reconstructed entirely from fd tables:
```
container init 0/1/2  --> pipe:[22485389] / pipe:[22485390]   (created by the shim)
shim fd 11,13         --> read ends of those pipes
shim fd 12,14,15,16   --> /run/docker/containerd/<id>/<id>-stdout   (FIFO, write+read)
shim fd 17,19,20,21   --> ...-stderr (FIFO)
dockerd fd 43         --> ...-stdout   (O_RDONLY)
dockerd fd 45         --> ...-stderr   (O_RDONLY)
dockerd fd 42         --> /var/lib/docker/containers/<id>/<id>-json.log  (O_WRONLY)
```
So: **`docker logs` reads a file. `docker attach` reads the live FIFO through dockerd's hijacked HTTP connection.** Two entirely separate paths over the same bytes. `[O]` HIGH — the json log for one 11-day-old container is 27,498,753 bytes (`ls -l /var/lib/docker/containers/301b.../301b...-json.log`), with no rotation configured, which is the well-known unbounded-growth footgun.

`[C]` HIGH — the FIFO indirection exists so that **dockerd can restart and re-open the FIFO** without the shim noticing. A pipe would break; a named FIFO can be re-opened by path. That is the entire reason for the extra hop.

`[R]` HIGH — **Our design: the supervisor writes the log file directly and also serves attach.**
- Container's 1/2 → pipes owned by the supervisor.
- Supervisor appends framed records to `<state>/<id>/log.json` (one JSON object per line: `{"t":RFC3339,"s":"stdout","m":"..."}`). This is Docker's json-file format and it is fine.
- `mydocker logs` = read the file. Zero IPC. `--follow` = read + inotify/`poll` on the file.
- `mydocker attach` = phase 2; the supervisor accepts on `<state>/<id>/attach.sock` and multiplexes. **Defer it.**

`[R]` HIGH — **Do not put a FIFO between the supervisor and the log file.** The FIFO hop exists only because Docker split the writer and the reader into two daemons. We have one. Tradeoff: our supervisor owns log rotation, so it must implement a size cap from day one — an unbounded log file is a production outage waiting to happen, and it is 20 lines to cap it.

`[R]` HIGH — **A hard rule that falls out of the fd analysis:** the supervisor must hold **both** ends of the container's stdout/stderr pipes, or at least never let the read end be the only reference. If the read end closes while the container writes, the container gets `SIGPIPE`/`EPIPE`. `[O]` HIGH — the shim demonstrably holds both ends of the FIFOs (fds 12/14/15/16 include both `l-wx` and `lr-x` on the same path), specifically so that the FIFO never has zero readers or zero writers and thus never returns EOF/EPIPE to the other side. **This is the single most-copied trick in the fd table and the easiest one to miss.**

## 5.4 Exit status collection when the parent is gone — the mechanism, proven

`[F]` HIGH — `prctl(PR_SET_CHILD_SUBREAPER, 1)` marks a process such that any orphaned descendant is reparented to it instead of PID 1, and it therefore receives `SIGCHLD` and can `wait()` for that descendant's status. (`prctl(2)`.)

`[O]` HIGH — **Verified on `host-a`, kernel 6.11.0-29-generic**, with a throwaway Python script in `/tmp` (`python3 /tmp/subreap_test.py plain` then `... subreaper`). Structure: supervisor forks a child; the child forks a grandchild that exits with status **7**; the child exits immediately with **0**, orphaning the grandchild; the supervisor loops `waitpid(-1, WNOHANG)` for 3 s.

```
### WITHOUT subreaper
SUPERVISOR pid=2773824 mode=plain     reaped=[(2773825, 0)]

### WITH subreaper
prctl(PR_SET_CHILD_SUBREAPER,1) -> 0 errno 0
SUPERVISOR pid=2773841 mode=subreaper reaped=[(2773850, 0), (2773851, 7)]
```
Host PID 1 is `systemd`.

**Read that carefully.** Without the flag, the supervisor collected exactly one status — its direct child's `0`. The grandchild's exit code **7 was delivered to systemd and is permanently unrecoverable.** With the flag, the supervisor collected both, including the 7. This is precisely the shape of `runc create` (child) forking the container init (grandchild) and then exiting: **without `PR_SET_CHILD_SUBREAPER` in the supervisor, the container's exit code goes to PID 1 and is lost forever.**

`[C]` HIGH — runc sets it too, in its foreground path: `system.SetSubreaper(1)` *"before registering the signal handler for the container"*, controlled by `--no-subreaper` (`utils_linux.go`). And the runc docs state the consequence of omitting it: failing to set it *"may result in zombie processes being accumulated on your host."*
`[C]` HIGH — containerd's shim does the same: *"The shim process takes responsibility as a sub-reaper to cleanup exited containers or setns(2) processes."*

`[R]` HIGH — **Set `PR_SET_CHILD_SUBREAPER` in our supervisor unconditionally, as the very first thing after `setsid()`.** Cost: one syscall. Benefit: exit codes are recoverable, and `mydocker exec`'d processes whose intermediate parent died still get reaped instead of accumulating zombies in the container. There is no downside for a process whose only job is supervising.

`[C]` HIGH — runc's reap loop shape is worth copying exactly (`signals.go`):
- `signal.Notify(s, SIGCHLD)` **first**, alone, then `signal.Notify(s)` for everything else in a goroutine — because *"signal.Notify is actually quite expensive, as it has to configure the signal mask and add signal handlers for all signals (all ~65 of them)"*. The Rust equivalent concern is a `signalfd`/`sigprocmask` setup cost, which is trivial — so ignore this specific optimisation.
- Buffer of **2048** signals: *"Ensure that we have a large buffer size so that we do not miss any signals in case we are not processing them fast enough."*
- On SIGCHLD, `reap()` loops `Wait4(-1, &ws, WNOHANG, &rus)` until it returns `pid <= 0`. **`-1` means any child, not just the container init** — a subreaper must drain everything.
- The container's status is the entry whose `pid == pid1`; everything else is discarded.

`[R]` HIGH — **In Rust: `signalfd` (or a self-pipe) + a `waitpid(-1, WNOHANG)` drain loop.** Do *not* use a signal handler that touches allocation or locks. `signalfd` turns SIGCHLD into a readable fd, which composes with `poll`/`epoll` alongside the stdio pipes and the attach socket — one event loop, no async-signal-safety hazards. Tradeoff: `signalfd` requires blocking SIGCHLD via `sigprocmask` first, and that mask is **inherited across `fork` but reset on `execve`** — so the container's process never inherits our blocked mask. `[F]` HIGH (`signalfd(2)`, `execve(2)`: the signal *mask* is preserved across execve, the *dispositions* are reset). **Correction and a real trap: `execve` preserves the signal mask.** So the supervisor must `sigprocmask(SIG_SETMASK, empty)` in the child after `fork` and before `execve`, or the container's PID 1 starts life with SIGCHLD blocked — which will silently break any container that reaps its own children. Confidence HIGH; this is a classic and it bites everyone once.

`[F]` MED — Modern alternative worth knowing: **`pidfd`**. `pidfd_open(pid, 0)` gives an fd that becomes readable when the process exits, is immune to PID reuse, and works with `poll`/`epoll`; `pidfd_send_signal` signals without a PID-reuse race; `waitid(P_PIDFD, fd, ...)` reaps it. `[O]` HIGH — available on `host-a` (kernel 6.11): `syscall(434, 1, 0)` returned fd `3`, errno 0 (a pidfd for PID 1). `[R]` MED — **use a pidfd for the container's init** in addition to SIGCHLD reaping: it removes the PID-reuse check entirely (see §6.1) and it makes "is the container alive" answerable without `/proc` parsing. Confidence MED because a pidfd is only useful to the process that holds it; it does not survive into the state file, so the `/proc/<pid>/stat` start-time check is still needed for the after-supervisor-died case.

## 5.5 Signal forwarding to the container's PID 1

`[C]` HIGH — runc forwards **by pid, to init only** (`signals.go`): `process.Signal(s)`, logged as *"forwarding signal %d (%s) to %d"*. Not cgroup-wide. Exceptions: `SIGCHLD` is consumed (triggers reap), `SIGURG` is ignored (Go runtime uses it for preemption), `SIGWINCH` triggers `tty.resize()`.

`[F]` HIGH — a signal sent from outside a PID namespace to that namespace's PID 1 has special semantics: **the kernel only delivers a signal to PID 1 of a namespace if PID 1 has installed a handler for it** — default actions do not apply to a namespace's init for signals sent from outside. (`pid_namespaces(7)`.) Exceptions: `SIGKILL` and `SIGSTOP` always work.
`[R]` HIGH — **This is why `docker stop` waits 10 s and then SIGKILLs**, and why so many containers "ignore" SIGTERM: their PID 1 (a shell, or an app that never installed a handler) genuinely cannot be terminated by SIGTERM from outside. Our `stop` must be `SIGTERM → timeout → SIGKILL`, and the docs must say why.

`[C]` HIGH — killing *all* processes: runc special-cases it. From `container_linux.go`: *"if s == unix.SIGKILL && !c.config.Namespaces.IsPrivate(configs.NEWPID) { ... kill all pids to avoid leftover processes"* — i.e. when the PID namespace is **shared**, killing init does not kill the rest, so runc enumerates and kills. And: *"For cgroup v1, killing a process in a frozen cgroup does nothing until it's thawed. Only thaw the cgroup for SIGKILL."*

`[R]` HIGH — **We should use `cgroup.kill`, and this is a genuine cgroup-v2-only simplification.** `[O]` HIGH — verified present on `host-a` (kernel 6.11): `sudo ls /sys/fs/cgroup/system.slice/docker-<id>.scope/` lists `cgroup.kill`, `cgroup.freeze`, `cgroup.events`, `cgroup.procs`. `[F]` MED — writing `1` to `cgroup.kill` sends SIGKILL to every process in the cgroup and its descendants, atomically, with no freeze/thaw dance and no enumeration race (`cgroup-v2` docs; kernel ≥ 5.14). Confidence MED on the exact semantics because I read the file's presence, not the kernel doc, in this session — what would settle it: `Documentation/admin-guide/cgroup-v2.rst`, section on `cgroup.kill`.
`[O]` HIGH — note `cgroup.kill` is **absent from the root cgroup**: `ls /sys/fs/cgroup/cgroup.kill` → not found, while it is present in `system.slice/docker-<id>.scope`. Expected (you cannot kill the root cgroup) but worth knowing before you write a probe that checks for the file at the root.

`[R]` HIGH — **A second cgroup-v2 lifecycle gift: `cgroup.events`.** `[O]` HIGH — `sudo cat .../docker-<id>.scope/cgroup.events` → `populated 1` / `frozen 0`. This file is `poll()`-able and transitions to `populated 0` when the last process in the cgroup exits. That gives us an **exit notification that does not require being the parent**. It does not give us the *exit code* — only the parent's `wait()` does — but it is a robust "the container is gone" signal that survives supervisor restarts and is immune to PID reuse. `[R]` MED — use it as a *cross-check*, not a replacement: `mydocker ps` on a container whose supervisor died can consult `cgroup.events` to distinguish "running, unsupervised" from "exited, status unknown".

## 5.6 Failure-mode catalogue — the things naive implementations get wrong

Each is a specific bug, not a category.

| # | Bug | Symptom | Fix |
|---|---|---|---|
| 1 | Supervisor doesn't set `PR_SET_CHILD_SUBREAPER` | container exit code is always 0 or "unknown"; zombies accumulate under PID 1 | `[O]` proven in §5.4. Set it first thing. |
| 2 | Supervisor is in the container's cgroup | container OOMs → supervisor is a candidate → status lost, container orphaned | `[O]` Docker puts the shim in `containerd.service`, not the container scope. Keep the supervisor out. |
| 3 | Child inherits the blocked signal mask | container's PID 1 can't reap its own children; `zombie` processes inside the container | `execve` preserves the mask. Reset to empty between `fork` and `execve`. |
| 4 | Only one end of the stdio pipe held | container gets `EPIPE`/`SIGPIPE` and dies when the log reader restarts | `[O]` shim holds both ends of each FIFO. |
| 5 | Log file unbounded | disk full outage | `[O]` 27 MB in 11 days for one nginx on `host-a`, no rotation. Cap from day one. |
| 6 | Detached + real tty passed through | `TIOCSTI` host shell injection; interleaved output; stdin split non-deterministically | Never hand a container a tty fd we didn't create. |
| 7 | Exit code held only in supervisor memory | supervisor killed → container shows "running" forever; every API call on it hangs | containerd #13849 is exactly this. **Write the exit status to disk before anything else.** |
| 8 | PID reuse: state file says pid N, N is now something else | `mydocker kill` kills an unrelated process | runc's `/proc/<pid>/stat` field 22 start-time check; or a pidfd while the supervisor lives. §6.1. |
| 9 | `create` failure leaves the state dir behind | id permanently unusable; `mydocker run` fails with "exists" | Unwind in reverse; treat state-dir removal as the last step of `delete` and the first step of a failed `create`'s rollback. |
| 10 | SIGTERM to a container that has no handler | `stop` appears to hang | `pid_namespaces(7)` semantics. TERM → timeout → KILL. |
| 11 | `runc create` exit code interpreted as the container's | success reported for a container that failed | `[C]` runc's detached path always returns 0. Our `create` must only report *creation* success. |
| 12 | FIFO opened `O_NONBLOCK` on the read side, then `read()` | spurious `EAGAIN`, "container never started" | `[O]` hit this myself in §3.2. Blocking open + blocking read. |
| 13 | Two concurrent invocations on the same id | both proceed; second corrupts the first's state | runc has a **real TOCTOU here** (§6.6). Use `O_EXCL`/`mkdir` atomicity plus an flock. |

---

# 6. Persistent state

## 6.1 `runc`: `<root>/<id>/state.json`

`[O]` HIGH — real content from `host-a`, `/run/docker/runtime-runc/moby/<id>/state.json` (30,633 bytes total; the `config` blob is ~28 KB of it). Non-`config` fields verbatim:

```json
{
 "id": "7bfda1b6dda911ace6be96035b459a49b970a9810384d9226ed735d7469f0b7c",
 "init_process_pid": 2585529,
 "init_process_start": 1420814509,
 "created": "2026-08-11T23:43:03.038034189Z",
 "rootless": false,
 "cgroup_paths": {
   "": "/sys/fs/cgroup/system.slice/docker-88ecb99d....scope"
 },
 "namespace_paths": {
   "NEWCGROUP": "/proc/2585529/ns/cgroup",
   "NEWIPC":    "/proc/2585529/ns/ipc",
   "NEWNET":    "/proc/2585529/ns/net",
   "NEWNS":     "/proc/2585529/ns/mnt",
   "NEWPID":    "/proc/2585529/ns/pid",
   "NEWTIME":   "/proc/2585529/ns/time",
   "NEWUSER":   "/proc/2585529/ns/user",
   "NEWUTS":    "/proc/2585529/ns/uts"
 },
 "external_descriptors": ["/dev/null", "pipe:[22485389]", "pipe:[22485390]"],
 "intel_rdt_path": ""
}
```
plus a full copy of the resolved `config` (the internal libcontainer config, **not** the OCI `config.json` — it is the post-translation form).

**Why each field exists — this is the honest minimum list:**

| Field | Why it must be persisted |
|---|---|
| `init_process_pid` | the only handle for `kill`, `exec`, `state` |
| `init_process_start` | **PID-reuse defence.** `/proc/<pid>/stat` field 22 (starttime, in clock ticks since boot). `hasInit()` compares it: `if stat.StartTime != c.initProcessStartTime \|\| stat.State == Zombie \|\| stat.State == Dead { return false }`. Without this, a recycled PID makes a dead container look alive and `kill` hits a stranger. `[C]` HIGH `container_linux.go`. |
| `created` | for `runc list` / age reporting; also the tiebreaker for "is this the same container" |
| `namespace_paths` | `runc exec` needs `setns` targets. Note they are all `/proc/<initpid>/ns/*` — **so they are only valid while init lives.** They are a convenience, not durable state. |
| `cgroup_paths` | so `kill`/`delete`/`ps` can find the cgroup without re-deriving it from config |
| `external_descriptors` | so `runc exec` can default a new process's stdio to whatever init's was at create time |
| `rootless` | changes the cleanup path (no cgroup destroy, different mount handling) |
| `config` | so `exec`/`update`/`delete` work without the bundle. **The bundle may be gone.** |

`[C]` HIGH — **written atomically**: temp file in the same dir + `os.Rename`. No `fsync`, no lock file.
```go
tmpFile, err := os.CreateTemp(c.stateDir, "state-")
... WriteJSON(tmpFile, s) ...
return os.Rename(tmpFile.Name(), stateFilePath)
```
`[R]` HIGH — copy this exactly: `O_TMPFILE`-or-`mkstemp` in the same directory, write, `rename(2)`. `rename` within a directory is atomic on Linux. Skipping `fsync` is a deliberate choice (see §6.5 on reboot).

`[C]` HIGH — state dir is `0o711` (`rwx--x--x`): traversable but not listable by others. Combined with the exec FIFO's `0o622`, the container can write the fifo by exact path but cannot enumerate the state dir. That pairing is intentional and worth copying.

`[C]` HIGH — **runc keeps no exit code.** There is no "stopped with status N" state on disk. Once init is gone, `runc state` reports `stopped` and that is all it knows. The exit code lives only in whoever `wait()`ed. **This is the gap that forces the shim to exist**, and it is the single most important thing to fix in our design.

## 6.2 containerd: boltdb

`[C]` HIGH — `/var/lib/containerd/io.containerd.metadata.v1.bolt/meta.db`, a single BoltDB file. `[O]` HIGH — confirmed on `host-a`: it is containerd's fd 3 (`ls -l /proc/912/fd` → `3 -> /var/lib/containerd/io.containerd.metadata.v1.bolt/meta.db`), held open for the daemon's whole life. **BoltDB is single-writer with a file lock — so exactly one containerd can own it.** That is the mechanism that makes containerd a daemon whether it wants to be or not.

`[C]` HIGH — schema (containerd `core/metadata/buckets.go`), abridged to the container path:
```
v1
└── <namespace>            (e.g. "moby")
    ├── labels
    ├── image / <name>     → createdat, updatedat, target{digest,mediatype,size}, labels
    ├── containers / <id>  → createdat, updatedat,
    │                        spec  : <binary>   proto-marshalled OCI spec
    │                        image : <string>
    │                        snapshotter, snapshotKey
    │                        runtime{name, options}
    │                        extensions, labels
    ├── snapshots / <snapshotter> / <key> → name, parent, children, createdat, labels
    ├── content / blob / <digest>  → createdat, size, labels
    │         / ingests / <ref>    → ref, expireat, expected
    ├── sandboxes / <id>
    └── leases / <id>      → snapshots, content, ingests references  (GC roots)
```

`[C]` HIGH — note what is **not** in boltdb: the *task* (the running process). Tasks live in the shim, and their durable trace is the bundle dir under `/run/containerd/io.containerd.runtime.v2.task/<ns>/<id>/` — which contains `address` (how to reconnect), `init.pid`, `options.json`, `runtime`, `shim-binary-path`. `[O]` HIGH, listed in §2.2. So containerd's state is split: **container definitions in boltdb (persistent, survives reboot), task/runtime state in `/run` (tmpfs, does not survive reboot)**. That split is correct and we should copy the *shape*.

`[C]` HIGH — `leases` are containerd's GC roots. Content and snapshots are reference-counted via leases; unreferenced blobs get collected. D3 owns this.

## 6.3 Docker: `/var/lib/docker`

`[O]` HIGH — `sudo ls -la /var/lib/docker` on `host-a`:
```
buildkit/  containers/  engine-id  image/  network/  overlay2/  plugins/
runtimes/  swarm/  tmp/  volumes/
```
`[O]` HIGH — per-container dir `/var/lib/docker/containers/<id>/`:
```
<id>-json.log        27,498,753 bytes   the log
checkpoints/                            CRIU
config.v2.json       5,867 bytes        the container definition + State
hostconfig.json      1,833 bytes        host-side settings (ports, mounts, restart policy)
hostname  hosts  resolv.conf  resolv.conf.hash    generated files bind-mounted in
mounts/                                 mount metadata
```
`[O]` HIGH — `config.v2.json` top-level keys:
```
StreamConfig, State, ID, Created, Managed, Path, Args, Config, Image, ImageManifest,
NetworkSettings, LogPath, Name, Driver, OS, ImagePlatform, RestartCount,
HasBeenStartedBefore, HasBeenManuallyStopped, MountPoints, SecretReferences,
ConfigReferences, MountLabel, ProcessLabel, AppArmorProfile, SeccompProfile,
NoNewPrivileges, WritableCgroups, HostnamePath, HostsPath, ShmPath, ResolvConfPath,
LocalLogCacheMeta
```
`[O]` HIGH — and the `State` sub-object, which is the answer to "what does `docker ps` need":
```json
{"Running": true, "Paused": false, "Restarting": false, "OOMKilled": false,
 "Dead": false, "Pid": 2585529, "ExitCode": 0, "Error": "",
 "StartedAt": "2026-08-11T23:43:02.939418778Z", "FinishedAt": "0001-01-01T00:00:00Z",
 "Health": null}
```
Compare with `docker inspect -f '{{json .State}}'`, which returns the same fields plus a derived `"Status": "running"`. **So Docker's `Status` string is computed from the booleans, and the booleans plus `ExitCode`/`FinishedAt` are the persisted truth.** `RestartCount`, `HasBeenStartedBefore`, `HasBeenManuallyStopped` exist purely for restart-policy logic.

`[R]` HIGH — **The fields Docker persists that runc does not, and that we absolutely need: `ExitCode`, `FinishedAt`, `OOMKilled`, `Error`.** Those four are the difference between "a container manager" and "a container runtime". runc genuinely cannot answer `docker ps -a`; Docker can, because it wrote those down.

## 6.4 What `mydocker ps` actually needs

Enumerate the hard cases first; the field list falls out.

**Case A — everything healthy.** Supervisor alive, container running. Trivial: read the state file, print.

**Case B — container exited, supervisor still alive.** Supervisor `wait()`ed, got the status. It must **write it down** before exiting or before anyone asks. Needed on disk: `status=exited`, `exit_code`, `finished_at`, `oom_killed`.

**Case C — supervisor was `SIGKILL`ed while the container was running.** The container is still alive (it's in its own namespaces and cgroup; nothing kills it). But now: nobody holds its stdio read ends (→ container blocks or gets EPIPE when the pipe fills), nobody will ever reap it (it reparents to PID 1 on death, status lost). `mydocker ps` must report **`running`, unsupervised**. Detection: state file says `running`, `pid` is alive with matching start-time, but the supervisor's pid is dead / the supervisor's socket doesn't answer. This is containerd #13849's neighbourhood and the honest answer is a distinct status, not a lie.

**Case D — supervisor and container both killed.** State file says `running`; `pid` is dead or reused. `mydocker ps` must report **`exited`, code unknown** — and should say `unknown`, not `0`. Cross-check with `cgroup.events` `populated 0` and with the cgroup's `memory.events` `oom_kill` counter to at least say *why*.

**Case E — after reboot.** `/run` is tmpfs and gone. Everything under `/var/lib/mydocker` survives. Every container whose state file says `running` is now definitely not running. `[R]` HIGH — **the reboot marker: record the boot id** (`/proc/sys/kernel/random/boot_id`) in the state file at every transition. On read, if the stored boot id ≠ current boot id and status was `running`, the container is `exited`, code unknown, reason `host rebooted`. This is exact, cheap, and unambiguous — far better than heuristics on `/proc` or on mtimes. `[F]` HIGH — `boot_id` is a per-boot random UUID (`random(4)` / `proc(5)`).

`[R]` HIGH — **Minimum honest state model.** One file per container: `/var/lib/mydocker/containers/<id>/state.json`, plus a copy of the OCI config, plus the log.

```
id                  string
name                string                      (for `ps`, and the name→id index)
created_at          RFC3339
boot_id             uuid                         written at every transition
status              creating|created|running|exited|unknown
                    -- our internal set; maps to the OCI four for `state` output
bundle              abs path
rootfs              abs path                     (resolved root.path)
image_ref           string                       (D3's domain; a string to us)

init_pid            i32                          host pid of container init
init_start_ticks    u64                          /proc/<pid>/stat field 22, PID-reuse guard
supervisor_pid      i32                          so `ps` can tell case C from case A
cgroup_path         abs path
namespace_paths     map<kind, path>               convenience; only valid while init lives
external_fds        [string; 3]                  what 0/1/2 pointed at, for `exec` defaults

started_at          RFC3339 | null
finished_at         RFC3339 | null
exit_code           i32 | null                   null means genuinely unknown
oom_killed          bool
error               string                       last failure reason, human-readable
```
Plus sidecar files in the same directory: `config.json` (the OCI bundle config, copied — the bundle may vanish), `log.json`, `exec.fifo` (during created), `attach.sock` (phase 2), `supervisor.pid`.

`[R]` HIGH — **`exit_code: Option<i32>` where `None` means "we do not know", never `Some(0)` as a default.** Docker's `ExitCode: 0` for a running container is a lie that has confused people for a decade; it works only because `Running: true` overrides it. An `Option` in the type system is free and honest.

`[R]` MED — **Two directories, not one.** `/var/lib/mydocker/containers/<id>/` for state that must survive reboot (definition, exit status, log). `/run/mydocker/<id>/` for runtime-only artifacts (`exec.fifo`, `attach.sock`, `supervisor.pid`). Rationale: mirrors containerd's boltdb-vs-`/run` split, and makes "did we reboot" answerable by the *absence* of `/run/mydocker/<id>/` as a second signal. Tradeoff: two paths to keep in sync, and a `delete` that must clean both. Given the boot_id already answers the reboot question, this is a nice-to-have — `[R]` say: put it all under `/var/lib/mydocker/containers/<id>/` in phase 1, and note that splitting later is cheap because nothing outside the state module knows the paths.

## 6.5 Durability: what to `fsync`, and when

`[R]` HIGH — **`fsync` exactly one thing: the state file after writing a terminal status (`exited`).** Reasoning: for `running` state, durability is worthless — a crash that loses the write is a crash that also means the container is gone, and the boot_id check catches it. For `exited` with an exit code, the write is the *only* record of information that can never be recreated. `fsync` the temp file, then `rename`, then `fsync` the directory. Cost: two fsyncs per container death. Tradeoff: without this, a power loss between a container's death and the state write loses the exit code — the same class of bug as containerd #13849, just with a different cause.

`[R]` HIGH — **Order of operations when the container dies, and it matters:**
1. `waitpid` returns status.
2. Read the cgroup's `memory.events` `oom_kill` count → set `oom_killed`.
3. Drain the stdio pipes to EOF and flush the log. (Do this *before* declaring exited, or you lose the container's last output — a very common bug.)
4. Write state: `status=exited`, `exit_code`, `finished_at`, `oom_killed`. fsync + rename + fsync dir.
5. *Then* tear down: cgroup rmdir, unmount rootfs, remove `/run` artifacts.
6. Exit.

Note step 3 before step 4. If you write "exited" first and then the log flush fails, you have a container reported as cleanly exited whose last 8 KB of output — the panic message — is gone.

## 6.6 Concurrency: two `mydocker` invocations at once

`[C]` HIGH — **runc has a real TOCTOU here and it is instructive.** `libcontainer/factory_linux.go`:
```go
stateDir, err := securejoin.SecureJoin(root, id)
if _, err := os.Stat(stateDir); err == nil {
    return nil, ErrExist
} else if !errors.Is(err, os.ErrNotExist) { return nil, err }
...
if err := os.Mkdir(stateDir, 0o711); err != nil { return nil, err }
```
`Stat` then `Mkdir` — two concurrent `runc create` with the same id can both pass the `Stat` and one gets `EEXIST` from `Mkdir` with a confusing error instead of a clean "already exists". More importantly, **runc's only mutex is `sync.Mutex` inside the `Container` struct — in-process only.** There is **no file lock anywhere.** Two concurrent `runc kill` / `runc delete` / `runc start` on the same container are unsynchronized.

`[C]` HIGH — runc gets away with it because **containerd guarantees single-writer per container**: one shim per container, and the shim serializes its own operations. The safety lives one layer up. `[R]` HIGH — **we do not have that layer, so we must do the locking ourselves.** That is a direct, concrete consequence of choosing "no daemon".

`[C]` HIGH — `validateID` rules (runc): allowed characters are ASCII letters, digits, `_`, `+`, `-`, `.`; must be non-empty; `.` and `..` rejected via lexical normalization. And `securejoin.SecureJoin` is used rather than `filepath.Join` — because the id becomes a path component and a malicious id is a path traversal. `[R]` HIGH — **validate the id against a strict allowlist regex before it ever touches a path.** `^[A-Za-z0-9][A-Za-z0-9_.+-]{0,127}$` plus explicit rejection of `.`/`..`. Do not rely on a safe-join helper as the only defence; reject early.

`[R]` HIGH — **Our concurrency design, minimal and sufficient:**

1. **Creation uniqueness by `mkdir(2)`.** `mkdir` is atomic and fails `EEXIST`. Do *not* `stat` first. `EEXIST` → "container id already exists", full stop. This removes runc's TOCTOU for free by deleting a line of code.
2. **One `flock(LOCK_EX)` per container**, held on a dedicated `<dir>/lock` file (not on `state.json` — you don't want the lock's fate tied to the file you rename). Every mutating operation (`start`, `kill`, `delete`, `update`) takes it. Read-only operations (`ps`, `state`, `logs`) take `LOCK_SH` or no lock at all and tolerate a torn view. `[F]` HIGH — `flock(2)` locks are associated with the open file *description*, released automatically on process death or fd close. **That auto-release on death is the whole reason to use `flock` over a pidfile**: a crashed `mydocker` cannot wedge a container forever.
3. **`ps` needs no global lock.** Read `state.json` per container; each is written atomically by rename, so a reader sees either the old or the new complete file, never a mix. A `ps` that races a transition shows a stale row — acceptable.
4. **The name→id index.** `ps` by name, and "name already in use". `[R]` — a directory of symlinks `/var/lib/mydocker/names/<name> -> ../containers/<id>`, created with `symlink(2)` (atomic, fails `EEXIST`). No index file, no index lock, no index corruption. This is the lazy correct answer; a real database is the alternative and it is not needed until `ps` gets slow, which is at thousands of containers.
5. **No global lock, ever.** A global lock means one slow `mydocker pull` blocks `mydocker ps`. Per-container locks only.

`[R]` MED — What we lose vs. a daemon: (a) no in-memory cache, so `ps` is O(containers) `read`s — fine to ~10k; (b) no cross-container atomicity (can't atomically create two containers) — we don't need it; (c) no event stream (`docker events`) without a daemon or a shared append-only log — genuinely a gap, and the honest answer is "phase 3, and it needs a design".

---

# 7. Architecture recommendation

## 7.1 Do we need each piece? — answered one at a time

| Component | Need it in phase 1? | Reasoning |
|---|---|---|
| **A daemon** | **No.** | Every benefit of dockerd is remote/multi-client/API-versioning. We are single-machine, single-user, learning-oriented. Adding one costs: an HTTP or gRPC surface, a socket permission model, a startup/reconciliation path ("what containers existed when I died?"), and a second place for bugs. Podman and Apptainer both prove a daemonless container manager is viable. |
| **Client/server IPC** | **No** for control; **yes eventually** for `attach`. | Control operations need no IPC if the state is on disk and locked. `attach` genuinely needs to reach a live process that holds the pty — that's one Unix socket per container, owned by that container's supervisor. Not a client/server *architecture*; a per-container endpoint. |
| **A shim (separate binary)** | **No separate binary. Yes to the function.** | The functions (hold stdio, hold pty, reap, record exit) are non-negotiable. A separate *binary* buys runtime-pluggability and language separation, neither of which we want. Make it a subcommand of the same binary: `mydocker supervise <id>`. |
| **A separate runtime process** (runc-equivalent) | **No.** | The whole reason it exists is Go's threading. We're in Rust. Collapsing it is the single biggest simplification available. |
| **Persistent metadata** | **Yes, unavoidably.** | `mydocker ps -a` cannot work without it; exit codes cannot survive without it. §6.4. |
| **One process per container** | **Yes — one supervisor per container.** | The alternative (one supervisor for all containers) reintroduces the daemon's failure mode: one crash loses every container's stdio and exit status. Per-container isolation of failure is worth one process each. (containerd's pod-grouping shim is a Kubernetes-specific optimisation for sandbox-sharing; irrelevant to us.) |
| **A supervisor** | **Yes.** | §5.4 proves it empirically: without a subreaping parent that outlives creation, exit codes go to PID 1 and are gone. |

## 7.2 The recommended architecture — simplest technically sound

**Three processes. One binary. No daemon. No RPC.**

```
  mydocker run -d nginx                          (the CLI — transient, exits)
    │
    │  1. resolve image, prepare rootfs (D3), write bundle + config.json
    │  2. mkdir /var/lib/mydocker/containers/<id>   (atomic uniqueness)
    │  3. flock(<dir>/lock, LOCK_EX)
    │  4. fork/exec  self  "supervise <id>"   with pipes for the handshake
    │  5. read handshake: OK + init_pid   (or an error)
    │  6. print id, release lock, exit 0        ← CLI is NOT in the ancestry
    │
    └─fork→  mydocker supervise <id>             (the SUPERVISOR — lives as long
             │                                     as the container)
             │  setsid()
             │  prctl(PR_SET_CHILD_SUBREAPER, 1)
             │  create cgroup; create pipes; (optional) openpty
             │  block SIGCHLD -> signalfd
             │  clone(CLONE_NEWPID|NEWNS|NEWUTS|NEWIPC|NEWNET[|NEWCGROUP])
             │     [+ CLONE_INTO_CGROUP where available]
             │
             └─child→  container init            (becomes PID 1 in its netns/pidns)
                       │  mount /proc, /sys, /dev; pivot_root  (D1)
                       │  set hostname, sysctls, rlimits
                       │  drop caps, apply seccomp, no_new_privs  (D5)
                       │  reset signal mask to empty        ← §5.6 bug #3
                       │  open(exec.fifo, O_WRONLY)  ← BLOCKS until `start`
                       │  write one byte
                       │  execve(user program)
                       │
             │  supervisor then:
             │    write state.json (status=created), reply OK to CLI, close handshake
             │    poll { signalfd, stdout_pipe, stderr_pipe, attach_sock }
             │    on container exit: read oom_kill count, drain+flush log,
             │                       write state.json (exited, code), fsync,
             │                       teardown cgroup + mounts, exit
```

`mydocker start <id>` = flock, open `exec.fifo` `O_RDONLY`, read one byte, set `status=running`, unlock. **That's it — no IPC with the supervisor at all.** The kernel's FIFO is the entire signalling mechanism. This is the design's best property: the two most timing-sensitive operations (`create`/`start` rendezvous) need zero protocol.

`mydocker kill <id> SIG` = flock, read state, verify pid + start-ticks, `kill(pid, sig)` (or write `cgroup.kill` for `--all`), unlock. No IPC.
`mydocker ps` = readdir + read each `state.json`. No IPC, no lock.
`mydocker logs [-f]` = read the log file, optionally `inotify`. No IPC.
`mydocker attach` = **phase 2**, the only operation that needs the supervisor's socket.

`[R]` HIGH on the shape. `[R]` MED on one detail: whether the supervisor should be the process that calls `clone()` for the container, or whether it should fork an intermediate. Calling `clone()` directly is simpler and correct **as long as the supervisor is single-threaded at that moment**. §8 makes that a hard architectural rule. The intermediate becomes necessary only when we add a user namespace *and* need the supervisor itself in a different namespace set — noted in §7.5 as a shape not to preclude.

## 7.3 Single binary with subcommands vs multiple binaries

`[R]` HIGH — **One static binary with subcommands.**

Reasons, in order of weight:
1. **The supervisor must re-exec something.** Whatever it is, it must be findable. One binary means `/proc/self/exe` — no `PATH` lookup, no version skew, no "runtime not found" failure class. `[C]` HIGH — containerd needs `shim-binary-path` written into every bundle (`[O]` observed on `host-a`) precisely because it has this problem.
2. **Version skew is a real bug class.** `[O]` HIGH — on `host-a` there are three independently-versioned pieces in one chain: Docker 28.3.1, containerd 1.7.27, runc 1.2.5. Every combination is a support matrix. One binary has one version.
3. **Deployment.** `scp mydocker root@vm:/usr/local/bin/` and you're done.
4. Multi-binary buys: independent upgrade (we don't need), independent memory footprint (irrelevant), and the ability to swap the runtime (we don't want — we're writing it).

`[C]` HIGH — **runc itself does this**: `runc init` is not a separate binary, it's `argv[0]="runc" argv[1]="init"` re-executing `/proc/self/exe`. So does Podman (`podman --module`… no: Podman re-execs itself for rootless via `podman-in-userns`). The convention already exists.

`[F]` HIGH — but `/proc/self/exe` re-exec is exactly the **CVE-2019-5736** mechanism: a container that can write to the runtime's binary, or that controls a path the runtime re-execs, can overwrite the host binary. runc's fix was to copy itself into a `memfd` and re-exec *that*. `[R]` HIGH — **if we ever re-exec ourselves into a container context, use the memfd-clone technique, not `/proc/self/exe` directly.** Better: **design so we never re-exec into the container at all** — our container-init code runs in the `clone()`d child *before* pivot_root, in the same address space, with no exec. Rust makes that possible; Go did not. That eliminates the entire CVE class rather than mitigating it. This is the second-biggest Rust win after the threading one.

`[R]` HIGH — **static linking.** `[O]` HIGH on `host-a`, `file /usr/bin/*`: `containerd-shim-runc-v2` is **statically linked**; `runc` and `dockerd` are dynamic PIE. runc being dynamic is a known wart — a dynamically-linked runtime that is bind-mounted into a container (as some setups do) needs the container's loader. For us: `x86_64-unknown-linux-musl` static, or glibc static, so the binary has no dependency on the host's or the container's libc. Tradeoff: glibc NSS (`getpwnam`, DNS via `nsswitch.conf`) does not work in a static glibc build. **We need `getpwnam` to resolve `USER` from the image's `/etc/passwd`** — but we should parse the container's `/etc/passwd` ourselves anyway (it's the *container's* passwd, not the host's), so NSS is not a loss. `[R]` — musl static, parse `/etc/passwd` and `/etc/group` manually.

## 7.4 Should the CLI ever be in the container's process ancestry?

`[R]` HIGH — **No. Never, in any mode, including foreground.**

The reasoning is the runc `docs/terminals.md` argument, and it generalizes: *"any bug in the long-running foreground `runc` process (such as a memory leak) or a stray OOM-kill sweep could result in your container being killed through no fault of the user."* Concretely, if the CLI is the container's parent:
- `Ctrl-C` on the CLI kills or orphans the container.
- Closing the terminal SIGHUPs the CLI, which (as session leader) SIGHUPs the container.
- The CLI's memory footprint is charged to whatever cgroup it's in.
- A CLI crash loses the container's stdio and exit status.
- `mydocker run` over SSH: the SSH session ending takes the container with it.

`[R]` HIGH — **Even foreground `mydocker run` (no `-d`) should work by: fork the supervisor (detached, setsid, subreaper), then the CLI *attaches* to it and proxies stdio and signals.** The container's parent is always the supervisor. The CLI's death then degrades gracefully to "detached container you're no longer watching" instead of "dead container". This is exactly what `docker run` does (the CLI is an API client, never a parent) and it's the right answer for a reason that has nothing to do with having a daemon.

Cost: foreground mode needs the attach path, i.e. it's phase 2. Phase 1 is `-d` only, plus `mydocker logs`. `[R]` — ship that and say so.

## 7.5 What this architecture must NOT preclude — cheap vs expensive later additions

**Cheap later, because the design already accommodates it:**

| Addition | Why it's cheap |
|---|---|
| **`attach` / interactive `-it`** | The supervisor already owns the pty and the pipes. Add a listening Unix socket in its `poll` loop and a multiplexing framing. No structural change. |
| **`exec`** | Read `namespace_paths` + `external_fds` from state (both already persisted for exactly this), `setns`, fork, exec. The supervisor's subreaper flag already covers reaping the exec'd process's orphans. |
| **A daemon, later** | Because *nothing* in the design depends on there being no daemon. A daemon would become a *client* of the same state directory and the same supervisors — exactly what containerd is to its shims. The per-container `flock` is already the mutual-exclusion mechanism a daemon would need. |
| **An HTTP/gRPC API** | Same reason. It's a new front-end over the same state module. |
| **Restart policies** | The supervisor already sees the exit. Add a loop. Needs `RestartCount` in state — add the field now (Docker has it). |
| **`events`** | Append-only `events.jsonl` next to the state dir, written by supervisors. Not free, but not structural. |
| **cgroup v1 support** | Only the cgroup module changes, *if* the cgroup module is behind a trait from the start. |
| **Multiple containers per supervisor** | We'd never want it, but it's a supervisor-internal change. |

**Expensive later, so decide now:**

| Addition | Why it's expensive if not planned |
|---|---|
| **Rootless / user namespaces** | Needs (a) a privileged-outside-the-userns process to write `uid_map`/`gid_map`, (b) newuidmap/newgidmap or CAP_SETUID, (c) cgroup delegation or none at all, (d) different mount strategy. `[R]` **Structure the create path so the "who writes the uid_map" role is an explicit, named participant even in the root-only implementation** — a function `write_id_maps(child_pid)` called by the supervisor. Then rootless is filling it in, not restructuring. If instead the clone-and-setup is one monolithic function, rootless is a rewrite. |
| **Async anywhere in the create path** | See §8. Introducing a threaded runtime around `clone()` is a correctness disaster, and removing it later means restructuring everything it touched. **This is the rewrite-risk decision.** |
| **Making the state model transactional** | If phase 1 writes state ad-hoc from three places, adding real transactions means finding all of them. `[R]` **One module owns all state writes, from day one. Nothing else opens `state.json`.** This is the cheapest expensive-thing-to-defer on the list. |
| **The OCI runtime CLI surface** (`create`/`start`/`state`/`kill`/`delete` with `--root`, `--bundle`, `--console-socket`, `--pid-file`) | If our internal lifecycle doesn't map cleanly onto those five operations, exposing them later means inventing a translation layer. `[R]` **Make the internal API literally those five operations plus `exec`, from day one.** The higher-level `run`/`ps`/`logs` compose them. That is free now and expensive later. |
| **Not being the container's parent** | If we ever want the containerd shape (a manager that talks to supervisors over RPC), the supervisor needs an RPC endpoint. Adding a socket is cheap; *changing who is the container's parent* is not. `[R]` The supervisor is the parent, permanently. Don't design anything that assumes the CLI can be. |

## 7.6 Phasing

`[R]` MED — a defensible order, each phase shippable:

- **Phase 0** — `config.json` parse + `state` + a `create` that only makes namespaces and `execve`s, foreground, no cgroups, no supervisor. Proves the syscall sequence. Throwaway.
- **Phase 1** — supervisor + create/start FIFO split + cgroup v2 + state file + `run -d`, `ps`, `logs`, `kill`, `stop`, `rm`. No attach, no exec, no hooks, no rootless.
- **Phase 2** — `attach`, `-it` with pty + console socket, `exec`, `events`.
- **Phase 3** — OCI CLI compatibility surface (be a drop-in `runc`), hooks (`createRuntime`, `poststop`).
- **Phase 4** — rootless.

---

# 8. Sync vs async internals — and where the boundary goes

## 8.1 The hazard, stated precisely

`[F]` HIGH — `fork(2)` man page: *"The child process is created with a single thread—the one that called fork()."* And: *"After a fork() in a multithreaded program, the child can safely call only async-signal-safe functions (see signal-safety(7)) until such time as it calls execve(2)."* And: *"The entire virtual address space of the parent is replicated in the child, including the states of mutexes, condition variables, and other pthreads objects."*

Unpack what that means operationally. Suppose thread B holds the malloc arena lock at the instant thread A calls `fork()`. The child has the lock's *memory* in the locked state, but not thread B. **Any allocation in the child deadlocks, forever, with no error.** The same applies to: any `Mutex` in a Rust global, `std::io::Stdout`'s lock, a `tracing` subscriber's internal lock, a logger's file buffer, a DNS resolver's cache, a `lazy_static`/`OnceLock` initialization in progress.

`[F]` HIGH — And it is worse than "don't allocate". `signal-safety(7)`'s allowlist is small: it's basically raw syscalls. **`println!` is not on it. `format!` is not. `Vec::push` is not. Returning a `String` error is not.** So the window between `clone()` and `execve()` in the child must be written like signal-handler code.

`[F]` HIGH — Note that `clone(2)` with namespace flags is subject to exactly the same constraint as `fork` — it's the same kernel path, and glibc's `fork()` is itself implemented as `clone(SIGCHLD)`.

## 8.2 Why this is fatal to "just use tokio everywhere"

`[F]` HIGH — `tokio`'s multi-threaded runtime spawns worker threads at construction. Any `fork`/`clone` after `#[tokio::main]` starts is a fork from a multithreaded process. The child then has: N worker-thread stacks it can't use, a locked-or-not-locked allocator, a locked-or-not-locked reactor, and a task scheduler referring to threads that don't exist.

And the container-init child has a *lot* to do before `execve`: mount ~8 filesystems, `pivot_root`, `sethostname`, write sysctls, drop capabilities, load a seccomp filter, set rlimits, open the exec fifo. Every one of those wants to allocate (paths, strings, the seccomp program) and every one wants to report a useful error.

`[R]` HIGH — **The container creation path must run in a process that has never had a second thread.** Not "a single-threaded runtime". Not "we're careful". **Never had a second thread**, because a thread that has exited can still have left a lock held at the moment it was cancelled, and because you cannot audit every transitive dependency for thread spawning.

`[C]` HIGH — this is exactly the constraint that produced `nsexec.c`. Go could not satisfy it, so runc satisfied it by running the code *before the runtime existed*. **We satisfy it by not creating threads.** That is the whole trick.

`[C]` MED — youki's evidence: its workspace `Cargo.toml` lists `tokio = { version = "1", features = ["rt-multi-thread"] }`, but `crates/libcontainer/Cargo.toml` does **not** depend on tokio at all. `[O]` HIGH — verified by fetching both files: libcontainer's dependency list is `caps, chrono, fastrand, libc, nix, oci-spec, procfs, prctl, libcgroups, libseccomp, serde, serde_json, rust-criu, regex, thiserror, tracing, safe-path, nc, netlink-packet-*, netlink-sys, pathrs` — no tokio, no async. So the most mature Rust OCI runtime keeps async entirely out of the container-creation crate. That is strong corroboration.

## 8.3 Where async *is* appropriate

`[R]` HIGH — the boundary, drawn concretely:

| Subsystem | Sync or async | Why |
|---|---|---|
| **Container create/start (clone, mounts, pivot_root, caps, seccomp, execve)** | **Strictly sync, strictly single-threaded, no runtime.** | §8.1/§8.2. Non-negotiable. |
| **The supervisor's event loop** (signalfd, stdio pipes, attach socket) | **Sync `poll`/`epoll`.** | It's 3–5 fds. A hand-rolled `poll` loop is ~80 lines and has zero fork hazard. Introducing tokio here means the supervisor is multithreaded — and the supervisor is the process that calls `clone()`. **Hard no.** |
| **Registry pull: HTTP, TLS, N parallel blob downloads, decompression** | **Async is genuinely the right tool** — or a thread pool. | Real I/O parallelism over the network; 6 layers at once is a 6× wall-clock win. |
| **Layer extraction (tar → overlay dirs)** | CPU + disk bound → **thread pool**, not async. | `rayon` or plain `std::thread`. Async buys nothing for blocking file I/O. |
| **`ps` / `logs` / `state` / `kill` (the CLI's read paths)** | **Sync.** | Reading N small files. Async would be slower and more code. |

`[R]` HIGH — **The rule that makes this safe and is worth writing on the wall: the process that calls `clone()` never has more than one thread, ever, and never links a runtime that could create one.**

`[R]` HIGH — **Mechanism to enforce it: put the pull path in a separate process, not just a separate module.** `mydocker pull` (and the pull step inside `run`) executes as its own process invocation — CLI-side, before any supervisor exists. It may be `#[tokio::main]`, use `reqwest`, spawn 32 threads, whatever. It writes layers to disk and exits. The supervisor process never links any of it. Tradeoff: `mydocker run` becomes two process phases (pull, then create), which we want anyway because pull is cacheable and create is not. Alternative considered and rejected: one binary with `#[tokio::main]` on `pull` only and plain `fn main` on `supervise` — this *works* (tokio only spawns threads when a runtime is constructed) but it is a discipline invariant rather than a structural one, and one careless `lazy_static` with a runtime inside breaks it silently. **Structural beats disciplined.**

`[R]` HIGH — **Is this a rewrite-risk decision? Yes, and it is the biggest one in the project.** If phase 1 puts `#[tokio::main]` on the whole binary "because we'll need it for pulls later", then the supervisor is multithreaded, the `clone()` child inherits a poisoned address space, and the bugs are **nondeterministic deadlocks in the container's init path that reproduce once in a thousand runs.** That is the worst possible failure signature — it will be blamed on the kernel, on the image, on the mounts, for weeks. Unwinding it later means re-plumbing everything the runtime touched. **Decide sync-core now and never revisit it.**

`[R]` MED — Sub-decision: `tracing` in the supervisor. `tracing`'s subscriber holds locks and allocates. If the supervisor emits a `tracing` event and *then* forks, the child may inherit a held lock. `[R]` — **the `clone()`d child must not log through `tracing` at all.** Give it a dedicated raw fd (a pipe to the supervisor) and `write(2)` pre-formatted bytes into it. `[C]` HIGH — this is exactly runc's `_LIBCONTAINER_LOGPIPE` design, and `standard_init_linux.go` explicitly closes the log pipe before the exec fifo dance. Also exactly conmon's and crun's approach. Everybody who has built this has converged on "the child gets a raw fd, not a logger". Confidence MED only on whether `tracing` specifically deadlocks post-fork (it depends on the subscriber); confidence HIGH that "raw fd for the child" is the right design regardless. What would settle the `tracing` question: reading `tracing-subscriber`'s `fmt` layer for its `Mutex`/`RwLock` usage.

---

# 9. Error handling and failure atomicity

## 9.1 The problem, concretely

Container creation is a sequence of ~15 privileged operations, each of which can fail, and most of which mutate global kernel state. A failure at step 12 leaves steps 1–11 done. Enumerate what exists at each point:

```
 1. mkdir state dir                        -> a directory
 2. write state.json (creating)            -> a file
 3. mkdir cgroup, write limits             -> a cgroup with controllers enabled
 4. mkfifo exec.fifo                       -> a fifo
 5. create stdio pipes / openpty           -> fds, a pty pair
 6. mount overlay rootfs (D3)              -> a MOUNT on the host
 7. clone(NEWNS|NEWPID|NEWNET|...)         -> namespaces + a live process
 8. move veth into netns (D4)              -> a netns interface, an iptables rule,
                                              an allocated IP from a pool
 9. child: mount /proc /sys /dev /dev/pts  -> mounts INSIDE the container's mnt ns
10. child: pivot_root + umount old root
11. child: apply seccomp / caps / rlimits
12. child: open exec.fifo (blocks)
13. parent: write state.json (created)
```

**What leaks if you get it wrong, ranked by how bad:**

| Leak | Severity | Why |
|---|---|---|
| **The mounted rootfs (step 6)** | **Worst.** | A leaked overlay mount pins the lower/upper dirs. `mydocker rm` deletes the directories, the mount survives, disk is never reclaimed, and `df` disagrees with `du` until reboot. Silent, cumulative, and the #1 real-world container-runtime disk bug. |
| **The cgroup (step 3)** | Bad. | An orphan cgroup dir cannot be removed while any process is in it. If the container init survived, the cgroup pins it. Accumulating empty cgroups is mostly cosmetic but they cost kernel memory and slow cgroup walks. |
| **The container init process (step 7)** | Bad. | A live process holding namespaces that nothing will ever reap or kill. It shows in `ps` as an orphan with a container's argv. Its namespaces stay alive as long as it does. |
| **The network resources (step 8)** | Bad. | A leaked IP from the pool is a slow exhaustion. A leaked iptables/nft rule pointing at a dead IP is a latent misroute. A leaked veth in the host netns is cosmetic but confusing. (D4's domain; noting the atomicity requirement.) |
| **The state directory (step 1)** | Annoying. | The id becomes permanently unusable — every retry says "already exists". |
| **The fifo / pipes / pty (steps 4,5)** | Benign. | Closed by process exit; the fifo inode goes with the state dir. |
| **Namespaces without a process** | Subtle. | A namespace persists if *anything* holds a reference: a process, a bind-mount of `/proc/<pid>/ns/X`, or an open fd. `[F]` HIGH `namespaces(7)`. If we bind-mount a netns to make it persistent (D4 likely will, as Docker does — `[O]` `/run/docker/netns/*` observed on `host-a`), that bind-mount must be in the rollback list or the netns leaks forever. |

## 9.2 What mature runtimes actually do

`[C]` HIGH — **runc's model is defer-based reverse unwind, in Go.** From `process_linux.go` and `container_linux.go`:
- Each acquisition is immediately followed by a `defer func(){ if retErr != nil { <release> } }()`.
- The named-return-error pattern (`func ... (retErr error)`) is what makes the defer able to see whether the function is failing.
- `createExecFifo()` → `defer func(){ if retErr != nil { c.deleteExecFifo() } }()`.
- `initProcess.start()` failure → `manager.Destroy()` (cgroups), `intelRdtManager.Destroy()`, `terminate()` (SIGKILL + wait).
- `runner.run()` → `if !detach || retErr != nil { r.destroy() }`, gated by `--keep` so an operator can inspect a failed container.

`[C]` HIGH — **runc gives the operator an escape hatch (`--keep` / `shouldDestroy`).** Deliberate: sometimes you need the corpse. `[R]` MED — we should have this too (`--keep-failed`), but not in phase 1.

`[C]` HIGH — **runc annotates the error with the cgroup's OOM count** before terminating, turning "creation failed" into "container init was OOM-killed". `[R]` HIGH — copy.

`[C]` HIGH — **The child cannot roll back its own work and doesn't try.** Steps 9–11 happen inside the container's mount namespace. If step 10 fails, the child dies; **the mount namespace is destroyed with it and every mount inside it disappears automatically.** `[F]` HIGH — `mount_namespaces(7)`: when the last process in a mount namespace exits and nothing holds a reference, the namespace and all its mounts are torn down. **This is the single most useful atomicity property in the whole system: everything the child mounts *after* `unshare(CLONE_NEWNS)` is free to roll back — just let the child die.** The mounts that need explicit rollback are exactly the ones made *before* the clone, in the host's namespace (the overlay at step 6). `[R]` HIGH — **exploit this deliberately: do as little as possible in the host mount namespace, and everything else inside the new one.** If D3 can arrange for the overlay mount to happen inside the container's mount namespace rather than on the host, the worst leak on the list disappears entirely. Flag to D3 as a design question.

## 9.3 The error model this implies for Rust

`[R]` HIGH — **Three distinct mechanisms, and conflating them is the mistake.**

**(1) RAII guards for release, not `Drop` for logic.**
Each acquired resource returns a guard whose `Drop` releases it, plus an explicit `.commit()`/`into_inner()` that disarms the guard on success. This is the `scopeguard::ScopeGuard`/`defer` pattern, and it maps one-to-one onto runc's defers — but the compiler enforces it, and it is exception-safe against `?` early return, which is exactly where hand-written cleanup gets forgotten. `[C]` MED — youki lists `scopeguard` in dev-dependencies only, which suggests it hand-rolls guards in the main path; that's a data point, not a recommendation.

Tradeoff to state honestly: **`Drop` cannot return an error and cannot be `async`.** So a `Drop` that unmounts must log-and-continue on failure. That's acceptable for cleanup, unacceptable for the happy path. Hence: **`Drop` only for the failure path; the success path tears down explicitly with real error handling.** Never write a resource type whose only teardown is `Drop` if teardown can meaningfully fail.

**(2) A typed error enum per phase, with the phase in the type.**
Not `anyhow::Error` in the core. The reason is specific and not stylistic: **the caller must be able to decide what to roll back based on the error, and "how far did we get" is exactly what a typed error can carry and a string cannot.** E.g. `CreateError::Clone{source}` means no namespaces exist; `CreateError::ChildSetup{stage, source}` means a child exists and must be killed and reaped. If the error is a string, the rollback has to be conservative (try everything, ignore failures), and "try everything and ignore failures" is how you get code that reports success while leaking mounts.

`[R]` HIGH — shape: `thiserror`-derived enums in the core library, one per phase, each carrying (a) the operation, (b) the OS errno, (c) enough context to name the resource (path, pid, cgroup). `anyhow` (or nothing) at the CLI boundary where the only job left is printing.

**(3) An explicit, ordered rollback list, not just guards.**
Guards handle the in-function case. But creation spans processes, and a guard in the supervisor cannot un-mount something the child mounted. `[R]` HIGH — the supervisor keeps a `Vec<Cleanup>` in creation order and unwinds it in reverse on failure, where `Cleanup` is a small enum (`Rmdir(path)`, `Umount(path)`, `RemoveCgroup(path)`, `Kill(pid, start_ticks)`, `ReleaseIp(addr)`, `RemoveStateDir(id)`). Two reasons this beats guards alone: (a) it is *inspectable* — you can log the exact rollback plan, which turns a mystery leak into a diagnosable one; (b) it can be **persisted**, which is the only way to clean up after the supervisor itself is killed mid-create.

`[R]` MED — that last point deserves emphasis: **write the cleanup list to disk (`<dir>/rollback.json`) as you acquire resources.** Then `mydocker rm --force` on a container whose supervisor died mid-create knows exactly what to undo. This is the difference between "leaks recoverable by a command" and "leaks recoverable by a reboot". Cost: one small file write per acquisition step (~8 writes). Confidence MED that this is worth phase-1 complexity — the cheaper phase-1 answer is "on `rm`, best-effort try every possible cleanup and report what failed", which is 80% as good for 20% of the work.

**(4) The child's error channel.**
`[F]` HIGH — the child cannot return a `Result` to the parent. It must serialize. `[C]` HIGH — runc uses a socketpair with a sync protocol (`procError` is carried over it) plus a separate log pipe. `[R]` HIGH — same for us: a `socketpair` where the child writes a **fixed-size, pre-formatted** error record before `_exit(1)`. Fixed-size and pre-formatted matters: after `clone()`, before `execve()`, the child must not allocate (§8.1). Practical shape: a small `#[repr(C)]` struct with a phase discriminant, an errno, and a fixed `[u8; 128]` context buffer written with a single `write(2)`. Ugly, correct, and it is the only design that reports a real error from a post-fork failure instead of "child exited 1".

`[R]` HIGH — and the parent's fallback: if the child dies with no error record, the parent must still produce something better than "exit 1". Check, in order: the cgroup's `memory.events` `oom_kill` counter (→ "OOM-killed during setup"), `WIFSIGNALED` (→ "killed by SIG*"), the child's log pipe contents. `[C]` runc does exactly the first of these.

## 9.4 The delete path is the same problem, deferred

`[F]` HIGH — the spec: "Deleting a container MUST delete the resources that were created during the `create` step", and `delete` "MUST generate an error" if the container isn't `stopped`.

`[R]` HIGH — **`delete` and `create`'s rollback should be the same code.** They undo the same list. If they're separate implementations, they will diverge, and the divergence will be a leak. Concretely: `create` failure = "run the rollback list from where we got to"; `delete` = "run the rollback list from the end". Same function, different starting index.

`[R]` HIGH — **`delete --force` must be able to succeed against arbitrary corruption**, including: state.json unparseable, pid dead, pid reused, cgroup already gone, mount already gone, supervisor gone. Every step best-effort, collect all errors, report them all, and **remove the state dir last** — so a partial failure leaves something retryable, and a success leaves nothing. The failure mode to avoid: `delete` bailing on the first `ENOENT` and leaving the container permanently undeleteable, which forces users to `rm -rf` our state directory by hand — at which point they leak the mount.

---

*(Sections 10 and 11 — Rust substrate detail and the architecture-family comparison including Apptainer — are integrated below from parallel research.)*

---

# 10. runc vs crun vs youki — process structure, side by side

This is the direct answer to "which of runc's design constraints are Go artifacts?" — settled by reading what the C implementation and the Rust implementation actually do.

## 10.1 crun (C, no runtime threads)

`[C]` HIGH — **crun never re-execs itself.** Verified by grepping `src/libcrun/container.c` (4703 lines, fetched from `raw.githubusercontent.com/containers/crun/main`) for `proc/self/exe`, `execv`, `memfd`, `reexec`: the only `execv` calls are line 1424 (`execv (exec_path, def->process->args)` — the *user's* program) and line 3545 (the same for `crun exec`). **There is no self re-exec, no `/proc/self/exe`, no memfd-clone, no C constructor trick.** The container setup code runs in the cloned child's own address space and then `execv`s the user program directly.

`[R]` HIGH — **This is the decisive evidence.** runc's `runc init` re-exec, `nsexec.c`, the `__attribute__((constructor))`, the netlink TLV bootstrap and the `setjmp`/`longjmp` staging are **100% Go artifacts.** A language without a mandatory multithreaded runtime does not need any of them. crun proves it in production (it is the default runtime on RHEL/Fedora via CRI-O and Podman).

`[C]` HIGH — **crun keeps the create/start split, and keeps an exec FIFO — but inverts the direction relative to runc.**
- `libcrun_container_create()` (line 2936) calls `libcrun_status_create_exec_fifo()` (line 2965) and stores the resulting fd in `context->fifo_exec_wait_fd`. The **child** ends up holding the fifo's **read** end.
- The child, in `container_init()` after "sync 4", blocks on `select()` then `read()` on that fd (lines 1285–1306), with the comment-adjacent error string `"read from the exec fifo"`.
- `libcrun_container_start()` (line 3045) calls `libcrun_status_write_exec_fifo()` (line 3076) — the **parent writes**.
- The created-vs-running discriminator is `libcrun_status_has_read_exec_fifo()` (lines 1558, 3159).

`[R]` MED — crun's direction (child blocks in `read()` on an already-open fd) is arguably better than runc's (child blocks in `open(O_WRONLY)`) for one reason: the fd is already open, so the child's blocking point does not depend on FIFO *open* semantics, and the child can `select()` on it alongside other fds. runc's direction has one advantage: the parent doesn't need to keep an fd open across the create/start gap, because `open()`-blocking *is* the rendezvous. `[R]` — **either is fine; pick runc's (`open(O_WRONLY)` in child, `open(O_RDONLY)`+`read` in `start`) because it needs no fd to survive the gap**, which matters if our `start` is a fresh process invocation. Confidence MED because crun's design would also work if the fd is inherited by the supervisor, which it is in our architecture.

`[C]` HIGH — **crun's child→parent error channel is a fixed-size struct over a socketpair.** `src/libcrun/container.c` lines 71–102:
```c
enum { SYNC_SOCKET_SYNC_MESSAGE, SYNC_SOCKET_ERROR_MESSAGE,
       SYNC_SOCKET_WARNING_MESSAGE, SYNC_SOCKET_DEBUG_MESSAGE };

struct sync_socket_message_s {
  int  type;
  int  error_value;
  char message[512];
};
```
written with a single `write()` via `sync_socket_write_msg()` / `sync_socket_write_error()`, and crun *redirects its entire logging output handler* into that socket while in the child: `crun_set_output_handler (log_write_to_sync_socket, args)` (line 1246), switching back to stderr only after the fifo wait: `crun_set_output_handler (log_write_to_stderr, NULL)` (line ~1308).

`[R]` HIGH — **This is exactly the design recommended in §9.3(4), and crun's independent arrival at it is strong corroboration.** Fixed-size struct, no allocation, one `write`, a type discriminant, an errno, and a bounded message buffer. Adopt this shape verbatim (`#[repr(C)] struct { kind: u32, errno: i32, msg: [u8; 512] }`).

`[C]` HIGH — one more crun detail worth stealing: `container_init()` **reads its own host pid from the sync socket** as its first act (line 1249: `read (sync_socket, &own_pid, sizeof (own_pid))`). It has to, because once inside the PID namespace `getpid()` returns 1 and `/proc/self` reports the namespace-local pid — the child cannot discover its own host pid. `[R]` HIGH — we need the same: if the child ever needs its host pid (for cgroup writes, for error messages, for `/proc/<hostpid>/...` paths the parent will use), the parent must tell it.

`[C]` HIGH — crun `create` and `run` do `fork()` (lines 2872, 2990) to detach, and the namespace `clone` happens in `libcrun_run_linux_container()` in `src/libcrun/linux.c`. So crun's process count for a detached create is: `crun` → forked detached `crun` → cloned container init → `execv`. **Three, versus runc's five.** The two it drops are exactly the two Go forced.

## 10.2 youki (Rust) — did it mirror runc or simplify?

`[C]` HIGH — **youki mirrors runc's three-process shape but implements it entirely in Rust with no re-exec.** `crates/libcontainer/src/process/container_main_process.rs`: three processes, **main → intermediate → init**, with the intermediate created by
```rust
let container_clone_fn = if container_args.as_sibling {
    fork::container_clone_sibling
} else {
    fork::container_clone
};
```
and the callback closure running `container_intermediate_process`. The intermediate then clones the init process and reports the init pid back: *"The intermediate process will send the init pid once it forks the init process"*, received via `intermediate_main_receiver.wait_for_intermediate_ready()`. Main then reaps the intermediate with `waitpid`.

`[C]` HIGH — **four unidirectional channels**, not runc's netlink-TLV-plus-socketpair:
```
intermediate_main : intermediate -> main   (identifier_mapping_request, ready)
init_main         : init         -> main   (hook / SetupNetworkDeviceReady / SeccompNotify)
inter_chan        : main -> intermediate   (mapping_written, acks)
init_chan         : main -> init           (acks)
```
Message names observed: `identifier_mapping_request`, `mapping_written`, `SetupNetworkDeviceReady`, `SeccompNotify`, `InitReady`. Same *protocol shape* as runc's `SYNC_USERMAP_PLS`/`ACK` and `procReady`/`procRun` — because the underlying kernel constraints are the same — but expressed as typed Rust channels over socketpairs instead of a hand-rolled TLV.

`[C]` HIGH — **`crates/libcontainer/src/process/fork.rs`: `clone3` first, `libc::clone` as fallback.** Quoted comments:
- *"For now, we can only use clone3 as a kernel syscall. Libc wrapper is not available yet."* — so youki issues the raw syscall.
- Fallback stack: *"we allocate through mmap to the system default limit, which is 8MB on most of the linux system today. This is OK since mmap will only reserve the address space upfront, instead of allocating physical memory upfront."*
- Guard page: *"we create a guard page of 1 page, to protect the child stack collision. Note, for clone call, the child stack will grow downward, so the bottom of the child stack is in the beginning."*
- **Why not `nix`:** *"The nix wrapper also does not treat the closure memory correctly. The wrapper implementation fails to pass the right ownership to the new child process."* — youki uses a "double boxing" trick and re-boxes the raw pointer in the child so the child owns the heap memory.
- `CLONE_PARENT` gotcha: *"when using `CLONE_PARENT`, the `clone3` will return EINVAL if an exit signal is set."*

`[R]` HIGH — **Three concrete takeaways from that file, and they are the most actionable findings in this whole document:**
1. **Do not use `nix::sched::clone`.** The most mature Rust OCI runtime explicitly does not, and states why: the closure's memory ownership is not correctly transferred to the child. This is a real soundness problem, not a style preference. Use `clone3` via raw syscall, with a `libc::clone` fallback.
2. **`clone3` is the right primary path** — no caller-allocated stack, no closure-pointer marshalling, and it supports `CLONE_INTO_CGROUP`. `[O]` HIGH — `clone3` present on `host-a` (kernel 6.11): `syscall(435, 0, 0)` → `-1 EINVAL`, not `ENOSYS`. It is **absent on `old-host`** (kernel 4.18 predates clone3, which landed in 5.3) — so the fallback is not theoretical if we ever care about EL8-era kernels. Our stated dev target is a modern kernel, so `clone3` primary is right, but keep the fallback shape in mind.
3. **The `libc::clone` fallback needs an mmap'd stack with a guard page**, and getting that wrong is a stack-collision corruption bug that will present as random crashes in the child. This is the single scariest piece of unsafe code in the project. `[R]` — if we can require kernel ≥ 5.3 and use only `clone3`, **we delete this entire hazard.** Given our target is "a local destroyable Linux VM with a modern kernel", **require ≥ 5.7** (for `CLONE_INTO_CGROUP` too) and drop the `libc::clone` path. State the requirement, don't write the fallback. That is a real, defensible simplification.

`[C]` HIGH — **youki does not re-exec itself either.** No `/proc/self/exe`, no `runc init` equivalent. The setup runs in the cloned child directly. So: **the Rust simplification is real and shipped.**

`[C]` HIGH — **youki keeps the intermediate process even though Rust does not force it.** Why? Two legitimate reasons: (a) the userns map-writing dance needs a process that unshares the userns while a privileged process outside writes its maps — that's the intermediate; (b) `CLONE_NEWPID` needs a fork, and youki's `as_sibling` / `CLONE_PARENT` handling wants the init's parent to be main, not the intermediate. So youki's three processes are **kernel-imposed, not language-imposed** — which matches my §3.4 table exactly.

## 10.3 The synthesis table

| Concern | runc | crun | youki | Ours (recommended) |
|---|---|---|---|---|
| Language | Go + C | C | Rust | Rust |
| Self re-exec | **Yes** (`runc init`) | **No** | **No** | **No** |
| C constructor / pre-runtime hook | **Yes** (`nsexec.c`) | n/a | **No** | **No** |
| Processes, detached create | 5 (incl. shim) | 3 | 3 | **3** (CLI exits; supervisor + init persist) |
| Namespace clone mechanism | `clone(CLONE_PARENT)` ×2 in C | `clone` in `linux.c` | `clone3`, fallback `libc::clone`+mmap stack | **`clone3` only**, require kernel ≥ 5.7 |
| Create/start rendezvous | exec fifo; child blocks in `open(O_WRONLY)` | exec fifo; child blocks in `read()` | **unix socket** `notify.sock`; init blocks in `accept()` | **exec fifo** (see note below) |
| created/running discriminator | fifo file exists (`os.Stat`) | `has_read_exec_fifo` | — | **explicit `status` field** |
| Child→parent bootstrap encoding | netlink TLV (C↔Go) | direct struct on socketpair | typed Rust channels | **typed struct on socketpair** |
| Child→parent error channel | sync socket + separate log pipe | `struct sync_socket_message_s {type, error_value, char[512]}`, one `write()` | typed channel | **fixed-size `#[repr(C)]` struct, one `write()`** |
| Supervisor | external (shim/conmon) | external (conmon) | external (conmon/shim) | **built in (`mydocker supervise`)** |
| Exit code persisted by the runtime | **No** | **No** | **No** | **Yes — this is our main addition** |
| In-process locking only, no file lock | Yes (relies on the layer above) | Yes | Yes | **No — we need real `flock`** |

`[R]` HIGH — the last two rows are the two places where we should deliberately *differ from all three*, and they are both consequences of having no layer above us. Everything else, we should converge on what crun and youki already do.

### Note: exec FIFO vs `notify.sock` for the create/start rendezvous

`[C]` HIGH — **youki uses a unix `SOCK_STREAM` socket, not a FIFO.** `crates/libcontainer/src/notify_socket.rs:10 pub const NOTIFY_FILE: &str = "notify.sock"`, and `NotifyListener::new()` is called in `container/builder_impl.rs:130` **before** pivot_root/userns entry — *"Need to create the notify socket before we pivot root, since the unix domain socket used here is outside of the rootfs"*. Init blocks in `process/init/process.rs:448 ctx.notify_listener.wait_for_container_start()`, i.e. in `accept()`, immediately before `execve`.

`[R]` MED — **arguments for the socket:** `accept()` gives you `SO_PEERCRED`, so init learns *who* started it; you avoid FIFO open-blocking semantics (which I got wrong once in §3.2); and it composes with `poll` alongside other fds. **Arguments for the FIFO:** no `sun_path` 108-byte limit (§11.1 item 9 — and our state paths are long), one inode instead of a socket file, mode `0622` lets an unprivileged container write it with no socket permission subtleties, and `start` needs nothing but `open`+`read`. **Verdict `[R]` MED: use the FIFO.** The `sun_path` limit is a real, path-length-dependent trap that we would hit with 64-hex ids, and we do not need peer credentials for a rendezvous where the only legitimate peer is a process holding our per-container `flock`. Revisit if we ever want the start-side to be authenticated.

---

# 11. Architecture families — trust boundary, process boundary, state, purpose

Four families, not nine projects. The distinction that matters is **where the supervisor is relative to the client**.

| Family | Trust boundary | Supervisor | State of record | Built to solve |
|---|---|---|---|---|
| **Daemon + shim** (Docker, containerd) | the daemon's socket — root-equivalent API | per-container/pod shim, ttrpc-addressable | boltdb `meta.db` + content store + `/run` bundles | many clients, one machine-wide authority, Kubernetes |
| **Daemonless + shim** (Podman, CRI-O) | the calling user's uid; no privileged API by default | `conmon`, one per container *and* one per exec | Podman: sqlite `db.sql`; CRI-O: per-container `state.json` | delete the root daemon as SPOF and attack surface |
| **Sandboxed kernel** (gVisor, Kata, Firecracker) | the Sentry / the VM boundary — not the host syscall table | Sentry, or VMM + guest agent | slots into the family above (shim v2) | the host kernel's ~350-syscall surface is too large to trust |
| **Exec-a-process** (Apptainer/Singularity) | the user's uid, unchanged; the kernel enforces it | **none** — the container *is* your process | **nothing**, by design (except instances) | run software as yourself on a shared FS you don't administer |

## 11.1 Podman + conmon — the daemonless supervisor, operationally

`[C]` HIGH — the spawn sequence (`libpod/oci_conmon_common.go`, `libpod/oci_conmon_linux.go`, `conmon/src/conmon.c`):
1. libpod writes the DB record, lays out the bundle, builds conmon argv: `--api-version 1 -c <id> -u <cuuid> -r /usr/bin/crun -b <bundle> -p <pidfile> --exit-dir <dir> --persist-dir <dir> --full-attach -l k8s-file:<path> --exit-command <podman> --exit-command-arg container --exit-command-arg cleanup`.
2. `exec.Command(conmon, ...)` with `SysProcAttr{Setpgid: true}`; `ExtraFiles = [reserved port fds…, childSyncPipe, childStartPipe]`; env `_OCI_SYNCPIPE`, `_OCI_STARTPIPE`. Both are **`socketpair(AF_LOCAL, SOCK_SEQPACKET|SOCK_CLOEXEC)`**, not `pipe(2)` — message boundaries survive.
3. conmon writes `-1000` to `/proc/self/oom_score_adj`, then **blocks reading the start pipe**: *"Block for an initial write to the start pipe before spawning any children or exiting, to ensure the parent can put us in the right cgroup."* (`conmon.c:66`)
4. Podman moves conmon into `libpod-conmon-<id>.scope` (systemd) or `<parent>/conmon` (cgroupfs), **then** writes the start byte. `[R]` HIGH — this handshake is the clean solution to "the supervisor must be in a known cgroup before it forks anything", and it is strictly better than our alternative of the supervisor placing itself, because the *caller* knows the cgroup policy. Worth copying if we ever put the supervisor in a cgroup of its own.
5. **Double fork + setsid**: conmon forks; the parent writes conmon's pidfile and `_exit(0)`; podman reaps it. The survivor `dup`s `/dev/null` over 0/1/2 and calls `setsid()`. `[C]` HIGH — this is why podman exiting doesn't kill the container. Contrast §2.1's `[O]`: containerd's shim does fork-and-exit + `setpgid` but *not* `setsid`. Both work; conmon's is stricter.
6. `PR_SET_CHILD_SUBREAPER` (`conmon.c:138`).
7. I/O: with `-t`, a console-socket listener at `$TMPDIR/conmon-term.XXXXXX`; without, `pipe2(O_CLOEXEC)`. **A stderr pipe is always created** — *"because that way we can capture runc stderr messages before the tty is created."* (`conmon.c:180`) `[R]` HIGH — steal this. The single hardest thing to debug in a container runtime is a runtime that failed before it had anywhere to write.
8. `setup_attach_socket()` binds `<bundle>/attach` as `SOCK_SEQPACKET|SOCK_NONBLOCK|SOCK_CLOEXEC`, mode `0700`. `setup_terminal_control_fifo()` `mkfifo(<bundle>/ctl, 0660)` and **opens both ends** — *"to prevent getting flood of POLLHUPs when last writer closes"*. `[C]` HIGH — independent confirmation of the both-ends rule I derived from the shim's fd table in §5.3.
9. `[F]` HIGH — `sockaddr_un.sun_path` is **108 bytes**. conmon works around it by opening the parent dir `O_PATH` and binding relative to it via `/proc/self/fd` (`conn_sock.c:bind_unix_socket`), with a symlink-shortening fallback. `[R]` HIGH — **this will bite us**: `/var/lib/mydocker/containers/<64-hex-id>/attach.sock` is 105 bytes before the prefix. Use the `O_PATH` + `/proc/self/fd/N` bind trick, or put sockets under a short `/run/mydocker/<12-char-id>/` path. Cheap to design in, painful to retrofit.
10. conmon forks again into the "create child": `set_pdeathsig(SIGKILL)`, `dup2` fds onto 0/1/2, `reset_oom_adjust()` (*"We don't want runc to be unkillable"*), `execv(crun create ...)`. The stated reason: *"We have to fork here because the current runC API dups the stdio of the calling process over the container's fds. This is actually \*very bad\*."* (`conmon.c:208`)
11. `terminal_accept_cb` (`ctrl.c:24`) `recvfd()`s the pty master over `SCM_RIGHTS`, sets `c_oflag |= ONLCR`, `mainfd_stdin = console.fd; mainfd_stdout = dup(console.fd)`, unlinks the socket path, closes the listener.
12. On exit: `check_cgroup2_oom()` → `drain_stdio()` → `sync_logs()` → `close_other_fds()` (*"Podman injects some fd's into the conmon process so that exposed ports are kept busy while the container runs. Close them before we notify the container exited, so that they can be reused immediately."*) → write status to `<persist-dir>/exit` and `<exit-dir>/<cid>` (*"helps if a daemon process wants to monitor all container exits using inotify"*) → `atexit` fires `podman container cleanup <id>`.

**Final ancestry:** `PID 1 → conmon → container init`. Podman's process is gone.

`[C]` HIGH — **what conmon holds, and the failure mode if nothing does** — the definitive list:

| conmon holds | If nothing held it |
|---|---|
| the **pty master** (via `SCM_RIGHTS`) | last ref closes when `crun create` exits → slave side gets `SIGHUP`/`EIO`; `-t` impossible |
| the **read ends of stdout/stderr pipes** | container's first `printf` → `EPIPE` + `SIGPIPE` → dead app |
| **subreaper + `waitpid`** | init reparents to PID 1; systemd `wait()`s and discards; `$?` is gone (§5.4 proves this) |
| the **`exit` / `oom` files** | nothing durably records "137, OOM-killed" for `podman ps -a` |
| the **attach socket + `ctl`/`winsz` FIFOs** | no attach, no `logs -f`, no resize after the CLI exits |
| the **log writer** | output goes to `/dev/null` |
| **leaked bound port fds** | the published host port frees the instant `crun create` exits; another process steals it while the container runs |
| the **`--exit-command`** | netns, mounts, DB state leak on every exit |

`[C]` HIGH — **`podman ps` = read sqlite `db.sql` (`libpod/sqlite_state.go:34`, in `<GraphRoot>/libpod`) → reconcile each container against `crun state` and the exit files.** No daemon consulted. Two concurrent podmans coordinate purely through file locks in `TmpDir/locks`. `[C]` HIGH — **reboot detection: an `alive` file in `Engine.TmpDir`** (`runtime.go:581`); if absent, `Refresh()` resets all container states because *"it is safe to assume there are no containers running"*. `[R]` MED — that's the same problem my `boot_id` recommendation (§6.4 case E) solves, and `boot_id` is strictly better: an `alive` file in a tmpfs is a proxy for "did tmpfs get wiped", whereas `boot_id` answers the actual question and also works if state lives on persistent disk.

`[C]` HIGH — **attach framing:** conmon multiplexes with a **1-byte stream prefix** (`AttachPipeStdin=1, Stdout=2, Stderr=3`, source comment *"Sync with stdpipe_t in conmon.c"*). Resize goes the other way: podman writes `"<type> <h> <w>\n"` into the `ctl` FIFO; conmon forwards to `winsz` and `ioctl(TIOCSWINSZ)`. Type `2` = `REOPEN_LOGS_EVENT` (logrotate support). `[R]` HIGH — a 1-byte prefix on a `SOCK_SEQPACKET` socket is simpler than Docker's 8-byte length-prefixed frame (§2.3) *because SEQPACKET preserves message boundaries so you don't need the length*. Use SEQPACKET + 1-byte tag.

`[C]` HIGH — `podman system service` adds an HTTP API (Docker-compat + native libpod) on a socket, systemd-socket-activatable, and **exits after `--time` idle**. It does **not** become the container's parent. `[R]` HIGH — that is the model for us if we ever want an API: **a protocol adapter, not a supervisor.**

`[C]` HIGH — **Is conmon the same thing as `containerd-shim-runc-v2`?** Same role, different contract:

| | conmon | containerd-shim-runc-v2 |
|---|---|---|
| Control plane | argv + start pipe + sync pipe + attach socket + `ctl` FIFO | full **ttrpc `Task` service**; address on stdout from `shim start` |
| Cardinality | strictly 1 per container *and* 1 per exec | 1:1 **or 1:N** — groups by `io.kubernetes.cri.sandbox-id` |
| Exit reporting | write files + pipe + fork `--exit-command` | typed async events; **spec-mandated ordering** `TaskStart` before `TaskExit` |
| Crash recovery | podman reconciles from files on the next CLI run | containerd invokes the shim's **`delete` subcommand** (cwd = bundle) |
| Substitutability | always exec's an OCI runtime binary | **the shim can BE the runtime** — `containerd-shim-kata-v2` never calls runc |

`[R]` HIGH — **conmon's contract is the one to reimplement.** Files + pipes is cheaper and sufficient for a single machine. The ttrpc surface earns its keep only when a long-lived daemon needs live typed ordered state — i.e. only if you have a daemon. Tradeoff: files+pipes give no push notification, so the CLI reconciles on each invocation. At single-machine scale that is a few `read`s.

## 11.2 CRI-O — the state model worth stealing

`[C]` HIGH — exists solely to serve the kubelet's CRI. Its README bounds scope to CRI and **explicitly excludes** building, signing and pushing images; its CLIs are *"only meant for testing"*. It tracks Kubernetes minor releases with the n-2 skew policy. containerd, by contrast, is general-purpose with CRI as one plugin — which is why containerd carries a client API, namespaces, a plugin system and a metadata DB that CRI-O simply refuses to have.

`[C]` HIGH — trust boundary = the root-owned CRI gRPC socket. **CRI-O is a daemon**; it shares conmon with Podman but not the daemonless property.

`[C]` HIGH — **state: no database. Per-container `state.json`, written atomically.** `internal/lib/container_server.go:ContainerStateToDisk` → `ioutils.NewAtomicFileWriter(ctr.StatePath(), 0644)`; `internal/oci/container.go:368 StatePath() = filepath.Join(c.dir, "state.json")`. On restart, `LoadSandbox`/`LoadContainer` walk containers/storage, read the **OCI spec back off disk**, `ctr.FromDisk()`, and restore volumes and ID mappings **from the spec itself**. Everything else is an in-memory index rebuilt from disk + the runtime + conmon's exit files.

`[R]` HIGH — **This is the single most reassuring data point in this document.** CRI-O runs production Kubernetes nodes with hundreds of containers on a pile of `state.json` files, atomic-rename writes, and no database. Anyone claiming a single-machine container engine needs a k/v store is importing containerd's multi-tenant requirements. It also validates §6.4's design directly, including "keep a verbatim copy of `config.json` because you will need to restore things from it".

## 11.3 containerd — what's real and what's governance

`[C]` HIGH — *"containerd, the daemon, does not directly launch containers. Instead, it acts as a higher-level manager or hub for coordinating the activities of containers and content"* (`docs/runtime-v2.md`).

`[C]` HIGH — **plugin architecture**: everything is `<type>.<id>`, configured as `[plugins."<type>.<id>"]`: `io.containerd.content.v1`, `io.containerd.snapshotter.v1`, `io.containerd.metadata.v1`, `io.containerd.differ.v1`, `io.containerd.gc.v1`, `io.containerd.runtime.v2`, `io.containerd.grpc.v1`. Stated rationale: *"containerd uses plugins internally to ensure that internal implementations are decoupled, stable, and treated equally with external plugins."* `[R]` HIGH — **that is a third-party-extensibility (project governance) requirement, not a technical one.** An interface with one implementation is not architecture. We will have exactly one snapshotter, one runtime, one content store. Write the concrete thing; extract a trait the day there is a second implementation.

`[C]` HIGH — `plugins/metadata/plugin.go:169`: `bolt.Open(filepath.Join(root, "meta.db"), 0644, ...)` **with a configurable open timeout because `bbolt.Open` takes `flock(2)` and would otherwise block forever.** `[O]` HIGH — confirmed live on `host-a`: `meta.db` is containerd's fd 3, held for the daemon's life. **BoltDB's single-writer flock is what makes containerd a daemon whether it wants to be or not.**

`[C]` HIGH — **namespaces are explicitly not a security boundary**: *"namespaces, as implemented, is an administrative construct that is not meant to be used as a security feature."* Purpose: *"Underlying image content is still shared via content addresses but image names and metadata are separate per namespace."* Docker's is literally `"moby"` (`moby/daemon/config/config.go:58 DefaultContainersNamespace = "moby"`, plus `plugins.moby`) — `[O]` HIGH visible in the shim argv and every path on `host-a`. `[R]` HIGH — **pure multi-tenancy. Not ours. Skip.**

## 11.4 Docker/moby — what dockerd still owns, precisely

`[C]` HIGH — the API (and back-compat forever); BuildKit; libnetwork (bridge/overlay drivers, IPAM, embedded DNS, iptables/nftables); volumes and the volume plugin API; swarm; health checks; restart policies; events; `docker cp`.

`[C]` HIGH — **the userland proxy**: `daemon/libnetwork/portmapper/proxy_linux.go:StartProxy` spawns **one `docker-proxy` per port binding**, with `SysProcAttr.Pdeathsig = SIGTERM`, and where possible hands it the already-bound listening socket as an extra fd plus `-use-listen-fd`. `[O]` HIGH — exactly matches what I observed on `host-a`: four `docker-proxy` children of dockerd, all with `-use-listen-fd`, for :80 and :443 over v4 and v6. It exists to cover what DNAT alone misses: host-loopback→published-port, IPv6 gaps, and holding the port reserved. `[R]` HIGH — with root + netns + veth + nftables DNAT we do **not** need it. Skip; add only if `curl localhost:8080` fails and we care. (D4's call.)

`[C]` HIGH — **logging**: `daemon/internal/libcontainerd/remote/client.go:220 fifos := newFIFOSet(bundle, id, withStdin, spec.Process.Terminal)` → `cio.DirectIO` → the daemon's logger. dockerd creates the FIFOs and holds the read ends. `[O]` HIGH — confirmed in dockerd's fd table on `host-a` (§5.3). `[C]` HIGH — **consequence: if dockerd is down, container stdout blocks once the pipe buffer fills.** That is the concrete architectural difference from conmon, which writes the log itself and needs nobody. `[R]` HIGH — **be conmon here, not dockerd.** §5.3 already recommends this; this is the independent confirmation of why.

## 11.5 gVisor — where the boundary moved

`[C]` HIGH — *"The application's direct interactions with the host System API are intercepted by the Sentry, which implements the System API instead."* Three principles: *"No system call is passed through directly to the host. Every supported call has an independent implementation in the Sentry"*; *"Only common, universal functionality is implemented"*; *"The host surface exposed to the Sentry is minimized."*

`[C]` HIGH — process boundaries: `runsc` → **Sentry** (a userspace guest kernel, running in an *empty mount namespace*) + a separate **Gofer** that owns filesystem access and passes fds into the sandbox over a socket. The Sentry's own permitted host operations: socket comms with the Gofer, *"a minimal set of host system calls. The calls do not include the creation of new sockets or opening files"*, and packet read/write on a virtual ethernet device.

`[C]` HIGH — interception platforms: **ptrace** (`PTRACE_SYSEMU`) — *"high context switch overhead… system call-heavy applications may pay a performance penalty"*, now unsupported and *"expected to eventually be removed entirely"*; **KVM** — Sentry is *"both guest OS and VMM"*; **systrap** — `SECCOMP_RET_TRAP` delivers `SIGSYS` to the offending thread *"which hands over control to gVisor to handle the system call"*, **the default since mid-2023**.

`[C]` HIGH — what it solves that namespaces don't: namespaces leave the entire host syscall surface reachable, so one kernel bug in any syscall is a full escape. gVisor reduces the host-visible surface to a small fixed set from one hardened process. It explicitly does not address hardware side-channels: *"gVisor relies on the host operating system and the platform for defense against hardware-based attacks."*

`[R]` HIGH — **not our problem.** Our threat model is "the user runs their own images on their own machine". Note for honesty in the project's README: **a namespace-based engine does not protect against a hostile image.** Say so rather than implying otherwise.

## 11.6 Kata / Firecracker — and the one thing worth stealing

`[C]` HIGH — `containerd-shim-kata-v2` implements the shim v2 contract exactly but **never calls runc**: it boots a hypervisor and a VM per sandbox, with a Rust `kata-agent` inside as *"supervisor for containers and workloads"*. Runtime↔agent is **ttRPC over VSOCK**, carrying the container's I/O streams too. Rootfs reaches the guest via **virtio-fs**; the guest kernel image is mapped with **DAX**. Trust boundary = the VM/EPT boundary.

`[F]` MED — the OCI spec's `vm` section (`config-vm.md`) is the standardized handoff: `vm.hypervisor.path`, `vm.kernel.path` (+ optional `initrd`), `vm.image.path` + required `format` (`raw|qcow2|vdi|vmdk|vhd`), all *"absolute path in the runtime mount namespace"*. The spec defines the fields but prescribes no runtime behaviour, which is why Kata configures the hypervisor from its own TOML instead. `[R]` HIGH — we can ignore `vm` entirely and still be spec-compliant (it's OPTIONAL, §1.3).

`[C]` HIGH — **Firecracker's `jailer` is worth reading as a 200-line specification of "drop privileges, then exec"**: builds `<chroot_base>/<exec_file>/<id>/root` and `pivot_root()`s into it; `mknod`s `/dev/kvm` inside and `chown`s it; creates `<cgroup_base>/<parent>/<id>` and writes its pid to `tasks`/`cgroup.procs` (v1 and v2); `unshare()` for mounts, optional `clone(CLONE_NEWPID)` via `--new-pid-ns`; drops to uid 123 / gid 100; with `--daemonize` calls `setsid()` and redirects stdio to `/dev/null`; then `execv`s firecracker. `[R]` HIGH — that ordered checklist is exactly our container-init sequence minus the OCI parts. Use it as a cross-check against D1's syscall sequence.

## 11.7 Apptainer/Singularity — the fourth family, and which simplifications are principled

`[C]` HIGH — **the problem**: a scientist has an image on a shared filesystem and an allocation on a node they don't administer. They need the image's userspace, their own identity, their own `$HOME`, GPUs and MPI — and cannot be given a root socket or install anything. Everything Docker does to *manage* containers is, in that setting, irrelevant or harmful.

`[C]` HIGH — **trust boundary = the user's uid, kernel-enforced.** *"When a user runs a container with Apptainer, it is started as a normal process running under the user's account"*, explicitly contrasted with Docker's root daemon. The invariant: *"even if you run `sudo` inside your container, you won't be able to change to another user, or gain root privileges by other means"* — enforced with `PR_NO_NEW_PRIVS` and `nosuid` mounts. **There is no API surface to attack because there is no service.**

`[C]` HIGH — **`cmd/starter/c/starter.c` (1715 lines) is C with a Go tail** — the same trick as `nsexec.c`, for the same reason, but via a global `enum goexec goexecute {STAGE1, STAGE2, MASTER, RPC_SERVER}` that the C sets and *returns* into `main_linux.go` to dispatch. `fork_ns()` is `clone()` wrapped in `sigsetjmp` — commented as a *"hack to make clone() behave like fork()"*. The lineage for `apptainer exec image.sif cmd`:
```
your shell
└── apptainer            (Go CLI: resolve image, build engine config)
    └── starter[-suid]   (C; re-execs itself with the config on a pipe)
        ├── STAGE1                fork(CLONE_FILES) → parse apptainer.conf, validate, exit
        └── fork_ns(flags)        flags |= CLONE_NEWUSER (unpriv), CLONE_NEWPID if --pid
            ├── parent → MASTER   stays outside; writes uid_map/gid_map over master_socket;
            │                     holds FUSE mounts; instance mgmt; cleanup
            └── child
                ├── wait for the id-map event on master_socket
                ├── set_parent_death_signal(SIGKILL)
                ├── net/uts/ipc/cgroup ns init, then mount ns init
                ├── fork_ns(CLONE_FS) → RPC_SERVER  does mounts + pivot_root, then exits
                │        (CLONE_FS is deliberate: "we want that pivot_root/chroot occurring
                │         in RPC server process also affect stage 2 process")
                └── STAGE2 → apply_privileges() → execve(your command)
```

`[C]` HIGH — **Where is PID 1? Normally nowhere.** STAGE2 `execve`s your command in the host PID namespace: it is a process in your session, visible in `ps` on the node, reaped by your shell, and **its exit code is your exit code.** With `--pid` a new PID namespace is created and STAGE2 *is* PID 1 (`starter.c:1481`: *"at this stage we are PID 1 if PID namespace requested"*); apptainer then inserts a small init shim to reap zombies, disabled by `--no-init`. `[O]` HIGH — the flag exists: `apptainer exec --help` on `OLD-HOST` (kernel 4.18.0-425.3.1.el8) lists `--no-init  do NOT start shim process with --pid`.

`[C]` HIGH — **the master process is the closest thing to conmon and is deliberately weaker**: it writes id maps, holds FUSE mounts alive, supervises instances. It holds **no pty master, no log file, no exit-status file, no attach socket** — because your shell already holds the tty, the exit status flows through `wait()`, and there is nothing to reconnect *to*.

`[C]` HIGH — a hard safety check worth stealing: after joining a shared mount namespace, master compares `st_dev`/`st_ino` of `/` before and after and **aborts**, warning *"Running inside a weak chrooted environment, prefer pivot_root instead of chroot"*. `[R]` HIGH — a two-`stat` assertion that catches "we thought we pivoted and didn't". Add it.

`[C]` HIGH — **state: there isn't any.** Image → the SIF file on the shared FS (naming is the filesystem's job). "Is it running?" → `ps` / the scheduler. Resource limits → the scheduler's cgroup, inherited untouched. `[O]` HIGH — on `OLD-HOST`, `cat /proc/self/cgroup` → `12:pids:/user.slice/user-900000000.slice/session-1505185.scope`; apptainer inherits it. `[O]` HIGH — `apptainer buildcfg | grep SESSIONDIR` → `SESSIONDIR=/var/lib/apptainer/mnt/session`, root-owned and empty; `apptainer.conf` sets `sessiondir max size = 64` (MB tmpfs).

`[C]` HIGH — **the one exception: instances.** `~/.apptainer/instances/` holds a per-instance JSON (keyed by app/hostname/user/name) plus `~/.apptainer/instances/logs/<hostname>/<user>/<name>.{out,err}`. `instance list` reads that directory; `exec instance://name` reads the pid from the JSON and `setns()`es in — the `joinOnly` path in `starter.c`, which skips the RPC server entirely (*"Don't execute RPC server, joining instance"*). `[O]` HIGH — `ls -lad ~/.apptainer` on `OLD-HOST` → `drwx------ researcher domain-users`. Note *"Instances are linked to the user account that started them"* — the state is per-user, in the user's home, **on NFS**.

`[C]` HIGH — **SIF vs OCI layers.** SIF is one file: header + descriptors + data objects — typically a **squashfs partition**, plus the definition file, JSON metadata, and optionally an embedded signature: *"The container filesystem is immutable, and can be signed. The signature travels as part of the SIF image itself."* At runtime the partition is mounted **at its byte offset inside the SIF**: `internal/pkg/image/driver/imagedriver.go:248` appends `,offset=<n>` for `squashfuse`/`squashfuse_ll`. For ext3 partitions `fuse2fs` has no offset support, so apptainer injects `LD_PRELOAD=<libexecdir>/apptainer/lib/offsetpreload.so` with `OFFSETPRELOAD_FILE`/`OFFSETPRELOAD_OFFSET` — an interposition shim that shifts every read. `[O]` HIGH — both present on `OLD-HOST`: `rpm -ql apptainer | grep libexec` → `/usr/libexec/apptainer/bin/{squashfuse_ll,fuse2fs,gocryptfs,mksquashfs,unsquashfs}` and `/usr/libexec/apptainer/lib/offsetpreload.so`.
The trade is total: **no layer dedup, no incremental pull, no build cache** — in exchange for one `cp`-able, `rsync`-able, signable file that 500 nodes read-only-share off one NFS path. Correct for HPC, wrong for a laptop iterating on a Dockerfile. (D3's domain; noted for contrast.)

`[C]` HIGH — **setuid vs unprivileged.** Docs: *"The default installation of Apptainer does not include a setuid-root component"*; *"User namespaces are required whenever the setuid-root component of Apptainer is not installed"*; *"a kernel >=3.8 is required, with >=4.18 being recommended due to support for unprivileged mounting of FUSE filesystems (needed for example for mounting SIF files)"*; unprivileged overlayfs wants >= 5.11; without setuid it *"attempts to use external setuid binaries `newuidmap` and `newgidmap`"*.

What the suid helper does that a userns cannot (and this is the general lesson): `[F]` HIGH
1. **`mount(2)` of a loop-backed real filesystem.** An unprivileged userns grants `CAP_SYS_ADMIN` *in that namespace*, which does **not** authorize mounting a block-backed fs. Only real root can `losetup` + `mount` the squashfs. Unprivileged replacement: FUSE (`squashfuse_ll` + setuid `fusermount3`) — hence the 4.18 floor.
2. **Arbitrary uid/gid mapping** (`--fakeroot`: your uid→0 *plus* a subuid range). Writing a multi-range `uid_map` needs `CAP_SETUID` in the parent ns — from the suid starter or from setuid `newuidmap` + an `/etc/subuid` allocation.
3. **`overlayfs`** as a real mount below 5.11 (replacement: `fuse-overlayfs`).
4. **Kernel dm-crypt SIF encryption** (replacement: `gocryptfs`).

`[O]` HIGH — **on `OLD-HOST` (Rocky 8.8, kernel 4.18.0-425.3.1.el8) the suid path is absent and the unprivileged path is fully wired:**
- `apptainer buildcfg | grep SUID` → **`APPTAINER_SUID_INSTALL=0`**
- `rpm -ql apptainer | grep -i starter` → only `/usr/libexec/apptainer/bin/starter`; **no `starter-suid`**. `rpm -qa | grep -i apptainer` → only `apptainer-1.4.0-1.el8`.
- `ls -la /usr/libexec/apptainer/bin/starter` → `-rwxr-xr-x root root 23743768` — **no setuid bit**.
- `ls -la /usr/bin/fusermount3` → **`-rwsr-xr-x`** (setuid — this is what makes unprivileged FUSE SIF mounts work).
- `ls -la /usr/bin/newuidmap /usr/bin/newgidmap` → `-rwxr-xr-x` — **not setuid**.
- `grep "^$(id -un):" /etc/subuid /etc/subgid` → **no entry for this user** (other users have `:100000:65536` ranges).
- `sysctl user.max_user_namespaces` → `1029387` (unprivileged userns enabled).
- `apptainer.conf` (comments stripped): `allow setuid = yes`, `allow user ns = yes`, `mount home = yes`, `mount tmp = yes`, `enable fusemount = yes`, `enable overlay = yes`, `enable underlay = yes`, `systemd cgroups = yes`, `allow monitoring = no`, `config passwd = yes`, `config group = yes`.

`[O]` MED — **Consequence:** on this host `--fakeroot` cannot map a uid range — no suid starter, no setuid `newuidmap`, no subuid allocation. What works: identity-mapped `--userns`, FUSE-mounted SIF, host uid preserved. `allow setuid = yes` is vestigial since the binary doesn't exist. Confidence MED because settling it definitively requires actually running `apptainer exec --fakeroot`, which was out of scope.

`[C]` HIGH — **defaults**: `$HOME`, `/tmp`, `/var/tmp`, **`$PWD`**, `/etc/localtime`, `/etc/hosts` are bound by default; `/etc/passwd` and `/etc/group` entries are generated for you; your environment passes through; **your uid is not remapped** — you are uid 1234 inside and out. `[O]` HIGH — `apptainer exec --help` on `OLD-HOST`: `-H/--home` defaults to `"/home/researcher"`; `-c/--contain` = *"use minimal /dev and empty other directories (e.g. /tmp and $HOME) instead of sharing filesystems from your host"*; `-C/--containall` = *"contain not only file systems, but also PID, IPC, and environment"*; `--compat` = *"apply settings for increased OCI/Docker compatibility. Infers --containall, --no-init, --no-umask, --no-eval, --writable-tmpfs."*

`[C]` HIGH — **that last flag is the whole story: Docker semantics are the opt-in, and isolation is what you pay extra for. Docker inverted it.**

`[C]` HIGH — **`apptainer exec` vs `docker exec`, structurally:**

| | `docker exec` | `apptainer exec` |
|---|---|---|
| Who is asked | dockerd, over a socket | nobody |
| What happens | dockerd → containerd → shim → `runc exec` → `setns` into an existing container | `apptainer` → `starter` → clone/unshare **fresh** namespaces → mount SIF → `execve`, in your process tree |
| Precondition | a container must already be running | none — there *is* no container until you exec |
| Your process | a client of a remote process | **is** the container |
| Exit code | fetched over the API | `wait()` on your own child |
| PID 1 | the container's existing init | none (or your process, with `--pid`) |

(`apptainer exec instance://name` is the exception and *is* structurally like `docker exec`.)

### Principled vs HPC-specific — the key analytical judgement

`[R]` HIGH — **Principled. A general engine can and should adopt these:**
1. **The container is a process, not a service.** Foreground execution with the CLI as supervisor needs no shim, no exit file, no attach socket, no reconnect protocol. Apptainer proves the 95% interactive case needs none of it.
2. **Exit status via `wait()`.** Podman's exit-file + `--exit-command` + inotify dance exists *only* because the supervisor must outlive the client.
3. **No metadata database on the common path.** Nothing to corrupt, no lock contention, no schema migration. (CRI-O reached the same conclusion from the daemon side — §11.2.)
4. **Immutable, verifiable image; mount rather than unpack.** The SIF *format* is HPC-flavoured; "immutable image + signature + mount instead of unpack" is a general win.
5. **Isolation as opt-in, per-namespace.** `--pid`/`--net`/`--ipc`/`--uts`/`--userns` independently. Docker's all-or-nothing default forces you to build a network stack, a DNS server and a port proxy before `hello-world` works. `[R]` HIGH — **this is directly actionable for us: make each namespace a flag, and get `mydocker run` working with `--net=host` before writing any networking.**
6. **Refuse to be a package manager.** Bounded scope is why Apptainer is one binary.
7. **`pivot_root`, never `chroot`, and *verify* it** with the `st_dev`/`st_ino` assertion.

`[R]` HIGH — **HPC-specific. Do NOT generalize these:**
1. **Default-binding `$HOME`, `/tmp`, `$PWD`.** Correct only because the shared filesystem *is* the workflow. Anywhere else it is a data-exfiltration default. Apptainer knows — that's what `--contain` is for.
2. **Passing the user's uid through unmapped.** Works because the host account model is authoritative across all nodes. An engine pulling from Docker Hub cannot assume the image's `USER 1000` means anything.
3. **No cgroups by default.** Works because Slurm/PBS already put the job in a cgroup. On a general single machine, no cgroup = one runaway container takes the box. **We have cgroup v2 and root — this simplification is not available to us.**
4. **No network isolation by default.** Works because MPI/InfiniBand want the host network and the fabric is the boundary.
5. **Trusted users.** The threat model is "the user could do this anyway as themselves" — true on a login node, false the moment you accept someone else's image.
6. **A setuid-root C binary as an acceptable option.** On a machine we control with cgroup v2 and a modern kernel, that is a liability with no upside. `[O]` This site already agrees: `APPTAINER_SUID_INSTALL=0`.
7. **No image distribution problem.** `cp file.sif /shared/` is not a registry.

## 11.8 Which of these problems are ours

`[R]` HIGH — **Ours, because the kernel or the spec forces it:** the intermediate fork for `CLONE_NEWPID`; `CLONE_PARENT` (or a subreaper) so init doesn't reparent to PID 1; a create/start rendezvous; a typed child→parent sync + error channel; `pivot_root` and mount ordering; `PR_SET_CHILD_SUBREAPER` + `waitpid` in the supervisor; cgroup v2 done properly (`clone3(CLONE_INTO_CGROUP)`, `cgroup.kill`, `memory.events`); and — the moment we bind-mount a writable host path — the **memfd self-clone against CVE-2019-5736**. `[C]` HIGH — runc and crun ship the *same file* for that last one (`cloned_binary.c`, in crun at `src/libcrun/linux.c:6986` as `__attribute__((constructor)) static void libcrun_rexec(void)` — *"Protection for attacks like CVE-2019-5736"*). Roughly 60 lines in Rust: `memfd_create` + copy + `F_SEAL_*` + re-exec once at startup. `[R]` HIGH — do it early; retrofitting means auditing every exec path.

`[C]` LOW — **youki appears not to have this.** The families agent grepped `crates/libcontainer/src/process/init/process.rs` for `memfd`/`5736` and found only `/proc/self/exe` in a paths list. *What would settle it:* grep the whole `crates/` tree for `memfd_create`/`MFD_ALLOW_SEALING` and check `crates/libcontainer/src/syscall/`. If genuinely absent, it is a real exposure — and either way it means **we cannot assume "youki does it" for security-critical mitigations.**

`[R]` HIGH — **Ours only if we want a specific feature, and priced accordingly:** a supervisor (only for `-d`); pty handling (only for `-t`); an attach socket (only for `-d` *and* `-t`); a log writer (only `-d`); an exit-status file (only `-d`); port publishing without `docker-proxy` (only if D4 needs host-loopback).

`[R]` HIGH — **Not ours. This is where the cargo-culting lives:** a metadata database; containerd namespaces; the plugin architecture; a shim ttrpc/gRPC API with mandated event ordering and a `delete` subcommand for orphan recovery; CRI (and with it pod sandboxes, the pause container, CNI, and shim-grouping-by-sandbox-id); the entire rootless stack (`newuidmap`/`newgidmap`, subuid allocation, `slirp4netns`/`pasta`, `fuse-overlayfs`, cgroup-delegation detection, rootlessport — `[O]` all installed next to podman on `OLD-HOST`, and precisely what root-first lets us skip); runc's Go workarounds; gVisor/Kata/Firecracker; image build/registry/GC/signing (D3); `docker-proxy`; a setuid helper.

`[R]` HIGH — **The one sentence worth taking from all nine projects: almost every mechanism above the fork/exec/namespace/cgroup layer exists to solve "my supervisor is not my client's parent."** Apptainer's whole architecture is the observation that if you never create that problem, you need none of the solutions. Our §7 design deliberately *does* create it (the supervisor is detached from the CLI) — and that is a **priced choice**, made because we want `-d` and a durable exit code. It is worth being explicit that the price is: a supervisor process, a log writer, a state file, and eventually an attach socket. Everything else on the "not ours" list is what we avoid by not going further.

---

# 12. Rust substrate — the foundational dependency decisions

**Relationship to D1 §11.1.** D1 already produced a crate table and a recommended stack, and it ends with open question 9: *"Delegated crate assessment (§11.1) had not returned at write time."* This section is that assessment. It **agrees with D1 on every `[R]` except two**, and both changes are in rustix's favour — D1 under-counted rustix's coverage because it looked at the mount and namespace modules only. The corrections are called out explicitly in §12.1.7 and §12.3 so the two documents do not silently disagree.

**Method.** Every version, date, download and maintainer number below was fetched on **2026-09-05** from the crates.io JSON API (`https://crates.io/api/v1/crates/<name>`, `.../owners`) and the GitHub REST API (`/repos/<o>/<r>`, `/contributors`, `/commits?since=`). Every coverage claim was verified by **downloading the published `.crate` tarball from `static.crates.io` and grepping the actual source**, not by reading docs.rs. Crate sources read in full or in part: `rustix-1.1.4`, `nix-0.31.3`, `caps-0.5.6`, `oci-spec-0.10.0`, `pentacle-1.1.0`. All `[F]` claims in this section are "fact about the crate as published at that version", verifiable by repeating the grep.

**The one question this section answers.** Not "which crate is nicest" but: *for each abstraction, does taking it cost us the mechanic we came here to learn?* A crate that types out 6,000 lines of serde structs for us costs nothing pedagogically. A crate that decides for us how a process becomes a container costs everything.

## 12.1 `nix` vs `rustix` vs raw `libc` — the foundational choice

### 12.1.1 They are not three points on one axis

`[F]` HIGH — the three are at genuinely different levels, and the difference that matters is *how the syscall gets issued*, not *how pretty the wrapper is*.

| | `libc` 0.2.189 | `nix` 0.31.3 | `rustix` 1.1.4 |
|---|---|---|---|
| What it is | FFI declarations for the platform's C library | safe 1:1 wrappers **over `libc`** | safe wrappers over **either** `libc` **or** raw syscalls |
| Syscall mechanism | glibc/musl function call → PLT → libc's wrapper → `syscall` insn | same (it calls `libc`) | on x86-64 Linux, **inline `asm!` `syscall` instruction**, no libc, no PLT |
| Errors | `errno` via a libc TLS accessor function | `Errno` — a `#[repr(i32)]`-style enum, `Copy`, no allocation | `Errno(u16)` newtype in the `linux_raw` backend, no allocation |
| Deps | none | `libc`, `bitflags`, `cfg-if` | `bitflags`, `linux-raw-sys` (+ `libc`/`errno` **only** on the libc backend) |
| Hides mechanics? | No — it *is* the syscall | Barely — argument marshalling and errno checking | Barely — same, plus `OwnedFd`/`BorrowedFd` I/O-safety types |

`[F]` HIGH — **rustix's `linux_raw` backend is the default**, not an opt-in. Verified in `rustix-1.1.4/Cargo.toml`: the `linux-raw-sys` and `libc` dependencies are behind mutually exclusive `[target.'cfg(...)']` predicates keyed on `rustix_use_libc`, `miri`, target OS and architecture; on `x86_64-unknown-linux-*` with none of those cfgs set, `linux-raw-sys` is pulled in and `libc` is **not**. The libc backend is selected by the `rustix_use_libc` cfg or by being on a non-Linux/unsupported-arch target. Our target (`x86_64-unknown-linux-{gnu,musl}`) gets the raw backend.

`[R]` HIGH — that property is worth more to us than to almost any other Rust project, for a reason specific to §8: **a syscall that does not route through libc cannot deadlock on a libc lock inherited across `clone()`.** It is not a *guarantee* of async-signal-safety (rustix does not claim one, and argument marshalling can still allocate — see §12.1.3), but it removes one entire class of post-clone hazard for free.

### 12.1.2 Coverage of the syscalls this project actually needs

`[F]` HIGH — all verified by grep against the extracted crate sources. ✅ = a public safe wrapper exists; ⚠️ = present but with a caveat named in the notes; ❌ = absent.

| Syscall | `libc` | `nix` 0.31.3 | `rustix` 1.1.4 | Notes |
|---|---|---|---|---|
| `clone3` | ❌ decl. | ❌ | ❌ **by policy** | see §12.1.4 — this is the decisive row |
| `clone` | ✅ variadic glibc wrapper | ⚠️ `sched::clone` | ❌ by policy | nix's is unsound; see below |
| `unshare` | ✅ | ✅ `sched::unshare` | ✅ `thread::unshare` (+ `unshare_unsafe`) | |
| `setns` | ✅ | ✅ `sched::setns` | ✅ under the name `thread::move_into_link_name_space` / `move_into_thread_name_spaces` | |
| `pivot_root` | ❌ decl. | ✅ `unistd::pivot_root` | ✅ `process::pivot_root` | both are `syscall(SYS_pivot_root)` underneath |
| `mount` / `umount2` | ✅ | ✅ `mount::{mount,umount2}` | ✅ + `mount_bind`, `mount_bind_recursive`, `mount_change`, `mount_move`, `mount_remount`, `unmount` | rustix's split-by-intent API is the nicer one |
| `fsopen`/`fsmount`/`fsconfig`/`move_mount`/`open_tree` | ❌ decl. | ❌ | ✅ all, in `rustix::mount` | rustix is the **only** crate with the new mount API |
| `mount_setattr` | ❌ decl. | ❌ | ❌ | rustix lists it in `not_implemented::yet`; PRs #975, #1002, #1630 all **open** as of 2026-09-05 |
| `openat2` | ❌ decl. | ✅ `fcntl::openat2` | ✅ `fs::openat2` | **corrects D1 §11.1**, which implied only rustix has it |
| `close_range` | ⚠️ gnu-only | ❌ | ❌ | raw syscall on musl either way |
| `pidfd_open` / `pidfd_send_signal` / `pidfd_getfd` | ❌ decl. | ❌ | ✅ all three, `rustix::process` | **only rustix**; D1 §8.2 makes pidfds the container handle |
| `signalfd` | ✅ | ✅ `sys::signalfd` | ❌ (`not_implemented::yet`) | **only nix**; §7.2's supervisor loop wants it |
| `prctl(PR_SET_CHILD_SUBREAPER)` | via `prctl` | ❌ | ✅ `process::set_child_subreaper` / `child_subreaper` | §7.2's supervisor requires it |
| `prctl(PR_SET_NO_NEW_PRIVS)` | via `prctl` | ❌ | ✅ `thread::set_no_new_privs` | |
| `prctl(PR_SET_PDEATHSIG)` | via `prctl` | ❌ | ✅ `process::set_parent_process_death_signal` | |
| `capget`/`capset` | ❌ decl., **no `CAP_*` consts** | ❌ | ✅ `thread::capabilities` / `set_capabilities` | see §12.3 |
| bounding set (`PR_CAPBSET_*`) | via `prctl` | ❌ | ✅ `capability_is_in_bounding_set` / `remove_capability_from_bounding_set` | |
| ambient set (`PR_CAP_AMBIENT_*`) | via `prctl` | ❌ | ✅ `capability_is_in_ambient_set` / `configure_capability_in_ambient_set` / `clear_ambient_capability_set` | |
| `setresuid`/`setresgid`/`setgroups` **per-thread, non-broadcasting** | raw `syscall()` | ❌ (nix uses glibc's) | ✅ `thread::set_thread_res_uid` / `set_thread_res_gid` / `set_thread_groups` / `set_thread_uid` / `set_thread_gid` | **this is the D1 §11.2 glibc-`setxid`-broadcast problem, already solved** |
| `seccomp(2)` filter install | raw `syscall()` | ❌ | ❌ — only `prctl(PR_SET_SECCOMP)` mode 0/1/2 via `set_secure_computing_mode` | verified in `src/thread/prctl.rs:163-176`; no BPF filter path. §12.11 |
| `sethostname` | ✅ | ✅ | ✅ `system::sethostname` | |
| `waitpid`/`waitid` | ✅ | ✅ | ✅ `process::{wait,waitpid,waitpgid,waitid}` | |
| `poll` / `epoll` / `eventfd` | ✅ | ✅ | ✅ `rustix::event::{poll, epoll, eventfd}` | |
| `sendmsg`/`recvmsg` + `SCM_RIGHTS` | ✅ | ⚠️ `ControlMessageOwned::ScmRights(Vec<RawFd>)` — **allocates on receive** | ✅ `ScmRights(&[BorrowedFd])` + `cmsg_space!` stack array — allocation-free | §5.2's console socket |
| `socketpair` | ✅ | ✅ | ✅ | |
| pty (`posix_openpt`/`grantpt`/`unlockpt`/`ptsname`) | ✅ | ✅ + `openpty` | ✅ + **`ioctl_tiocgptpeer`** (TIOCGPTPEER, Linux 4.13) | rustix's `TIOCGPTPEER` gets the pts fd with no path lookup — the right primitive for §5.2 |
| `ioctl(TIOCSCTTY)` | ✅ | ✅ | ✅ `process::ioctl_tiocsctty` | |
| `memfd_create` | ❌ decl. | ✅ `sys::memfd` | ✅ `fs::memfd_create` | for the CVE-2019-5736 self-seal, §11.8 |
| `execve` | ✅ | ✅ `unistd::execve` | ❌ — in `not_implemented::quite_yet` | rustix has it internally, not exposed (issue #1314) |

`[O]` HIGH — the `nix::sched::clone` soundness bugs D1 cites are **still open on 2026-09-05**, checked via the GitHub issues API: **#360** *"clone() is broken and unsafe with CLONE_VM flag"* — open, created 2016-04-19, last updated 2018-01-21; **#2803** *"Potential soundness issue: Stack use-after-free and data race in `sched::clone` under `CloneFlags::CLONE_VM`"* — open, created 2026-06-20. Ten years and three months for #360. D1 §11.1 and D2 §10.2 (youki's own comment: *"The nix wrapper also does not treat the closure memory correctly"*) both reach the same verdict independently. Settled.

### 12.1.3 The allocation question — measured, not assumed

This is the constraint from D1 §11.2 that governs everything: **between `clone()` and `execve()` the child may not allocate.** So: do these crates allocate on the paths we would call?

`[F]` HIGH — **`nix`**, `src/lib.rs:319-365`. `NixPath::with_nix_path` for `[u8]`/`str`/`Path`/`OsStr` copies into a `MaybeUninit<[u8; MAX_STACK_ALLOCATION]>` where `const MAX_STACK_ALLOCATION: usize = 1024`, and for anything ≥ 1024 bytes calls the `#[cold] #[inline(never)] with_nix_path_allocating()` → `CString::new(from)` — **a heap allocation**. `impl NixPath for CStr` is `Ok(f(self))` — no copy, no allocation.

`[F]` HIGH — **`rustix`**, `src/path/arg.rs:952-997` and `src/path/mod.rs:9`. Same shape, smaller buffer: `const SMALL_PATH_BUFFER_SIZE: usize = 256`, and `bytes.len() >= SMALL_PATH_BUFFER_SIZE` falls through to `CString::new(bytes)`. `impl Arg for &CStr`'s `into_with_c_str` is literally `f(self)` — no allocation.

`[R]` HIGH — **the operational rule, and it is a one-liner:** *every path handed to a syscall inside the cloned child is a pre-built `&CStr`, never a `&str`, `&Path` or `String`.* With that discipline both crates are allocation-free on the paths we use, and the rule is exactly the one D1 §10.1 step 7 already imposes ("every `CString` for path, argv and envp must be built before the clone"). Without that discipline **either crate silently allocates for a path ≥ 256 (rustix) or ≥ 1024 (nix) bytes** — which is not a hypothetical: our state directory is `/var/lib/mydocker/containers/<64-hex-id>/...` and D2 §10.3's own note flags path length as a real trap. **The failure mode is a deadlock that only appears for long paths.** `[R]` HIGH — enforce it with a newtype: the child's setup functions take `&CStr` in their signatures and nothing else, so a `&str` is a compile error rather than a latent deadlock.

`[F]` MED — errors are the other allocation source, and both crates are clean: `nix::Result<T> = Result<T, Errno>` where `Errno` is a plain `Copy` enum (`src/lib.rs:229`), and rustix's `linux_raw` `Errno` is `pub struct Errno(u16)` (`src/backend/linux_raw/io/errno.rs:51`). Neither is `std::io::Error`, which boxes. MED only because I did not audit every error path in either crate, just the primary `Result` types.

### 12.1.4 What happens when a crate lacks a syscall we need — and the decisive case

`[F]` HIGH — the answer is boring and it is the reason this whole choice is low-stakes: you write the wrapper yourself, and it is **one line of `unsafe`**, because `libc` supplies the `SYS_*` number and the struct layout even when it does not supply the function. D1 §11.1 verified that `libc` declares `struct clone_args`, `struct open_how`, `struct mount_attr`, and the `MOUNT_ATTR_*` / `OPEN_TREE_*` / `MOVE_MOUNT_*` / `FSOPEN_*` constants. So a missing wrapper costs a `syscall(SYS_x, ...)` call and a `Errno::from_raw` on the return — not a fork of the crate, not a blocker.

`[F]` HIGH — **and the decisive case is `clone3`, which rustix will never provide.** `rustix-1.1.4/src/not_implemented.rs` places `fork`, `clone`, `clone3`, `exit`, `brk` and `sigaction` in a module named **`libc_internals`**, documented as: *"Functions which need access to libc internals are out of scope for rustix. Most Rust programs have a libc present, and when a libc is present, it expects to be the only thing in the process that can do certain operations… So, these functions are believed to be out of scope for rustix itself."* `execve`, `execveat`, `_exit` and `exit_group` sit in `quite_yet` — implemented internally, not publicly exposed, tracked in rustix issue #1314. `signalfd` and `mount_setattr` sit in `yet`.

`[R]` HIGH — **read that as good news, not a gap.** The two syscalls at the exact centre of this project — `clone3` and `execve` — are ones *no* general-purpose crate is going to abstract for us, by explicit policy in rustix's case and by absence in nix's. **We were always going to write those two ourselves.** That is the part we came here to learn, and it settles the framing: the substrate crates handle the ~40 syscalls *around* the clone; we own the clone and the exec. D2 §10.2's takeaway (`clone3` primary via raw syscall, require kernel ≥ 5.7, drop the `libc::clone` fallback) is unaffected by any crate choice.

### 12.1.5 Maintenance health and bus factor — checked, and one surprise

`[F]` HIGH — fetched 2026-09-05.

| crate | latest | date | 90-day dl | repo | commits last 180d | who | crates.io owners |
|---|---|---|---|---|---|---|---|
| `libc` | 0.2.189 | 2026-07-21 | 351M | `rust-lang/libc` | pushed 2026-09-05 | rust-lang org | rust-lang team |
| `rustix` | 1.1.4 | 2026-02-22 | 246M | `bytecodealliance/rustix` | **8** | sunfishcode ×2, xtqqczze ×2, badeend, SUPERCILEX, alexanderkjall, asomers | **`sunfishcode` alone** |
| `nix` | 0.31.3 | 2026-05-11 | 172M | `nix-rust/nix` | **34** | xtqqczze ×11, SteveLauC ×6, kamalmarhubi ×3, cgzones ×2, +2 | `carllerche` + **team `github:nix-rust:nix-maintainers`** |
| `linux-raw-sys` | 0.12.1 | 2025-12-23 | 239M | `sunfishcode/linux-raw-sys` | — | sunfishcode | sunfishcode |

`[F]` HIGH — **the surprise: `rustix` is closer to a one-person project than `nix` is.** All-time contributions are `sunfishcode` 1864 vs the next-highest at 37 — a ~50:1 ratio — the crates.io publish key belongs to `sunfishcode` alone, and `linux-raw-sys` (its type layer) is in `sunfishcode`'s personal namespace. `nix` has a crates.io **team** owner and four distinct humans committing in the last six months. The Bytecode Alliance GitHub org gives rustix organisational cover (CoC, security policy, a place to escalate) but the *code* is one person's.

`[R]` MED — this does **not** flip the decision, for four reasons: (1) 246M downloads per 90 days means rustix is load-bearing for a large fraction of the Rust ecosystem (`cargo` itself, `tempfile`, `is-terminal`, `linux-raw-sys` consumers) and would be forked within days if abandoned; (2) `Apache-2.0 WITH LLVM-exception OR Apache-2.0 OR MIT` makes forking legally trivial; (3) our usage is a few dozen thin wrappers over stable kernel ABI — the code we depend on does not need maintenance, only the wider surface does; (4) 8 commits in 180 days on a mature 1.x crate reads as "finished", not "abandoned" — there is no yanked release, no open advisory, and a 1.1.4 point release in 2026. MED rather than HIGH because "would be forked" is a prediction, not a fact. **If it matters to you: the mitigation is `cargo vendor` plus a pinned `Cargo.lock`, which we want anyway.**

`[F]` HIGH — the counter-datum on `nix`: **347–354 open issues+PRs** (D1 measured 347; the GitHub API returned `open_issues_count: 354` on 2026-09-05), including a soundness bug open since 2016 in the one primitive we care most about. High commit activity with a large unclosed backlog is not obviously healthier than low activity with a small one.

### 12.1.6 `unsafe` surface

`[F]` MED — raw counts, `grep -rn unsafe src/ | wc -l` on the extracted crates: `nix` **925**, `rustix` **1768**. MED because a line count is a crude proxy — it counts `unsafe` blocks, `unsafe fn`, and the word inside `// SAFETY:` comments alike, and rustix's higher count partly reflects it having two complete backends plus inline asm. `libc` is ~100% `unsafe` at the call site by construction (every `extern "C"` fn).

`[R]` HIGH — **the number that matters is not theirs, it is ours.** Whichever we pick, the `unsafe` *we* write is: `clone3`, `execve`, the raw `seccomp(2)`, `close_range` on musl, `mount_setattr` if we use it, and the child-side entry point. That is well under 200 lines and it is auditable in an afternoon. Choosing raw `libc` for everything would push our own `unsafe` line count into four figures and put the audit burden on us for code that has no pedagogical value — nobody learns anything about containers from hand-marshalling `mount(2)`'s five arguments for the ninth time.

### 12.1.7 Verdict

`[R]` HIGH — **`rustix` is the primary substrate. `libc` for `SYS_*` numbers, struct layouts and `syscall()`. `nix` for `signalfd` only — and possibly not even that.**

Justification, in order of weight:

1. **Post-clone safety is the project's single largest correctness risk (§8.3), and rustix is the only option that structurally reduces it.** The `linux_raw` backend issues syscalls by inline asm with no libc call, no PLT, no `__errno_location()`, no lock. `nix` routes every call through glibc. That does not make rustix async-signal-safe by fiat, but "fewer libc entry points in the child" is a real, structural, free win on exactly the axis where our worst failure mode lives.
2. **rustix already covers the prctl-family and capability operations that D1 told us to hand-write.** `set_child_subreaper`, `set_no_new_privs`, `set_parent_process_death_signal`, `capabilities`/`set_capabilities` (i.e. `capget`/`capset`), bounding-set drop, ambient-set manipulation, `set_keep_capabilities`, `capabilities_secure_bits`. **This is the first correction to D1 §11.1**, which recommended ~60 lines of hand-written `capget`/`capset` because it assessed only the `caps` crate. §12.3.
3. **rustix has `set_thread_res_uid`/`set_thread_res_gid`/`set_thread_groups` — the raw per-thread syscalls, not glibc's broadcasting `setxid`.** D1 §11.2 identifies glibc's `setxid` signal-broadcast as a **hang** in a cloned child and prescribes raw syscalls with a youki-style comment. rustix ships exactly that primitive, named and documented. This is the second D1 correction and it removes a genuinely subtle piece of hand-written `unsafe` from the child's most dangerous stretch.
4. **rustix owns the new mount API outright** (`fsopen`/`fsmount`/`fsconfig`/`move_mount`/`open_tree`) — no other crate has it. D1 §4.5 defers the decision on adopting it; adopting rustix now means that decision costs nothing later.
5. **`pidfd_open`/`pidfd_send_signal`/`pidfd_getfd` exist only in rustix**, and D1 §8.2 makes the pidfd the container handle (race-free signalling, pollable exit). Going nix-primary means hand-writing all three.
6. **I/O-safety types.** `OwnedFd`/`BorrowedFd` in the signatures rather than bare `RawFd` is not cosmetic in a program whose central bug class is *leaking an fd into a container* (CVE-2024-21626, D1 §11.2). A wrapper that makes fd ownership visible in the type system is worth having in exactly this program.

**What we give up by not going nix-primary, and the price:**

- `signalfd` — nix's only unique win, and §7.2's supervisor loop is specified around it. Three options: (a) add `nix` with `features = ["signal"]` for this one call; (b) hand-write it (`signalfd4` is a 3-argument syscall and `struct signalfd_siginfo` is a fixed 128-byte layout in `libc`); (c) **drop signalfd**: `pidfd_open` the container init and `poll` the pidfd for exit, which needs no signal handling at all, then do a `waitpid(-1, WNOHANG)` sweep for subreaped orphans on each wake. `[R]` MED — **prefer (c), fall back to (b).** (c) is strictly less machinery and uses the primitive D1 already recommends; the caveat is that a pidfd tells you *the direct child* exited and says nothing about grandchildren reparented to us by `PR_SET_CHILD_SUBREAPER`, so the `WNOHANG` sweep is not optional. This is a genuine, small revision to §7.2's loop and it is listed in §14 as open question 3. **Do not add `nix` for one syscall until (b) and (c) have both been tried.**
- `openpty` — nix has the glibc convenience wrapper; rustix gives you `openpt`/`grantpt`/`unlockpt`/`ptsname` plus `ioctl_tiocgptpeer`. `[R]` — rustix's is *better* for us: `TIOCGPTPEER` (Linux 4.13) returns the pts fd directly with no `/dev/pts/N` path lookup, which is precisely the fd-not-path discipline §5.2's console-socket protocol wants.
- `execve` — not exposed by rustix. We write it. We were writing it anyway (it takes pre-built `CString` argv/envp arrays per D1 §10.1 step 7, and no wrapper helps with that).

`[R]` HIGH — **explicitly rejected: raw `libc` for everything.** It hides nothing, which sounds ideal for a learning project, but the mechanics we are here to learn are *the sequence and the ordering constraints*, not the ABI of `mount(2)`. Writing 40 hand-rolled wrappers buys zero insight, adds ~1000 lines of `unsafe` we must audit, and routes every call through glibc — losing the one property (no-libc syscalls) that actually helps with our hardest problem. `libc` stays in the graph as a **types-and-numbers** dependency: `SYS_clone3`, `struct clone_args`, `struct mount_attr`, `struct open_how`, `SYS_seccomp`, `syscall()`. That role is not optional and not replaceable.

`[R]` MED — **the mixed-error-type annoyance is real and smaller than D1 feared.** D1 warned that mixing `nix` and `rustix` means two error types. Going rustix-primary means we mostly have *one* (`rustix::io::Errno`) plus our own raw-syscall wrappers, which we make return the same `Errno` via `Errno::from_raw_os_error`. One `From` impl, written once.

**Recommended stack (supersedes D1 §11.1's list):**

```
rustix   default features + ["process","thread","mount","fs","event","net","pty","system","termios"]
         (linux_raw backend, i.e. do NOT set rustix_use_libc)
libc     SYS_* numbers; clone_args / mount_attr / open_how / signalfd_siginfo layouts; syscall()
oci-spec features = ["runtime"]     — later, when we do OCI compat (§12.2)
+ ~150 lines of ours: clone3, execve, seccomp(2) filter install, close_range,
                      mount_setattr (only if D1 §4.5 adopts it), signalfd (only if §14 Q3 says so)
NOT: nix (unless §14 Q3 resolves toward signalfd), caps, cgroups-rs, sc, libcontainer, daemonize
```

## 12.2 `oci-spec` — does it track the spec, and does it defeat the purpose?

`[F]` HIGH — crate `oci-spec` **0.10.0**, published 2026-05-27, 4.39M downloads/90d, repository **`https://github.com/youki-dev/oci-spec-rs`**. Note the crate is named `oci-spec`, not `oci-spec-rs`; `oci-spec-rs` is the repository name. Fetched 2026-09-05.

**Does it track the current spec version?** `[F]` HIGH, and the answer has a wrinkle worth knowing:

- The OCI runtime-spec's latest release is **v1.3.0**, published 2025-11-04 (GitHub releases API on `opencontainers/runtime-spec`; tag list `v1.3.0, v1.2.1, v1.2.0, v1.1.0, ...`). Its additions: `config-vm hwConfig`, `config-linux intelRdt.schemata`, `config-linux netDevices`, `config-linux memoryPolicy`, `config-freebsd`, `intelRdt.enableMonitoring`, plus the behavioural fix *"runtime: fail when a poststart hook fails"* (#1262).
- `oci-spec` 0.10.0's **structs carry the v1.3.0 Linux additions**: `linux.rs:26 net_devices: Option<HashMap<String, LinuxNetDevice>>`, `linux.rs:93 memory_policy: Option<LinuxMemoryPolicy>`, `linux.rs:1449` `intelRdt.schemata`. Also present: `linux.rs:102 time_offsets: Option<HashMap<String, LinuxTimeOffset>>` (v1.1.0 time-namespace offsets), `IdMapping`, `Scheduler`, `personality`, `rdma`. Absent: `hwConfig` (config-vm — irrelevant to us, we are not a VM runtime).
- **But the emitted `ociVersion` constant lags.** `src/runtime/version.rs`: `VERSION_MAJOR = 1`, `VERSION_MINOR = 1`, `VERSION_PATCH = 0`, so `oci_spec::runtime::VERSION == "1.1.0"`. `[R]` HIGH — **do not use that constant as our `ociVersion`.** Write the version string we actually conform to, ourselves. `[O]` HIGH — for calibration, `runc 1.2.5` on `host-a` reports `spec: 1.2.0`, so even production runtimes lag the spec by a minor version; lagging is normal, silently emitting the wrong number is not.

**Maintained?** `[F]` HIGH — 40 commits in the last 180 days (2026-03-09 → 2026-09-05) from 6+ distinct humans plus dependabot: `saschagrunert` ×7 (a CRI-O maintainer), `flavio` ×6 (SUSE/Kubewarden), `tommady` ×6, `nayuta723` ×3, `DavSanchez` ×2, dependabot ×12. crates.io owners: `saschagrunert`, `utam0k`, `smiyaguchi` — **three publishers, not one.** All-time top contributors span the CRI-O, youki and Kubewarden projects. This is a genuinely multi-project, multi-vendor crate, which is the healthiest signal on this whole page.

**Does using it defeat the project's purpose?** `[R]` HIGH — **No, and this is the clearest case on the list.** The runtime module is **6,360 lines** of serde-derive structs, builders and enums with **3 occurrences of `unsafe` in the entire crate** (all three in the image module, not runtime). It contains no syscalls, no process management, no mount logic, no policy — it is a JSON schema expressed in Rust types. Hand-writing it is typing, not learning. Every mechanic worth understanding (what `config.json` *means*, which fields are mandatory, what the runtime must *do* with `linux.namespaces`) is in §1 of this document and is unaffected by who typed the struct.

`[R]` MED — **but defer it to phase 3.** §7.6 puts OCI CLI compatibility in phase 3; phases 0–2 need a hand-written subset of maybe 40 fields, and hand-writing that subset once is genuinely instructive — it forces us to read `config.md` field by field rather than tab-completing an IDE. The cost of switching later is a `From` impl or a straight replacement of one module, because §7.5 already mandates that one module owns config parsing. `[R]` — **write the subset in phase 0–1, adopt `oci-spec` with `features = ["runtime"]` at phase 3.** Do not carry a hand-written full schema into phase 3; that is the worst of both.

`[F]` MED — dependency cost if adopted: `const_format`, `derive_builder`, `getset`, `regex`, `serde`, `serde_json`, `strum`, `strum_macros`, `thiserror`. All build-time/derive-heavy, none runtime-hazardous, none thread-spawning. `regex` is the heaviest and is a compile-time cost, not a fork hazard. MED because I read the declared list, not the resolved lockfile.

## 12.3 `caps` — superseded, do not adopt

`[F]` HIGH — `caps` 0.5.6, published **2025-10-17**, 7.38M dl/90d, repo `lucab/caps-rs`, crates.io owner **`lucab` alone**, all-time contributions `lucab` 85 of ~95, and **0 commits in the last 180 days** (GitHub commits API, `since=2026-03-09`). One-person, dormant-but-not-abandoned, edition 2018.

`[F]` HIGH — D1 §11.1 already found the disqualifying properties: `caps::read()` returns a `HashSet<Capability>` and its error type is `CapsError(String)` — **it allocates on both the success and the failure path**, so it cannot be called between `clone` and `execve`. It also does not cover `PR_SET_NO_NEW_PRIVS`, and its capability table is hardcoded (`CAP_CHECKPOINT_RESTORE = 40`).

`[R]` HIGH — **this is the second correction to D1 §11.1.** D1 concluded "write ~60 lines of `capget`/`capset`/`PR_CAPBSET_DROP`/`PR_CAP_AMBIENT` ourselves". Verified against `rustix-1.1.4/src/thread/`, that is no longer necessary: rustix ships `capabilities()`/`set_capabilities()` (the v3 `capget`/`capset` pair), `capability_is_in_bounding_set()`/`remove_capability_from_bounding_set()`, `capability_is_in_ambient_set()`/`configure_capability_in_ambient_set()`/`clear_ambient_capability_set()`, `set_keep_capabilities()`/`get_keep_capabilities()`, and `capabilities_secure_bits()`/`set_capabilities_secure_bits()`. That is the complete five-set surface D5 needs, in the crate we already adopted, with `CapabilitySet` bitflags instead of an allocating `HashSet`. **Adopting rustix deletes both the `caps` dependency and D1's 60 hand-written lines.**

`[R]` LOW — one caveat I could not settle: rustix 1.1.0 **deprecated** its per-capability `Capability` enum in favour of `CapabilitySet` bitflags (`src/thread/prctl.rs:~358`, `#[deprecated(since = "1.1.0", note = "Use CapabilitySet with a single bit instead")]`). I did not check whether every constant we need (e.g. `CAP_CHECKPOINT_RESTORE`, `CAP_BPF`, `CAP_PERFMON`) is present in the non-deprecated `CapabilitySet`. LOW confidence that the bitflags set is complete; *what would settle it:* `grep -n 'const CAP_' rustix-1.1.4/src/backend/linux_raw/thread/types.rs` and compare against `capabilities(7)`'s list up to `CAP_LAST_CAP` on kernel 6.11. If a constant is missing it is a one-line `CapabilitySet::from_bits_retain`, not a blocker. This is D5's call in the end; the dependency consequence is ours.

`[R]` — **keep `caps` bookmarked for exactly one thing**: its name↔bit table if we want `--cap-drop=CAP_NET_RAW` string parsing on the CLI. That is a table, not a dependency-worthy abstraction; copy it or generate it.

## 12.4 `clap` — adopt, with a deliberately small feature set

`[F]` HIGH — `clap` **4.6.6**, 2026-08-06, 227M dl/90d, `clap-rs/clap`, MSRV **1.85**, edition 2024. Multi-maintainer (`epage` et al.), the de-facto standard.

`[R]` HIGH — **adopt.** Argument parsing hides nothing about containers. It is the single least controversial dependency in this document.

`[R]` MED — feature discipline, because it interacts with §7.3's static-musl single-binary goal:
- Use `default-features = false` and enable only `["std", "derive", "help", "usage", "error-context", "suggestions"]`. Skip `color` and `wrap_help` in phase 1; they pull `anstream`/`anstyle`/`terminal_size` for cosmetics.
- `[C]` HIGH — this is what youki does: its workspace pins `clap = { version = "4.5.13", default-features = false }` and each crate opts in — `crates/youki` takes `["std","suggestions","derive","cargo","help","usage","error-context"]`, `crates/libcgroups` takes a smaller set for its dev-dependency, and `crates/liboci-cli` takes `["std","suggestions","derive","cargo","help","usage","error-context"]`. Nobody enables `color`.
- `[R]` HIGH — **clap lives in the CLI subcommand only.** Not in the supervisor's argv handling, not in the child. §8.3's structural rule is that the process which calls `clone()` links as little as possible; the supervisor is invoked by us, with arguments we chose, so it should parse `argv` with a `match` over 2–3 fixed positions and no parser at all. That is five lines and it removes a whole crate from the cloning process's dependency graph.

`[F]` HIGH — MSRV 1.85 (edition 2024) is the highest MSRV of anything here and effectively sets our toolchain floor. That is fine — our dev target is a VM we control — but it is worth writing down so it does not surface as a surprise on an older builder.

## 12.5 `serde` / `serde_json` — adopt, and be deliberate about where

`[F]` HIGH — `serde` 1.0.229 (2026-07-18, 296M/90d, MSRV 1.56), `serde_json` 1.0.151 (2026-07-20, 300M/90d, MSRV 1.71). `serde-rs`, dtolnay + team. No health question exists.

`[R]` HIGH — **adopt, for exactly two things: parsing `config.json` (§1.3) and reading/writing `state.json` (§6.1).** Both are on-disk JSON schemas defined by someone else; hand-rolling a JSON parser teaches nothing about containers and is a bug farm on the exact path (state durability, §6.5) where bugs are least recoverable.

`[R]` HIGH — **and be deliberate about the boundary.** §7.5 mandates that one module owns all state writes. That module is the only place `serde_json` is called. The child never touches it — the child receives a `#[repr(C)]` fixed-size struct over a socketpair (§10.3's recommendation), not JSON, precisely because deserialising JSON in the cloned child would allocate.

`[R]` MED — `state.json` durability (§6.5) needs write-to-temp + `fsync` + `rename` + `fsync(dir)`. `serde_json::to_writer` gives us the bytes; the atomicity is ours. Do not reach for a crate (`atomicwrites`, `tempfile::persist`) for four syscalls we should understand — this is squarely the case where the mechanic *is* the lesson.

## 12.6 `thiserror` / `anyhow` — both, at the boundary §9.3 already drew

`[F]` HIGH — `thiserror` 2.0.20 (2026-08-08, 350M/90d), `anyhow` 1.0.104 (2026-07-18, 206M/90d). Both dtolnay. `thiserror` is a proc-macro that expands to `impl Display`/`impl Error` — **zero runtime dependency**, nothing in the binary that would not be there if we wrote the impls by hand. `anyhow` is a runtime type: a boxed, type-erased error with an optional backtrace.

`[R]` HIGH — **`thiserror` in the library/core; `anyhow` in the CLI's `main` only.** That maps exactly onto §9.3's error model: the create path needs typed errors because §9.1's cleanup logic must *branch on which step failed* to know what to unwind, and a type-erased `anyhow::Error` cannot be matched on. The CLI's top level needs none of that — it needs a message and an exit code.

`[R]` HIGH — **neither may appear in the cloned child.** `thiserror`'s generated `Display` impl formats, which allocates; `anyhow::Error` boxes on construction. §10.3 already fixes the child's error channel as a fixed-size `#[repr(C)]` struct written with one `write(2)` (crun's `struct sync_socket_message_s {type, error_value, char[512]}` is the model). The child's "error type" is an errno plus a small integer step code plus a fixed byte array — no `Result<_, MyError>` with a `String` in it, ever. `[R]` HIGH — **make this a type-level rule, not a comment**: the child's functions return `Result<T, ChildErr>` where `ChildErr` is `Copy` and `#[repr(C)]`, so a `thiserror` type physically cannot flow there.

`[C]` HIGH — youki does exactly this split: `crates/libcontainer` and `crates/libcgroups` depend on `thiserror` and **not** `anyhow`; `crates/youki` (the binary) depends on `anyhow` and both libraries. `anyhow` appears in the libraries' `[dev-dependencies]` only, i.e. in tests. Independent convergence on the same boundary.

## 12.7 `tracing` — adopt in the supervisor and CLI, ban it in the child, and re-check the deadlock claim

`[F]` HIGH — `tracing` 0.1.44 (2025-12-18, 185M/90d), `tracing-subscriber` 0.3.23 (2026-03-13, 140M/90d), both `tokio-rs/tracing`, MSRV 1.65.

`[R]` HIGH — **adopt for the CLI and the supervisor.** A container runtime whose failures are "step 12 of 15 failed, here is the errno and here is what step 11 had already created" (§9.1) is unusable without structured, levelled, per-container logging, and `tracing`'s span model maps naturally onto "this event happened inside container `<id>`'s create sequence".

`[R]` HIGH — **absolutely not in the cloned child.** §8.3 already states this and the reasoning is D1 §11.2's: a subscriber holds locks and allocates, and a lock held by another thread at clone time is held forever. §8.3 leaves one thing MED — *whether `tracing` specifically deadlocks post-fork* — and flags "read `tracing-subscriber`'s `fmt` layer for its `Mutex`/`RwLock` usage" as the settling experiment. **I did not do that read.** It stays open as §14 question 5. `[R]` HIGH regardless — the *design* answer is unchanged and does not depend on the outcome: **the child gets a raw fd and writes pre-formatted bytes into it** (runc's `_LIBCONTAINER_LOGPIPE`, conmon's and crun's equivalent). Every implementation that has built this converged there, so the question is worth answering for our own understanding, not for the decision.

`[R]` MED — **`tracing-appender` is banned outright, not merely discouraged.** Its `non_blocking` writer **spawns a background thread**, which would make the supervisor multithreaded — and the supervisor is the process that calls `clone()`. That violates §8.3's rule ("the process that calls `clone()` never has more than one thread, ever") structurally, not stylistically. `[R]` HIGH — the supervisor writes its own log file with plain buffered `std::fs::File` writes, or emits to stderr which the parent captured. MED only on whether `tracing-appender`'s *blocking* (non-`non_blocking`) writer is thread-free; HIGH that we should not find out.

`[R]` HIGH — **and assert it.** D1 §3.4 recommends asserting `Threads == 1` from `/proc/self/status` immediately before the clone. That assertion is the enforcement mechanism for every rule in §12.4–12.7: it catches a dependency that started a thread behind our back, at the exact instant it matters, with a clear message — instead of a one-in-a-thousand deadlock in the container's init path. `[R]` HIGH — **make it a hard assert in release builds, not a `debug_assert!`.** The cost is one `read` of a small procfs file per container creation. The alternative is §8.3's "worst possible failure signature".

## 12.8 The youki ecosystem — `libcontainer`, `libcgroups`, `liboci-cli`

These three are published crates, not just source in a repo, so "could we depend on them?" is a real question with a real answer. The answer is different for each, and the distinction between **depending on** and **reading as a reference** is the whole point.

`[F]` HIGH — fetched 2026-09-05. All three are version **0.7.0**, published 2026-07-25, edition **2024**, MSRV **1.85.0**, repository `youki-dev/youki`, license Apache-2.0.

| crate | 90-day dl | total dl | crates.io owner | size (`main`) |
|---|---|---|---|---|
| `libcontainer` | 18,350 | 122,015 | **`utam0k` alone** | 69 files, 825 KiB of `.rs` |
| `libcgroups` | 41,662 | 187,135 | (same repo) | 66 files, 453 KiB |
| `liboci-cli` | 2,578 | 141,930 | (same repo) | 18 files, 16 KiB |

`[F]` MED — the download numbers are the story. 18K/90d for `libcontainer` is roughly **1/240th** of `oci-spec`'s and **1/13,000th** of `rustix`'s. Essentially nobody outside youki itself depends on `libcontainer`. MED on the interpretation (downloads include CI and mirrors), HIGH on the raw figures.

### `libcontainer` — `[R]` HIGH: **read as a reference. Never depend on it.**

**Depending on it would defeat the project's entire purpose, and not in a hand-wavy way.** Its public API is `ContainerBuilder::new(id, syscall).with_root_path(p)?.with_pid_file(p)?.with_console_socket(p).with_stdin/out/err(fd).as_init(bundle).build()` (`crates/libcontainer/src/container/builder.rs`, 569 lines). That builder *is* §4's creation sequence, §3's create/start split, §5's console-socket protocol and §10.2's three-process shape — the exact four things this document spent 1,300 lines establishing. Taking the dependency means the project is "a CLI over youki", which is precisely the runc-shelling-out shape the brief rules out, one language up.

`[F]` HIGH — and it is architecturally explicit about it: `crates/libcontainer/src/syscall/syscall.rs` defines `pub trait Syscall` with ~30 methods — `pivot_rootfs`, `set_ns`, `set_id`, `unshare`, `set_capability`, `mount`, `mount_from_fd`, `move_mount`, `fsopen`, `fsconfig`, `fsmount`, `open_tree`, `mknod`, `symlink`, `set_rlimit`, `get_pwuid`, … — documented as *"An interface trait so that rest of Youki can call necessary functions without having to worry about their implementation details."* **"Without having to worry about their implementation details" is a verbatim statement of what we are here to worry about.** It is a good abstraction for youki and exactly the wrong one for us.

`[R]` HIGH — **read it, extensively, and here is what specifically repays reading** (all confirmed to exist at `main` on 2026-09-05):
- `src/process/fork.rs` — `clone3` primary with a `libc::clone` + mmap'd-stack + guard-page fallback, and the "double boxing" trick for closure ownership. §10.2 already mines this; it is the single highest-value file in the repo for us.
- `src/process/container_main_process.rs` and `container_intermediate_process.rs` — the four-channel sync protocol (§10.2).
- `src/process/init/process.rs` — the child's ordered setup, and the `notify_listener.wait_for_container_start()` rendezvous.
- `src/rootfs/rootfs.rs` — mount ordering (D1 §6).
- `src/syscall/linux.rs` — how they hand-write `mount_setattr`, `open_tree`, `fsopen`/`fsconfig`/`fsmount`/`move_mount`. Note they do this via the `nc` crate rather than rustix; we get the same calls from rustix (except `mount_setattr`, §12.1.2).
- `src/container/builder.rs` + `builder_impl.rs` — the shape of the state machine, useful as a check on §7.2.

`[O]` MED — a design observation from that trait worth carrying: **every path-taking method takes `&Path` or `&str`, not `&CStr`.** Combined with §12.1.3's finding that `nix`'s `NixPath` allocates past 1024 bytes, youki's child-side syscalls are allocation-free only by luck of path length. I did not test whether youki actually deadlocks with a >1024-byte bundle path — MED, and *what would settle it:* on the dev VM, create a bundle under a ~1,200-character path and run `youki create`. Whether or not youki has a bug there, it is a concrete argument for §12.1.3's `&CStr`-in-signatures rule: **we should make the type system enforce what youki enforces by convention.**

### `libcgroups` — `[R]` HIGH: **do not depend. Formally D5's call; the dependency cost is ours.**

`[F]` HIGH — same reasoning D1 §11.1 applied to `cgroups-rs`: its value is v1 support and the **systemd** cgroup driver, and we want neither (root-first, cgroup-v2-preferred, no systemd coupling). Its `systemd` feature is on by **default** and pulls `nix/socket` + `nix/uio` for D-Bus. cgroup v2 for our purposes is `mkdir`, `std::fs::write` to `cgroup.subtree_control`/`cgroup.procs`/`memory.max`, `cgroup.kill`, and reading `memory.events` — plus `clone3(CLONE_INTO_CGROUP)`, which is a syscall flag, not a library. That is a file-writing module we should own, and §7.5 already says the cgroup module should sit behind a trait from day one so v1 remains a later option.

### `liboci-cli` — `[R]` MED: **do not depend, but read it once and steal its shape.**

`[F]` HIGH — it is 16 KiB and its *entire* dependency list is `clap`. It is nothing but clap `derive` structs for the OCI runtime CLI: `StandardCmd` (`create`/`start`/`state`/`kill`/`delete`), `CommonCmd`, `GlobalOpts` (`--root`, `--log`, `--log-format`, …). No logic.

`[R]` MED — **don't depend, do read.** §7.5 says "make the internal API literally those five operations plus `exec`, from day one", and `liboci-cli`'s type definitions are a free, authoritative checklist of every flag runc accepts for each of them. Copying a 16 KiB set of struct definitions after reading them is cheaper than an extra crate in the graph and gives the same benefit. The only argument for depending is guaranteed drift-tracking of the *unofficial* runc CLI surface (which the spec explicitly does not standardise — §1.2), and at phase 3 that is worth re-examining, not now.

`[R]` HIGH — **the general rule this section establishes:** *depend on crates that hide typing; read crates that hide mechanism.* `oci-spec` hides typing (6,360 lines of serde structs) → adopt. `libcontainer` hides mechanism (a `Syscall` trait whose docstring says so) → read. That single test resolves every row of §12.13's table.

## 12.9 What youki's actual `Cargo.toml` depends on, and what it tells us

`[O]` HIGH — fetched 2026-09-05 from `raw.githubusercontent.com/youki-dev/youki/main/{Cargo.toml,crates/libcontainer/Cargo.toml,crates/libcgroups/Cargo.toml,crates/liboci-cli/Cargo.toml,crates/youki/Cargo.toml}`. This **confirms and extends** §8.2's `[C]` MED claim, which was based on an earlier fetch.

**Workspace pins (`[workspace.dependencies]`), abridged to what matters:**
`anyhow 1.0.102`, `caps 0.5.6`, `chrono 0.4` (default-features off), `clap 4.5.13` (**default-features off**), `crossbeam 0.8.4`, `errno 0.3.14`, `fastrand ^2.3.0`, `libc 0.2.186`, `libseccomp 0.4.0`, `nc 0.9.8`, `netlink-packet-core 0.9.0` / `netlink-packet-route 0.33.0` / `netlink-sys 0.9.0`, **`nix 0.29.0`**, `oci-spec 0.10.0 (features=["runtime"])`, `pathrs 0.2.4`, **`pentacle 1.1.0`**, `prctl 1.0.0`, `procfs 0.18.0`, `regex 1.12.4` (default-features off), `rust-criu 0.6.1`, `safe-path 0.1.0`, `scopeguard 1.2.0`, `serde 1.0`, `serde_json 1.0`, `thiserror 2.0.18`, **`tokio 1 (features=["rt-multi-thread"])`**, `tracing 0.1.44`, `tracing-journald 0.3.2`, `tracing-subscriber 0.3.23`, plus wasm runtimes (`wasmer`, `wasmedge-sdk`, `wasmtime`, `wasi-common`) and build tooling.

**Seven things this tells us:**

1. `[C]` HIGH — **`libcontainer` still has no `tokio`, no async, nothing that spawns a thread.** Its full runtime dependency list: `caps, chrono, fastrand, libc, nix, oci-spec, procfs, prctl, libcgroups, libseccomp(optional), serde, serde_json, rust-criu, regex, thiserror, tracing, safe-path, nc, netlink-packet-route, netlink-sys, netlink-packet-core, pathrs`. `tokio` appears **only** in `crates/youki`, and there only as `optional = true` gated behind the `wasm-wasmer` feature. **This directly corroborates §8.3's central rule** — the most mature Rust OCI runtime keeps async out of the container-creation crate entirely, and admits it only for a WASM workload feature that has nothing to do with `clone()`.
2. `[C]` HIGH — **`rustix` is absent from the entire youki workspace.** youki is `nix`-based (0.29.0, two minor versions behind current 0.31.3) and hand-writes the new mount API through `nc`. `[R]` MED — this is the one place we are deliberately diverging from the most mature Rust reference implementation, so it deserves to be stated as a risk rather than glossed: **we are betting on rustix's coverage (§12.1.2) against youki's proven-in-production nix stack.** The bet is defensible — rustix gives us pidfds, the capability surface, the non-broadcasting `setresuid`, the new mount API and no-libc syscalls, all of which youki either hand-writes or does without — but it means we cannot cross-check our syscall wrappers against youki line for line. That is a real cost.
3. `[C]` HIGH — **`nix` is pinned per-crate with minimal features**, not blanket-enabled: `libcontainer` takes `["socket","sched","mount","mman","resource","dir","term","hostname","personality"]`, `libcgroups` takes `["signal","user","fs"]` (+`socket`,`uio` under `systemd`), `crates/youki` takes `["event","poll"]`. `[R]` — feature-minimalism on the substrate crate is standard practice here; do the same with rustix's feature list.
4. `[O]` HIGH — **`pentacle 1.1.0` is in `crates/youki`'s dependencies, and `crates/youki/src/main.rs:76` calls `pentacle::ensure_sealed()`**, with a comment at lines 68–70 reading *"…in runc and was assigned as CVE-2019-5736, but it also affects youki. The fix is to copy /proc/self/exe in an anonymous file descriptor (created via memfd_create)…"*. **This resolves §11.8's `[C]` LOW claim that "youki appears not to have this" — it does have it; the earlier grep looked in `libcontainer`, but the mitigation lives in the *binary* crate, which is the correct place for it.** `[R]` HIGH — §11.8's recommendation (do the memfd self-seal early) is unchanged and now has three implementations behind it, not two.
5. `[R]` MED — **but do not adopt `pentacle` itself.** 962 lines total (`src/file.rs` 573, `src/syscall.rs` 180, `src/lib.rs` 164), last release **2024-10-03**, repo `haha-business/pentacle` last pushed the same day, 45 stars, crates.io owner **`iliana` alone**, 15,803 dl/90d. A one-person, two-years-untouched crate performing a security-critical `memfd_create` + copy + `F_SEAL_*` + re-exec is a strange thing to take on faith. §11.8 already prices it at "roughly 60 lines in Rust", we have `rustix::fs::memfd_create` and `fcntl_add_seals`, and this is a mechanic we want visible. **Read `pentacle/src/file.rs` for the sealing flag order, then write it.** (Not a criticism of the crate — it is doing a narrow job and appears to do it correctly. It is a statement about where our dependency risk should sit.)
6. `[C]` MED — **`prctl 1.0.0` was last published 2015-10-13** — eleven years — and youki depends on it anyway (1.75M dl/90d, so it is widely load-bearing). `[R]` HIGH — **we do not need it at all**: rustix covers `PR_SET_CHILD_SUBREAPER`, `PR_SET_NO_NEW_PRIVS`, `PR_SET_PDEATHSIG`, `PR_SET_DUMPABLE`, `PR_CAPBSET_*` and `PR_CAP_AMBIENT_*` with typed wrappers (§12.1.2). One more crate deleted by the rustix choice.
7. `[C]` HIGH — **`libseccomp 0.4.0` is C FFI**, and it has a consequence for §7.3. See §12.11.

`[R]` HIGH — **the meta-lesson.** youki's dependency graph is a working existence proof of §8's boundary: the crate that calls `clone()` has *no* async, *no* thread-spawning dependency, and a deliberately narrow feature surface, while the binary crate on top is free to pull `tokio`, three WASM runtimes and `tracing-subscriber`. **That is §8.3's rule, discovered independently, shipped, and CNCF-sandboxed.** It is the strongest external validation §7 and §8 are going to get.

## 12.10 Process supervision and Unix socket crates — does `std` + `rustix` suffice?

The honest framing: §7.2's supervisor is `setsid`, `PR_SET_CHILD_SUBREAPER`, `clone3`, a `poll` loop over 3–5 fds, `waitpid`, and one `flock`. The question is whether any crate improves on that.

**Daemonization — `daemonize` 0.5.0.** `[F]` HIGH — last published **2023-02-25**, 3.1M dl/90d, `knsd/daemonize`, no declared edition. `[R]` HIGH — **do not use.** §5.1 and §7.2 already specify our detach sequence precisely (fork → `setsid` → the CLI reaps the intermediate → subreaper → handshake on a pipe), and it is *deliberately different* from the classic SysV daemonize recipe: we must **not** close 0/1/2 (the supervisor holds the container's stdio), we must **not** `chdir("/")` blindly, and we must keep the handshake pipe open past the fork. A daemonization crate encodes the recipe we are explicitly not following, and §5.6 catalogues the ways that goes wrong quietly. Roughly 20 lines of ours, fully understood.

**Unix sockets — `std::os::unix::net`.** `[F]` HIGH — `std` gives `UnixListener`, `UnixStream`, `UnixDatagram`, `bind`/`accept`/`connect`, `SocketAddr`, and `UnixStream::pair()`. What `std` does **not** give is `SCM_RIGHTS` — and §5.2's console-socket protocol is *entirely* about passing an fd over a socket.

`[F]` HIGH — `rustix::net` covers the gap: `sendmsg`/`recvmsg` with `SendAncillaryMessage::ScmRights(&[BorrowedFd])`, `RecvAncillaryMessage::ScmRights(AncillaryIter<OwnedFd>)`, the `cmsg_space!` macro for a stack-allocated control buffer, and `socketpair()`. `[R]` HIGH — **`std` for the listener/stream, `rustix` for the ancillary data.** `std::os::unix::net::UnixStream` converts to/from `OwnedFd` for free, so the two compose with no glue.

`[R]` HIGH — therefore **skip `passfd` (0.1.6, 2023-02-24), `sendfd` (0.4.4, edition 2015), `uds` (0.4.2, 2023-12-31) and `command-fds` (0.3.3, google)**. All four exist to paper over `std`'s missing `SCM_RIGHTS`; rustix already covers it, allocation-free, in a crate we have. `command-fds` additionally solves "pass fds through `std::process::Command`" — a problem we do not have, because §7.2's supervisor uses `clone3` directly, not `Command`.

`[R]` MED — **`ControlMessageOwned::ScmRights(Vec<RawFd>)` is a second, quieter reason to prefer rustix over nix here.** nix's receive path hands you a `Vec<RawFd>`: it allocates, and it hands you *raw* fds with no ownership, so a lost `Vec` leaks the fd. rustix hands you `OwnedFd`s. In a program whose named CVE class is "leaked host fd into a container" (CVE-2024-21626, D1 §11.2), the ownership-typed version is the correct default. MED only because the console-socket receive happens in the CLI/supervisor, not post-clone, so the allocation is harmless there — the argument is about fd hygiene, not fork safety.

**Signals — `signal-hook` 0.4.4.** `[F]` HIGH — 52.8M dl/90d, maintained (`vorner/signal-hook`), and genuinely good at what it does: making async-signal-unsafe work possible from a signal handler. `[R]` HIGH — **we do not want a signal handler at all.** §7.2 and §5.5 both route signals through `signalfd` (or, per §12.1.7, a pidfd) into a `poll` loop, so signals arrive as *readable file descriptors*, not as interrupts. That is the whole point: no handler means no async-signal-safety problem, no self-pipe trick, no `signal-hook`. If we ever need a handler in the CLI (`Ctrl-C` during a foreground attach in phase 2), `rustix::runtime` is `doc(hidden)` and off-limits (D1 §11.1), so that is a `libc::sigaction` call — and by then it is one handler that sets one `AtomicBool`.

**File locking — `flock(2)`.** `[F]` HIGH — §6.6 makes per-container `flock` the concurrency mechanism, and §7.5 notes a future daemon would reuse it. `[R]` HIGH — `rustix::fs::flock` (or `std`'s `File::lock`/`try_lock`, stabilised in Rust 1.89) covers it. **No `fs2`/`fd-lock`/`file-lock` crate.** The subtlety §6.6 depends on — that `flock` locks are per-open-file-description and survive `fork` but not `execve` with `O_CLOEXEC` — is exactly the mechanic we need visible, and a crate that wraps it in a `Guard` type hides the lifetime question we most need to reason about.

`[F]` HIGH — one supporting fact for §12.10's file-locking claim, verified rather than assumed: `File::lock`/`lock_shared`/`try_lock`/`try_lock_shared`/`unlock` stabilised in **Rust 1.89.0 (2025-08-07)** (`rust-lang/rust` `RELEASES.md`), and on Linux `std`'s implementation is **`libc::flock(fd, LOCK_EX)`** — `library/std/src/sys/fs/unix.rs:1338` — i.e. genuine BSD `flock(2)` with per-open-file-description semantics, **not** POSIX `fcntl` record locks. That matters: §6.6's concurrency argument depends on the lock being held by the open file description (so it survives `fork` and is released when the last fd for that description closes), which POSIX record locks do not give. `std` is therefore safe to use here; it would not have been if it had chosen `F_SETLK`.

**Process supervision generally.** `[R]` HIGH — **no crate.** The supervision logic is `PR_SET_CHILD_SUBREAPER` + a `poll` loop + `waitpid(-1, WNOHANG)` + writing an exit code to `state.json`. §5.4 proves empirically that this is *the* thing our engine must own; §7.1 lists the supervisor as one of only two components we build ourselves. Any crate here is a crate that owns our core.

`[R]` HIGH — **net result: `std` + `rustix` suffices for the entire supervisor and every IPC path in §5, §6 and §7, with zero additional crates.** That is not a coincidence — §7's architecture was chosen partly *because* it needs so little. Every crate in this subsection would be solving a problem the architecture does not have.

## 12.11 The seccomp substrate — flagged here because it constrains §7.3

Formally D5's decision. Raised here because it collides with a §7.3 `[R]` HIGH.

`[F]` HIGH — **neither `nix` nor `rustix` can install a seccomp BPF filter.** rustix has `thread::secure_computing_mode()`/`set_secure_computing_mode()`, which is `prctl(PR_GET_SECCOMP)`/`prctl(PR_SET_SECCOMP)` with a mode enum — verified at `src/thread/prctl.rs:107-176`, `SECCOMP_MODE_{DISABLED,STRICT,FILTER}`. There is no `seccomp(2)` / `SECCOMP_SET_MODE_FILTER` path, no `sock_fprog` marshalling, no `SECCOMP_FILTER_FLAG_NEW_LISTENER`. So the filter comes from a crate or from us.

`[F]` HIGH — the three options, fetched 2026-09-05:

| | version | date | 90d dl | nature |
|---|---|---|---|---|
| `libseccomp` | 0.4.0 | 2025-04-05 | 346K | **C FFI** to `libseccomp.so` via `libseccomp-sys`; what youki uses |
| `seccompiler` | 0.5.0 | 2025-03-07 | **5.75M** | **pure Rust** BPF assembler; `rust-vmm`, i.e. Firecracker's |
| `syscallz` / `extrasafe` | 0.17.0 / 0.5.1 | 2023 / 2024 | 4.3K / 7.4K | thin wrappers over `libseccomp`; low usage |

`[O]` HIGH — **`host-a`, `ldd /usr/bin/runc`: `libseccomp.so.2 => /lib/x86_64-linux-gnu/libseccomp.so.2` and `libc.so.6`; `file /usr/bin/runc` → "dynamically linked, interpreter /lib64/ld-linux-x86-64.so.2".** By contrast `file /usr/bin/containerd-shim-runc-v2` → "statically linked". So **the reason runc is the one dynamically-linked binary in the chain is, at least in part, libseccomp.** §7.3 flags runc-being-dynamic as "a known wart" and recommends `x86_64-unknown-linux-musl` static for us; taking `libseccomp` would recreate the exact wart §7.3 tells us to avoid, or force a static-libseccomp build with `pkg-config`, a C toolchain and a musl cross-build of a C library.

`[R]` MED — **prefer `seccompiler`, or write the `sock_filter` array ourselves.** Reasons: it keeps the single-static-binary property §7.3 argues for; it removes a C build dependency from a project that otherwise has none; 5.75M dl/90d and rust-vmm/Firecracker provenance is stronger maintenance evidence than `libseccomp`'s 346K; and a BPF program that is an array of `sock_filter` structs is *more* visible than one built by a C library's opaque `scmp_filter_ctx`, which suits a learning project. **Tradeoff, stated:** `libseccomp` handles multi-architecture syscall-number translation and the `scmp_arch` machinery for free, and the OCI `linuxSeccomp` schema (`architectures`, `defaultAction`, `syscalls[].names`) maps onto its API almost one-to-one; with `seccompiler` we own that translation. Since D1 fixes our target as x86_64-first, the multi-arch value is small now and real later. `[R]` — **D5 owns the final call; the constraint D5 needs from us is that §7.3's static-single-binary recommendation makes C FFI expensive, and that is the deciding input, not ergonomics.**

## 12.12 The maintainership re-check — what actually changed, and what I could not confirm

The previous research run flagged an unverified repo-ownership change among these crates and did not confirm it before ending. Re-checked directly against the GitHub API on 2026-09-05, not from memory:

`[F]` HIGH — **`github.com/containers/youki` now redirects to `github.com/youki-dev/youki`.** Verified: `GET /repos/containers/youki` returns `"full_name": "youki-dev/youki"` (GitHub's rename redirect), `created_at 2021-03-27`, `pushed_at 2026-09-05`, `archived: false`, 7,577 stars, 145 open issues. The repository moved out of the **`containers`** GitHub organisation (the home of Podman, CRI-O, Buildah, skopeo) into a standalone **`youki-dev`** organisation.

`[F]` HIGH — **the same move happened to `oci-spec-rs`:** `GET /repos/containers/oci-spec-rs` returns `"full_name": "youki-dev/oci-spec-rs"`, `pushed_at 2026-09-04`, `archived: false`, 296 stars. The `oci-spec` crate's `repository` metadata on crates.io already points at `https://github.com/youki-dev/oci-spec-rs`, and `libcontainer` 0.7.0's `Cargo.toml` carries `repository = "https://github.com/youki-dev/youki"` and `homepage = "https://youki-dev.github.io/youki/"` — so the move is reflected in published crate metadata, not just in GitHub redirects.

`[F]` HIGH — **youki remains a CNCF Sandbox project.** `README.md` at `youki-dev/youki@main` line 289: *"**We are a [Cloud Native Computing Foundation](https://cncf.io/) sandbox project.**"* So this is a change of GitHub org, not a loss of foundation governance.

`[R]` LOW — **why it moved, I could not establish.** I found no announcement, migration note, or governance document explaining the move; the README does not mention it and I did not locate a blog post or CNCF filing. Plausible readings range from "routine org tidy-up after CNCF sandbox acceptance gave youki its own governance" to something less benign, and **I am not going to guess between them.** *What would settle it:* the youki GitHub Discussions or the CNCF sandbox application PR for youki (`cncf/sandbox` repo), or the git history of `README.md`/`GOVERNANCE.md` around the rename date. Flagged as §14 question 6.

`[R]` MED — **the practical impact on us is small and I want to say why rather than wave it away.** We depend on **`oci-spec` only**, and only from phase 3 (§12.2). That crate has three crates.io owners across three unrelated employers (`saschagrunert` — CRI-O, `flavio` — SUSE, `utam0k` — youki), 40 commits in the last 180 days from six-plus humans, and is consumed by CRI-O and Kubewarden as well as youki. Its health does not rest on the youki org. The crates that *do* rest on one person's publish key — `libcontainer` (`utam0k`), `rustix` (`sunfishcode`), `caps` (`lucab`), `pentacle` (`iliana`) — are, with one exception, ones this section recommends **not** depending on. **The exception is `rustix`, and §12.1.5 prices it explicitly.**

`[R]` MED — one further ownership note worth recording because it is the same *shape* of finding: **the `tar` crate's repository metadata now reads `https://github.com/composefs/tar-rs`** (crates.io, `tar` 0.4.46, 2026-05-18). `tar-rs` was long `alexcrichton/tar-rs`. This is D3's dependency, not ours, so I did not chase it — but **D3 should verify it rather than assume**, since layer extraction is D3's most security-sensitive code path. Flagged in §13.

`[R]` HIGH — **the discipline this establishes, and it should outlive this document:** every "X is maintained by Y" claim here was re-derived from `api.github.com` and `crates.io/api` on 2026-09-05 and is dated. **Re-run the checks before writing any `Cargo.toml`.** Maintainership is the one property in this section with a short half-life; syscall coverage is not going to change under us, but a publish key can.


## 12.13 Decision table

Verdicts: **adopt** = put it in `Cargo.toml` · **read** = study the source, write our own · **write** = write ours, no equivalent crate worth reading · **defer** = revisit at a named phase.

| Crate | Verdict | One-line reason |
|---|---|---|
| `rustix` | **adopt** (primary substrate) | Only crate with `linux_raw` no-libc syscalls, pidfds, the full capability/prctl surface, non-broadcasting `setresuid` and the new mount API — all four matter post-`clone`. |
| `libc` | **adopt** (types & numbers only) | Supplies `SYS_*`, `clone_args`, `mount_attr`, `open_how`, `signalfd_siginfo` and `syscall()`; nothing else can, and no wrapper we want. |
| `nix` | **defer** (probably never) | Its only unique win we need is `signalfd`; try a pidfd-based loop (§14 Q3) or a 20-line hand-written `signalfd4` first. Also carries the 2016-open `sched::clone` soundness bug. |
| raw `libc` **for everything** | **rejected** | ~1000 lines of our own `unsafe` teaching nothing about containers, routed through glibc — losing the one property that helps our worst failure mode. |
| `oci-spec` (`features=["runtime"]`) | **adopt at phase 3** (defer) | 6,360 lines of serde structs, 3 `unsafe` in the crate, hides typing not mechanism; 3 crates.io owners across 3 orgs. Hand-write a ~40-field subset for phases 0–2. Do **not** use its `VERSION` const — it says 1.1.0. |
| `caps` | **write** (superseded) | Allocates (`HashSet`, `CapsError(String)`) so unusable post-clone; one maintainer, 0 commits/180d; **rustix already covers all five capability sets**. Keep only its name↔bit table. |
| `clap` | **adopt** (CLI subcommand only) | Argument parsing hides nothing. `default-features = false` + the six features youki uses; never in the supervisor or the child. Sets our MSRV floor at 1.85. |
| `serde` + `serde_json` | **adopt** (state & config module only) | `config.json` and `state.json` are someone else's on-disk schemas; a hand-rolled parser is a bug farm on the least recoverable path. Never in the child. |
| `thiserror` | **adopt** (library/core) | Proc-macro, zero runtime footprint; §9.1's cleanup must branch on which step failed, which needs typed errors. |
| `anyhow` | **adopt** (CLI `main` only) | Type-erased is right where you only need a message and an exit code; matches youki's own library/binary split exactly. |
| both, in the cloned child | **rejected** | Formatting and boxing allocate; the child's error channel is a `Copy`, `#[repr(C)]` struct written with one `write(2)` (§10.3). |
| `tracing` | **adopt** (CLI + supervisor) | Span-per-container is the only sane way to debug a 15-step privileged sequence. |
| `tracing` in the child | **rejected** | Subscriber locks + allocation across `clone` (D1 §11.2); child gets a raw fd and pre-formatted bytes, as runc/crun/conmon all do. |
| `tracing-appender` | **rejected** | `non_blocking` spawns a thread; the supervisor is the process that calls `clone()`. Structural violation of §8.3, not a style choice. |
| `libcontainer` (youki) | **read** | Its `Syscall` trait's own docstring — *"without having to worry about their implementation details"* — is a statement of exactly what we came to worry about. Depending on it makes us a CLI over youki. |
| `libcgroups` (youki) | **read** (D5's formal call) | Value is cgroup v1 + the systemd/D-Bus driver, both of which we want to *not* have; cgroup v2 is `mkdir` + `write`. |
| `liboci-cli` (youki) | **read** (revisit at phase 3) | 16 KiB of clap derive structs; a free checklist of runc's flags, cheaper to copy after reading than to depend on. |
| `pentacle` | **read**, then **write** | One maintainer, untouched since 2024-10-03; ~60 lines of `memfd_create` + `F_SEAL_*` + re-exec is a security mechanic we want visible (§11.8). Read `src/file.rs` for the seal ordering. |
| `prctl` | **write** (unnecessary) | Last published 2015; every `prctl` we need is already a typed rustix function. |
| `cgroups-rs` | **rejected** (D1 §11.1 stands) | Drags `zbus`, `strum`, `uuid` and two versions of `nix` for systemd delegation we do not want. |
| `daemonize` | **write** | Encodes the classic recipe we deliberately do **not** follow (we keep 0/1/2 and the handshake pipe); §5.1/§7.2 already specify ours in ~20 lines. |
| `passfd` / `sendfd` / `uds` / `command-fds` | **rejected** | All exist to patch `std`'s missing `SCM_RIGHTS`; `rustix::net` covers it allocation-free with `OwnedFd` ownership, and we never use `std::process::Command`. |
| `signal-hook` | **rejected** | We want no signal handler at all — signals arrive as a readable fd (`signalfd`/pidfd) in a `poll` loop. |
| `fs2` / `fd-lock` / `file-lock` | **rejected** | `std::fs::File::lock` is `flock(2)` on Linux (verified) with the per-OFD semantics §6.6 depends on; a `Guard` wrapper hides the lifetime question we most need to reason about. |
| `libseccomp` | **defer** → likely **reject** (D5 owns it) | C FFI is what makes runc the one dynamically-linked binary in the chain (`[O]` `host-a`), defeating §7.3's static-musl single binary. |
| `seccompiler` | **defer** → likely **adopt** (D5 owns it) | Pure Rust, rust-vmm/Firecracker, 5.75M dl/90d, keeps the static-binary property; cost is owning multi-arch syscall-number translation ourselves. |
| `procfs` | **defer** (narrow use only) | Fine for a `state`/`ps` subcommand; D1 §11.1's caveat stands — parse `/proc/self/mountinfo` and `/proc/self/status` by hand, those two are the lesson. |
| `syscalls` / `nc` / `linux-raw-sys` (direct) | **defer** | Attractive typed `Sysno` enums, but `libc` + `rustix` already covers everything and a third syscall-numbering source is a third thing to keep in sync. |
| `sc` | **rejected** | Stale since 2022 (D1 §11.1). |
| `tokio` | **adopt — in the `pull` process only** | §8.3 makes this structural, not disciplinary: the pull path is a separate process invocation, so the supervisor never links it. |
| `rayon` / thread pools | **adopt — in the `pull`/extract process only** | Same rule. Never in any process that will call `clone()`. |

`[R]` HIGH — **the shape of the answer.** Phase 1's entire `Cargo.toml` is plausibly **six entries**: `rustix`, `libc`, `clap`, `serde`, `serde_json`, `thiserror` (+`anyhow`, `tracing` for the binary). Everything genuinely container-shaped — clone, exec, mount ordering, the supervisor, the sync protocol, cgroup v2, state, capabilities policy, seccomp policy — is ours. That is not asceticism; it falls out of one test applied consistently: **depend on what hides typing, write what hides mechanism.**

---

# 13. Handoff

## To D1 (kernel isolation) — complete, but three things flow back

D1 is finished, so this is corrections and confirmations rather than requests. All three concern D1 §11, which was written before this crate assessment existed and closes with open question 9 (*"Delegated crate assessment had not returned at write time"*) — **this section is that assessment; §12 supersedes D1 §11.1's recommended stack.**

1. **D1 §11.1's "write ~60 lines of `capget`/`capset`/`PR_CAPBSET_DROP`/`PR_CAP_AMBIENT` ourselves" is no longer necessary.** rustix 1.1.4 ships the complete five-set capability surface plus the prctl family (§12.1.2, §12.3). D1 reached its conclusion by assessing the `caps` crate; it did not look in `rustix::thread`.
2. **D1 §11.2's glibc-`setxid`-broadcast hazard has a named, typed solution.** `rustix::thread::set_thread_res_uid` / `set_thread_res_gid` / `set_thread_groups` are the raw per-thread syscalls, not glibc's signal-broadcasting `setxid` family. D1 prescribed hand-written raw syscalls with a youki-style safety comment; we can use the crate instead, and the safety comment still belongs on the call.
3. **D1 §11.2's no-allocation rule needs a type-level enforcement mechanism, and §12.1.3 supplies the measurement it was missing.** Both `nix` (1024-byte stack buffer, `src/lib.rs:319-365`) and `rustix` (256-byte, `src/path/arg.rs:952-997`) **fall back to `CString::new` for long paths** — so "use a safe wrapper" is not by itself compliance with D1's rule. The fix is D1 §10.1 step 7 promoted into the type system: the child's functions take `&CStr` and nothing else. Our state paths (`/var/lib/mydocker/containers/<64-hex>/…`) sit uncomfortably close to rustix's 256-byte threshold.

D1's §11.2 conclusions are otherwise confirmed and load-bearing throughout §8 and §12: no allocation, no `println!`, no panic, no logging framework, no glibc `setuid` between `clone` and `execve`; assert `Threads == 1` before cloning (§12.7 upgrades that to a **release-build** assert, not `debug_assert!`).

## To D3 (images / layers / registries) — not yet written

- **The bundle contract, from our side.** We hand you nothing and you hand us a directory. Concretely: **D3 owns everything up to and including "a rootfs exists at path P and P is a mount point"; D2 owns everything from `clone()` onward.** D1's handoff already states the mount-point requirement (`pivot_root` needs it; if you give us a plain directory we recursively bind it onto itself). Add the D2-side requirement: **that mount must exist before we `clone()`**, because §9.1 step 6 puts it before step 7 and unwinding a mount after a failed clone is §9's worst leak.
- **The pull path is a separate process, and that is architectural, not stylistic.** §8.3 puts `tokio`, `reqwest`, TLS, parallel blob fetch and a thread pool entirely inside a `mydocker pull` process invocation that exits before any supervisor exists. **You may use async freely. You may not be linked into the process that calls `clone()`.** If your design assumes the puller and the runtime share an address space, say so early — it contradicts §8.3's central rule and that rule is §7.5's biggest rewrite-risk item.
- **Verify the `tar` crate's ownership before depending on it.** `[R]` MED — crates.io metadata for `tar` 0.4.46 (2026-05-18) gives `repository = https://github.com/composefs/tar-rs`; this crate was historically `alexcrichton/tar-rs`. I did not investigate. Given that layer extraction is the highest-concentration security-parsing code in the project (path traversal, symlink escape, hardlinks, device nodes, setuid bits — the plan's own D3 brief), **re-derive this from `api.github.com` and `crates.io/api` rather than assuming, and treat the answer as dated** (§12.12's discipline).
- **Two `[O]` observations from `host-a` you will want.** (1) `runc state` reports `"rootfs": "/var/lib/docker/overlay2/<hash>/merged"` while the bundle's own `rootfs/` is **empty** — in the Docker path, dockerd mounts overlay and hands runc an absolute `root.path` elsewhere; the shim does *not* mount into the bundle (§2.3). (2) D1 recorded that Docker's live container rootfs mounts overlay with `nouserxattr`.
- **Leak ranking.** §9.1 ranks a leaked rootfs mount as the **worst** failure in the whole engine — worse than a leaked cgroup or a leaked netns — because `mydocker rm` deletes the directories, the mount survives, and `df` disagrees with `du` until reboot. Whatever you design for layer lifetime must have an answer for "the mount outlived the container".
- **`oci-spec`'s image module exists too** (`features = ["image"]`, `["distribution"]`). §12.2's verdict — adopt at phase 3, hand-write a subset before that, never use its `VERSION` constant — was reasoned about the runtime module but the same test applies: it hides typing, not mechanism.

## To D4 (networking) — in progress

- **Where your work happens in the sequence is already fixed, by D1 and by §9.1 step 8.** Netns plumbing is done by the **parent** (our supervisor), after `clone()` returns and before the child drops capabilities. In §7.2's process diagram that is a named slot between "clone" and the child's "drop caps, apply seccomp".
- **The supervisor is single-threaded and must stay that way.** §8.3 is absolute: the process that calls `clone()` never has more than one thread and never links a runtime that could create one. **If your rtnetlink design wants `tokio`/`rtnetlink`-the-async-crate, it cannot live in the supervisor.** Options, in the order I would try them: synchronous `NETLINK_ROUTE` over a plain socket (netlink is request/response — async buys nothing for a handful of `RTM_NEWLINK`s); or, if the netlink work genuinely wants to be async, a separate short-lived process invocation the way §8.3 handles `pull`. `[R]` HIGH — **raise this now if it is a problem, because §7.5 lists "async anywhere in the create path" as the single biggest rewrite risk in the project.** youki's precedent supports the sync route: `libcontainer` uses `netlink-packet-route` + `netlink-sys` with **no tokio** (§12.9).
- **Crate substrate.** §12 did not evaluate `netlink-packet-route` / `netlink-sys` / `rtnetlink` — that is yours. Apply §12's test when you do: **depend on what hides typing (netlink message struct layouts are typing), write what hides mechanism (which messages, in which order, is the lesson).** youki pins `netlink-packet-core 0.9.0`, `netlink-packet-route 0.33.0`, `netlink-sys 0.9.0` and uses the sync API.
- **§9.1's failure-atomicity model is yours to extend.** Step 8 ("move veth into netns, allocate IP, install rules") is the step that mutates the *most* host state and the state least visible to `mydocker ps` — your D4 §9 (host state that outlives our process) is the same problem from the other side. The unwind order and the crash-consistency question (§9.8) should be one design, not two.
- **`--net=host` first.** §11.7's principled-Apptainer list item 5, made concrete: make each namespace an independent flag and get `mydocker run` working with `--net=host` before writing any networking at all. That gives D2 a working engine to test against while D4 is still open, and it is the phasing §7.6 assumes.

## To D5 (cgroups / caps / seccomp / LSM / rootless) — not yet written

- **The capability substrate question is already answered, and it changes D1's advice.** Use `rustix::thread`: `capabilities()`/`set_capabilities()` (v3 `capget`/`capset`), `capability_is_in_bounding_set()`/`remove_capability_from_bounding_set()`, the three ambient-set functions, `set_keep_capabilities()`, `capabilities_secure_bits()`, and `set_no_new_privs()`. **Do not adopt `caps`** — it allocates on both success (`HashSet`) and failure (`CapsError(String)`) paths, so it cannot run post-`clone`, and it has had no commits in 180 days (§12.3). One LOW-confidence loose end for you: whether rustix's non-deprecated `CapabilitySet` bitflags carry every constant up to `CAP_LAST_CAP` on 6.11 (§14 Q4).
- **The seccomp crate choice is yours, but the deciding constraint is ours and it is not ergonomics.** §7.3 recommends a static `x86_64-unknown-linux-musl` single binary. `[O]` HIGH on `host-a`: `ldd /usr/bin/runc` → `libseccomp.so.2`, `libc.so.6`; `file /usr/bin/runc` → "dynamically linked"; `file /usr/bin/containerd-shim-runc-v2` → "statically linked". **C libseccomp is at least part of why runc is the one dynamically-linked binary in the chain.** `seccompiler` (rust-vmm/Firecracker, pure Rust, 5.75M dl/90d) preserves the static-binary property; `libseccomp` (346K dl/90d, C FFI, what youki uses) does not, without a musl cross-build of a C library. The tradeoff `seccompiler` costs us is multi-arch syscall-number translation, which the OCI `linuxSeccomp` schema maps onto `libseccomp` almost 1:1. §12.11.
- **Neither `nix` nor `rustix` can install a BPF filter.** rustix has only `prctl(PR_SET_SECCOMP)` mode 0/1/2 (`src/thread/prctl.rs:107-176`), not `seccomp(2)`/`SECCOMP_SET_MODE_FILTER`. Whatever you choose, the `seccomp(2)` call itself is ours to write.
- **`libcgroups` (youki) and `cgroups-rs`: both rejected on our side, formally your call.** Their value is cgroup v1 plus a systemd/D-Bus driver; §7.5 wants cgroup v2 behind a trait so v1 stays a later option, and `cgroups-rs` drags `zbus`, `strum`, `uuid` and two `nix` versions (D1 §11.1). cgroup v2 as §7.2 uses it is `mkdir`, `write` to `cgroup.subtree_control`/`cgroup.procs`/`memory.max`, `cgroup.kill`, and reading `memory.events`.
- **Two ordering constraints §7.2 already bakes in, that your design must not break.** (a) `clone3(CLONE_INTO_CGROUP)` removes the unconstrained window between clone and `cgroup.procs` — D1 §13 Q1 flags it as unverified end-to-end and §14 Q1 carries that forward. (b) The supervisor may need to be in a known cgroup *before* it forks anything; §11.1 documents conmon's clean solution — conmon blocks reading a start pipe until podman has placed it, because *the caller* knows the cgroup policy. That handshake is strictly better than the supervisor placing itself, and it is cheap to add now.
- **The child's error channel constrains your error types.** §10.3 and §12.6: everything the child reports crosses a socketpair as a fixed-size `#[repr(C)]` `Copy` struct written with one `write(2)`. Capability-drop failures, seccomp-install failures and rlimit failures all report through it. **No `thiserror` type, no `String`, no `anyhow` inside the child.**
- **The supervisor reads `memory.events`' `oom_kill` count on container exit** (§7.2's diagram) before writing the terminal `state.json`. That is the only place OOM is observable after the fact, and §6.5 makes that write the one `fsync` in the whole engine.

## To Phase 3 (the adversarial pass) — where to aim at §12 specifically

`[R]` HIGH — the falsifiable claims in §12, ranked by how much damage a wrong answer does:

1. **"rustix's `linux_raw` backend is the default on our target and issues syscalls without touching libc."** Everything in §12.1.7 rests on it. Falsify by building a trivial binary for `x86_64-unknown-linux-gnu`, `cargo tree` for `libc` under rustix, and `objdump`/`strace` to confirm no libc symbol on the call path.
2. **"rustix covers the full capability + prctl + per-thread-setuid surface."** §12.3 and the D1 correction depend on it. Falsify by compiling each named function against rustix 1.1.4 with our feature set — a function existing in `src/` is not proof it is reachable under the features we enable.
3. **"Both crates allocate past a path-length threshold."** §12.1.3's `&CStr` rule follows from it. Falsify by reading the two cited line ranges, then by an actual long-path call under a heap-instrumented allocator.
4. **"`oci-spec` hides typing, not mechanism."** The whole depend/read test rests on this being a real distinction and not a rationalisation. Falsify by finding behaviour — validation, defaulting, ordering — in `oci-spec`'s runtime module rather than pure schema.
5. **The maintainership numbers.** All dated 2026-09-05 and all re-derivable in ten minutes from two public APIs. §12.12's discipline says re-run them, and Phase 3 is the natural time.

---

# 14. Open questions

Each entry: what is unresolved · why it could not be settled here · the specific experiment or source that would settle it. Ordered roughly by how much a wrong answer costs.

**1. Does rustix's `linux_raw` backend actually keep libc off the syscall path in a real build of ours?**
§12.1.7's primary justification for choosing rustix over nix is that on `x86_64-unknown-linux-*` the default backend issues syscalls by inline `asm!` with no libc call, no PLT, no `__errno_location()`. I verified this from `Cargo.toml`'s `[target.'cfg(...)']` predicates and the presence of `src/backend/linux_raw/`, i.e. **from the crate's configuration, not from a compiled artifact** — and I have no Rust toolchain available (macOS dev machine; `host-a` has none and installing one is forbidden). *Settles it:* on the dev VM, `cargo build` a binary calling `rustix::process::pivot_root`, then `cargo tree -i libc` (libc should be absent, or present only via another crate), `nm -u ./target/.../binary | grep -c ' U '`, and `strace -f -e trace=pivot_root` plus `objdump -d` on the call site to confirm a bare `syscall` instruction rather than a PLT thunk. Also confirm `cargo build --target x86_64-unknown-linux-musl` behaves the same. **If this fails, §12.1.7's ranking changes and nix becomes competitive again** — everything else in the comparison is close.

**2. Are all the rustix functions §12 relies on actually reachable under the feature set we enable?**
§12.1.2 and §12.3 were built by grepping `pub fn` out of the extracted crate source. A function existing in `src/` is **not** proof it is exported under `features = ["process","thread","mount","fs","event","net","pty","system","termios"]` — rustix gates heavily by feature and by `cfg`, and `process` additionally requires `linux-raw-sys/prctl`. *Settles it:* on the dev VM, one file that names every function §12.1.2 marks ✅ for rustix and calls or references each one; it either compiles or it names the gaps. Ten minutes, and it converts the single largest block of `[F]` claims in §12 from "grep says so" to "the compiler says so".

**3. `signalfd` or a pidfd for the supervisor's event loop?**
§7.2 specifies `block SIGCHLD -> signalfd`, and rustix does not implement `signalfd` (it is in `not_implemented::yet`). §12.1.7 proposes replacing it with `pidfd_open` on the container init + `poll`, which needs no signal handling at all — but a pidfd reports only that *the direct child* exited, while `PR_SET_CHILD_SUBREAPER` means arbitrary orphaned grandchildren also reparent to us and must be reaped. Unresolved: whether "poll the pidfd, and `waitpid(-1, WNOHANG)` in a loop on each wake" reaps reliably, or whether zombies can accumulate between wakes when the container init itself never exits. Could not be settled here because it needs a running supervisor, and `host-a` is production. *Settles it:* on the dev VM, a ~50-line supervisor that `clone3(CLONE_PIDFD)`s a child which forks a grandchild that exits immediately and then sleeps; `poll` only the pidfd; check `ps` for a zombie under the supervisor after 10s. Then repeat with `signalfd` and compare. **This is a real, small revision to §7.2, not a detail** — it decides whether `nix` enters the dependency graph at all.

**4. Does rustix's non-deprecated `CapabilitySet` carry every constant up to `CAP_LAST_CAP` on 6.11?**
rustix 1.1.0 deprecated its per-capability `Capability` enum in favour of `CapabilitySet` bitflags. §12.3 recommends deleting the `caps` crate *and* D1 §11.1's 60 hand-written lines on the strength of rustix's coverage, so a missing constant (`CAP_CHECKPOINT_RESTORE`, `CAP_BPF`, `CAP_PERFMON`) would be a small but annoying surprise. `[R]` LOW as written. *Settles it:* `grep -n 'const CAP_\|CapabilitySet::' rustix-1.1.4/src/backend/linux_raw/thread/types.rs` and diff against `capabilities(7)` plus `/proc/sys/kernel/cap_last_cap` on the target kernel. Worst case is one `CapabilitySet::from_bits_retain`. D5's problem to close.

**5. Does `tracing` / `tracing-subscriber` actually deadlock across `fork`?**
§8.3 flags this explicitly at MED and names the experiment; §12.7 carries it forward unresolved. I did not read `tracing-subscriber`'s `fmt` layer. Note the *design* answer does not depend on the outcome — the child gets a raw fd either way, as runc, crun and conmon all do — so this is worth answering for understanding rather than for the decision. *Settles it:* read `tracing-subscriber-0.3.23/src/fmt/{mod.rs,writer.rs}` and `registry/sharded.rs` for `Mutex`/`RwLock`/`ReentrantMutex` on the event-emit path; then, on the dev VM, a program that emits an event from a spawned thread in a tight loop while the main thread forks repeatedly and the child immediately emits an event — if it hangs, it hangs.

**6. `CLONE_INTO_CGROUP` end-to-end.** *(Carried from D1 §13 Q1 — restated because §7.2's process diagram depends on it.)*
D1 verified the flag is recognised and validated by kernel 6.11 (`EBADF` for a bad fd, not `EINVAL`) but never performed a successful placement, because that needs creating a cgroup on a production host. Unresolved: the errno when the target violates the no-internal-process rule, and whether the child is billed to the target cgroup from its first instruction. §7.2 puts `[+ CLONE_INTO_CGROUP where available]` in the clone call and §12 assumes it works. *Settles it:* on the dev VM, `mkdir /sys/fs/cgroup/test`, open it `O_DIRECTORY|O_RDONLY`, `clone3` with `CLONE_INTO_CGROUP` and that fd, read `/sys/fs/cgroup/test/cgroup.procs` from the parent before the child does anything.

**7. Exec FIFO vs `notify.sock`, and the actual `sun_path` headroom.**
§10.3's note picks the FIFO at `[R]` MED, and the deciding argument is that `sockaddr_un.sun_path` is 108 bytes and our state paths are long. That argument was made from the constant, not from a measurement of our real paths. If `/run/mydocker/<64-hex-id>/notify.sock` fits comfortably, `SO_PEERCRED` and `poll`-composability become live arguments for the socket. *Settles it:* arithmetic on the final path layout (§6.4 fixes it as `/var/lib/mydocker/containers/<id>/` + `/run/mydocker/<id>/`), then on the dev VM `bind()` a `sockaddr_un` at the longest realistic path and see whether it returns `ENAMETOOLONG`. Cheap, and it either confirms §10.3 or reopens it.

**8. `mount_setattr`: does rustix land it, and do we need it at all?**
Three PRs (#975, #1002, #1630) are open on `bytecodealliance/rustix` as of 2026-09-05 and none has merged. D1 §4.5 defers the new-mount-API adoption decision; §12.1.2 marks `mount_setattr` as the one mount-API gap in rustix. It matters mainly for idmapped mounts, i.e. the rootless track. *Settles it:* watch those three PRs; independently, decide from D1 §4.5 whether phase 1–3 needs `mount_setattr` at all (I believe not — classic `mount`+`MS_REMOUNT|MS_BIND` covers our flag needs at root). If we need it before rustix ships it, it is one `syscall(SYS_mount_setattr, ...)` with `libc`'s `struct mount_attr` (D1 §11.1 confirms the type exists).

**9. Does the long-path allocation fallback actually bite a real runtime?**
§12.1.3 measured the thresholds (rustix 256, nix 1024) and §12.8 observed that youki's `Syscall` trait takes `&Path`/`&str` throughout — so youki's post-clone syscalls are allocation-free only by luck of path length. I did not test whether that is an actual bug. It matters because if a shipped, CNCF-sandboxed runtime has been fine with this for years, the `&CStr`-in-signatures rule is cheap insurance rather than a live hazard; and if it is not fine, that is a finding worth reporting upstream. *Settles it:* on the dev VM, create a bundle under a ~1,200-character directory path and run `youki create`. Watch for a hang rather than an error.

**10. Is a static musl build actually achievable end-to-end?**
§7.3 recommends `x86_64-unknown-linux-musl` static, §12.11 shows C `libseccomp` is what makes runc dynamic, and §7.3 separately notes static glibc breaks NSS (with the mitigation that we parse the container's `/etc/passwd` ourselves anyway). Three unverified links in that chain: whether `rustix`'s `linux_raw` backend builds clean on musl (it should — it does not need libc at all — but `close_range` is gnu-only in `libc` and needs a raw syscall on musl, per D1 §11.1); whether our `libc` type-and-number usage is musl-clean; and whether `seccompiler` is. *Settles it:* on the dev VM, `cargo build --target x86_64-unknown-linux-musl` of a binary exercising rustix mount + capabilities + a raw `clone3` + a `seccompiler`-built filter, then `file` and `ldd` the result. **Do this in phase 0, not phase 3** — it is cheap and it validates a §7.3 `[R]` HIGH.

**11. Why did youki and `oci-spec-rs` leave the `containers` GitHub org?**
`[R]` LOW in §12.12. The rename is confirmed (`GET /repos/containers/youki` → `"full_name": "youki-dev/youki"`; same for `oci-spec-rs`), youki is still a CNCF sandbox project per its README, and the crate metadata reflects the new home — but I found **no** announcement, migration note or governance document explaining it, and I am not going to speculate. It matters only mildly (we depend on `oci-spec` alone, from phase 3, and it has three crates.io owners across three organisations), but "a container project left the container org and nobody wrote it down" is the kind of thing that turns out to have a reason. *Settles it:* `youki-dev/youki` GitHub Discussions and the git history of `README.md`/`GOVERNANCE.md`/`MAINTAINERS.md` around the rename date; the `cncf/sandbox` repository's youki application/annual-review issues; the CNCF landscape entry.

**12. The containerd shim-hang failure mode — real, and does our design have the same hole?**
§2.3 and §5.6 cite containerd issues **#13849** (shim alive with zero children, `TaskExit` never emitted, every API call on the container hangs), **#10727** (killing the shim does not kill the container; containerd may then start a *second* shim and a *second* container process) and **#3334** (shims left behind after removal), all at `[C]` MED because they are GitHub issues rather than post-mortems and I did not reproduce them — `host-a` is production. §5.6's row 7 claims our design avoids #13849 because we write the exit status to disk before anything else. **That claim is untested.** *Settles it:* on the dev VM, once a supervisor exists: `SIGKILL` the supervisor at each of three moments (before the container starts, while it runs, between the container's exit and the state write) and check that `mydocker ps` reports something honest in all three cases. §6.3's Case C already predicts the hard one. This is the single most valuable robustness test in the whole document.

**13. Does anything in our substrate allocate on an error path we care about?**
`[F]` MED in §12.1.3: I confirmed `nix::Result<T> = Result<T, Errno>` and rustix's `Errno(u16)` are allocation-free, but I did not audit every error path in either crate — a helper that builds a `std::io::Error`, or a `Display` impl invoked on the failure path inside the child, would allocate. *Settles it:* after question 2's compile check, run the child-path code under a global allocator shim that `abort()`s on any allocation (a ~15-line `GlobalAlloc` impl), on both the success and the forced-failure paths. That is also the permanent regression test for D1 §11.2's rule, and it is worth having as a real test rather than a rule in a document.

---

# 15. Sources

Reconstructed for the whole document. §1–§11 were written by an earlier agent that did not keep a source list, so those entries were recovered by scanning the body for citations and then **resolving each one against the live repository** — every URL below returned HTTP 200 on 2026-09-05 unless noted. Citations in the body that could not be resolved are listed separately at the end rather than quietly dropped.

## Primary — specifications

- **OCI Runtime Specification** — the source for all of §1, and for §3.1's "the create/start split is mandated" claim.
  - `runtime.md` (lifecycle, state, the five operations, hooks, `ociVersion`) — https://github.com/opencontainers/runtime-spec/blob/main/runtime.md
  - `config.md` (required vs optional fields, POSIX-platform hooks) — https://github.com/opencontainers/runtime-spec/blob/main/config.md
  - `config-vm.md` (§11.6's `vm.hypervisor` / `vm.kernel` / `vm.image` handoff) — https://github.com/opencontainers/runtime-spec/blob/main/config-vm.md
  - Rendered: https://specs.opencontainers.org/runtime-spec/
  - Release history and v1.3.0 changelog (§12.2) — https://api.github.com/repos/opencontainers/runtime-spec/releases/latest and `/tags`; latest release **v1.3.0**, 2025-11-04.

## Primary — man pages

Cited in the D2 body (§3–§9): `clone(2)`, `clone3(2)`, `fork(2)`, `execve(2)`, `signal-safety(7)`, `setns(2)`, `unshare(2)`, `namespaces(7)`, `pid_namespaces(7)`, `mount_namespaces(7)`, `user_namespaces(7)`, `mount(2)`, `prctl(2)`, `setsid(2)`, `signalfd(2)`, `flock(2)`, `pipe(2)`, `pipe(7)`, `read(2)`, `write(2)`, `rename(2)`, `mkdir(2)`, `symlink(2)`, `proc(5)`, `random(4)`, `pts(4)`, `seccomp(2)`. Read as the Ubuntu 24.10 `manpages`/`manpages-dev` **6.8-2** set on `host-a` and cross-checked against man7.org. Direct links for the ones carrying the load:
- https://man7.org/linux/man-pages/man2/clone.2.html — `CLONE_INTO_CGROUP`, `clone3` and "glibc provides no wrapper for `clone3()`"
- https://man7.org/linux/man-pages/man7/signal-safety.7.html — the async-signal-safe allowlist behind §8.1
- https://man7.org/linux/man-pages/man2/prctl.2.html — `PR_SET_CHILD_SUBREAPER`, `PR_SET_NO_NEW_PRIVS`, `PR_SET_PDEATHSIG`, `PR_CAPBSET_*`, `PR_CAP_AMBIENT_*`, `PR_SET_SECCOMP`
- https://man7.org/linux/man-pages/man2/signalfd.2.html · https://man7.org/linux/man-pages/man2/flock.2.html · https://man7.org/linux/man-pages/man2/seccomp.2.html · https://man7.org/linux/man-pages/man4/pts.4.html
- D1 §14 carries the fuller namespace/mount man-page list; it is not duplicated here.

## Primary — crate sources, read directly (§12)

Published `.crate` tarballs downloaded from `https://static.crates.io/crates/<name>/<name>-<version>.crate` and extracted, 2026-09-05. Every ✅/❌ in §12.1.2 and every line number in §12.1.3, §12.3 and §12.11 refers to these:
- **`rustix` 1.1.4** — `Cargo.toml` (backend cfg predicates, feature list), `src/not_implemented.rs` (the `libc_internals` / `quite_yet` / `yet` classification of `clone3`, `execve`, `signalfd`, `mount_setattr`), `src/path/{mod.rs,arg.rs}` (`SMALL_PATH_BUFFER_SIZE = 256`, the `CString::new` fallback, the allocation-free `impl Arg for &CStr`), `src/thread/{setns.rs,prctl.rs}`, `src/process/{pidfd.rs,pidfd_getfd.rs,pivot_root.rs,wait.rs,procctl.rs}`, `src/mount/{fsopen.rs,mount_unmount.rs}`, `src/net/send_recv/msg.rs`, `src/event/{epoll.rs,poll.rs,eventfd.rs}`, `src/pty.rs`, `src/fs/{openat2.rs,memfd_create.rs}`, `src/backend/linux_raw/io/errno.rs`
- **`nix` 0.31.3** — `src/lib.rs` (`NixPath`, `MAX_STACK_ALLOCATION = 1024`, `with_nix_path_allocating`, `Result`/`Error` = `Errno`), `src/sched.rs` (`clone`, `unshare`, `setns`), `src/unistd.rs` (`pivot_root`, `sethostname`), `src/mount/linux.rs`, `src/fcntl.rs` (`openat2`), `src/sys/{signalfd.rs,wait.rs,memfd.rs,socket/mod.rs}`, `src/pty.rs`, `Cargo.toml`
- **`caps` 0.5.6**, **`oci-spec` 0.10.0** (`src/runtime/{version.rs,linux.rs,mod.rs}`, `Cargo.toml`), **`pentacle` 1.1.0** (`src/{lib.rs,file.rs,syscall.rs}`)
- **`rustix` API reference**, cross-check — https://docs.rs/rustix/1.1.4/rustix/not_implemented/index.html

## Primary — registry and repository metadata (§12.1.5, §12.2, §12.8, §12.12)

All fetched 2026-09-05. These are the source for every version, date, download count, maintainer count and ownership claim in §12:
- crates.io JSON API — `https://crates.io/api/v1/crates/<name>` and `https://crates.io/api/v1/crates/<name>/owners` (requires a `User-Agent`; a browser-style request returns 403).
- GitHub REST API — `https://api.github.com/repos/<owner>/<repo>` (the rename redirects proving `containers/youki` → `youki-dev/youki` and `containers/oci-spec-rs` → `youki-dev/oci-spec-rs`), `/contributors`, `/commits?since=2026-03-09`, `/git/trees/<ref>?recursive=1` (crate file counts and sizes), `/issues/<n>`, `/search/issues`.
- **youki `Cargo.toml` set** (§12.9) — https://raw.githubusercontent.com/youki-dev/youki/main/Cargo.toml and `crates/{libcontainer,libcgroups,liboci-cli,youki}/Cargo.toml`
- **youki `crates/youki/src/main.rs`** — the `pentacle::ensure_sealed()` call at line 76 and the CVE-2019-5736 comment at lines 68–70 (§12.9, resolving §11.8's LOW claim)
- **youki `crates/libcontainer/src/{container/builder.rs,syscall/syscall.rs}`** — the `ContainerBuilder` API and the `Syscall` trait whose docstring §12.8 quotes
- **youki `README.md`** — the CNCF-sandbox statement (line 289)
- **Rust `RELEASES.md`** — `File::lock` et al. stabilised in **1.89.0 (2025-08-07)** — https://raw.githubusercontent.com/rust-lang/rust/master/RELEASES.md
- **Rust `std`** — `library/std/src/sys/fs/unix.rs:1338`, `File::lock` is `libc::flock(fd, LOCK_EX)` on Linux — https://raw.githubusercontent.com/rust-lang/rust/master/library/std/src/sys/fs/unix.rs

## Secondary — reference implementation source, read as raw files

- **runc** — `libcontainer/nsenter/nsexec.c` and `nsenter/README.md` (§3.3's two-stage design and sync protocol); `libcontainer/standard_init_linux.go` (the exec-fifo write immediately before `execve`, the log-pipe close); `libcontainer/process_linux.go` (§4's parent-side order); `libcontainer/container_linux.go`, `state_linux.go`, `factory_linux.go`, `message_linux.go`, `sync.go`; `utils_linux.go`, `signals.go`. Base: https://github.com/opencontainers/runc/blob/main/…
- **runc `docs/terminals.md`** — the source of §5.2's console-socket/`SCM_RIGHTS` protocol, §5.6's `TIOCSTI` warning, and §7.4's "through no fault of the user" argument. https://github.com/opencontainers/runc/blob/main/docs/terminals.md
- **runc `libcontainer/exeseal/`** — `cloned_binary_linux.go`, `doc.go` (*"Package exeseal provides mechanisms for sealing /proc/self/exe and thus protecting the runc binary against CVE-2019-5736-style attacks"*), `overlayfs_linux.go`. **This is the current home of the mitigation §11.8 discusses** — see the unresolved-citations note below.
- **crun 1.29.1 / `main`** — `src/libcrun/container.c` (§10.1's grep for `execv`/`memfd`/`reexec`; the exec-fifo direction inversion), `src/libcrun/linux.c` (`libcrun_rexec` as `__attribute__((constructor))`, the CVE-2019-5736 comment). https://github.com/containers/crun/blob/main/…
- **youki** — beyond the `Cargo.toml`s above: `crates/libcontainer/src/process/{fork.rs,container_main_process.rs,container_intermediate_process.rs,init/process.rs}`, `src/notify_socket.rs` (`NOTIFY_FILE = "notify.sock"`), `src/container/builder_impl.rs`, `src/rootfs/rootfs.rs`, `src/syscall/linux.rs`. https://github.com/youki-dev/youki/blob/main/…
- **containerd** — `docs/runtime-v2.md` (the shim `start`/`delete` subcommand contract, the sub-reaper responsibility, the bundle-mounting text) and `core/runtime/v2/README.md`, which is the same document in the 2.x tree; `core/metadata/buckets.go` (§6.2's boltdb schema). https://github.com/containerd/containerd/blob/main/…
- **Podman / conmon** — `libpod/oci_conmon_common.go`, `libpod/oci_conmon_linux.go` (§11.1's conmon argv and the `SOCK_SEQPACKET` sync/start pipes); `conmon/src/conmon.c` (the start-pipe block at `conmon.c:66`, `PR_SET_CHILD_SUBREAPER` at `conmon.c:138`, the double-fork + `setsid`). https://github.com/containers/podman/blob/main/… · https://github.com/containers/conmon/blob/main/src/conmon.c
- **moby** — `api/pkg/stdcopy/stdcopy.go` (§2.3's 8-byte multiplex frame header; the one URL the body cited in full) — https://raw.githubusercontent.com/moby/moby/master/api/pkg/stdcopy/stdcopy.go; `daemon/config/config.go` (the default containerd namespace `"moby"`).
- **CRI-O** — `README.md` (§11.2's scope-exclusion statement and the "CLIs are only meant for testing" line). https://github.com/cri-o/cri-o/blob/main/README.md
- **Apptainer** — `cmd/starter/c/starter.c` (§11.7's C-with-a-Go-tail staging and the `enum goexec` dispatch). https://github.com/apptainer/apptainer/blob/main/cmd/starter/c/starter.c
- **Firecracker `jailer`** — §11.6's "200-line specification of drop privileges, then exec". https://github.com/firecracker-microvm/firecracker/blob/main/docs/jailer.md
- **gVisor architecture guide** — §11.5's quoted principles, the Sentry/Gofer split, and the ptrace-vs-KVM platform tradeoff. https://gvisor.dev/docs/architecture_guide/ · `/platforms/` · `/security/`

## Secondary — issue trackers

- **containerd** #13849 (shim alive with zero children, `TaskExit` never emitted), #10727 (killing the shim does not kill the container; a second shim may be started), #3334 (shims left behind after removal) — all `[C]` MED in §2.3/§5.6 because they are issue reports, not post-mortems, and were not reproduced. https://github.com/containerd/containerd/issues/13849 (and /10727, /3334)
- **nix** #360 (open since 2016-04-19) and #2803 (opened 2026-06-20) — the `sched::clone` `CLONE_VM` soundness bugs. https://github.com/nix-rust/nix/issues/360 · /2803
- **rustix** #975, #1002, #1630 (`mount_setattr`, all open) and #1314 (why `execve`/`_exit` are not exposed). https://github.com/bytecodealliance/rustix/issues/1314
- **nix** PR #1656 — cited *inside* `nix/src/lib.rs` as the justification for the 1024-byte path buffer.

## Empirical — hosts for every `[O]` claim

- **`host-a`** / `HOST-A`, Ubuntu 24.10, kernel `6.11.0-29-generic`, x86_64, root, cgroup v2 unified. Live Docker Engine 28.3.1 (Go 1.24.4), containerd 1.7.27, runc 1.2.5 (spec 1.2.0, libseccomp 2.5.5), docker-init 0.19.0. **Production, serving public traffic on :80/:443.** Every observation in this document was read-only: `/proc`, `/sys`, `ps`/`pstree`/`lsns`, `docker inspect`/`ps`/`info`, `runc --root … list`/`state`, directory listings under `/run/containerd/` and `/var/lib/docker/`, `ldd`/`file` on installed binaries. No container was created, started or stopped; no `systemctl`, no iptables/nft, nothing touching :80/:443. §12.11's linkage observation (`ldd /usr/bin/runc` → `libseccomp.so.2`, `libc.so.6`; `file` → dynamically linked, vs the statically-linked shim) and the confirmation that no Rust toolchain is installed were made under the same discipline.
- **`old-host`** / `OLD-HOST`, Rocky Linux 8.8, kernel `4.18.0-425.3.1.el8`, x86_64, unprivileged, cgroup v1 with no delegation, NFS home. podman 4.9.4-rhel, runc 1.1.12, conmon 2.1.10, apptainer 1.4.0. Light read-only use; source of §11.7's `APPTAINER_SUID_INSTALL=0` observation and §11.8's note that the whole rootless stack is installed alongside podman.
- `[O]` claims are scoped to the named host and kernel and are never generalised into `[F]`.

## Citations in the body that could not be resolved to a current source

Listed rather than dropped, per the reconstruction discipline. None of these invalidates the surrounding claim, but each should be corrected before the text is reused.

1. **`cloned_binary.c` in runc** (§11.8: *"runc and crun ship the same file for that last one (`cloned_binary.c`…)"*). **No such file exists in runc `main` or at tag `v1.5.1`** — both return 404. runc's CVE-2019-5736 mitigation now lives in **`libcontainer/exeseal/cloned_binary_linux.go`**, a **Go** implementation, whose package doc states the purpose verbatim. crun's *is* still C, in `src/libcrun/linux.c`. The claim was true of an older runc (the C `libcontainer/nsenter/cloned_binary.c`, from which crun's version was derived) and is **stale for any current runc**. `[C]` — §11.8's substantive point (three implementations converge on memfd self-sealing; do it early; ~60 lines in Rust) is unaffected and is now *better* supported, since §12.9 confirms youki does it too, via `pentacle`.
2. **`main_linux.go` in runc.** No such file at `main` or `v1.5.1`; runc's root package has `main.go` and 26 other root-level `.go` files, none named `main_linux.go`. Likely intended `main.go` or a pre-1.0 layout. The claim it supported was not re-checked.
3. **`containerd docs/runtime-v2.md` vs `core/runtime/v2/README.md`.** Both resolve (HTTP 200) and are the same specification; the body's path is fine, but note the 2.x tree's canonical copy is `core/runtime/v2/README.md`.
4. **§2.3's shim fd table, §5.4's subreaper proof, §6.x's on-disk layouts** and the other `[O]` claims from §1–§11 cite commands rather than URLs. They are reproducible on `host-a` from the commands quoted inline, but I did not re-run them — those observations remain the earlier agent's, at the confidence it assigned.
5. **§12.12's `tar`-crate ownership note** (`repository = https://github.com/composefs/tar-rs`) is crates.io metadata for `tar` 0.4.46, not an investigated claim. Flagged to D3 in §13 as something to verify, not to inherit.
