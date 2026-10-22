# Flatpak evaluation — measured, not assumed

**Decision: Flatpak is NOT a supported release format for PortScope v1.0.**

This document records the experiment that produced that decision, because the
taskbook (§ Phase 6, packaging strategy item 3) only permits Flatpak *"if the
host process, systemd and journald access model can be safely and explainably
supported"*. It cannot.

---

## What Flatpak must provide for PortScope to work

PortScope's core value is the link between an endpoint, an owning process and a
systemd unit. That link needs:

| Requirement | Why |
| --- | --- |
| `ss -e` (no `-p`) | endpoint address/port/state, socket `uid`, `ino`, `cgroup` |
| `ss -p` **or** `/proc/<pid>/fd` | to identify the owning process |
| `/sys/fs/cgroup/<unit>/cgroup.procs` | to recover the owner without privileges |
| `/proc/<pid>/status`, `/stat` | PID, UID, start time (PID-reuse defence) |
| systemd D-Bus on the system bus | unit state, `MainPID`, `Triggers=`, `Listen=` |
| `journalctl` against the host journal | recent log entries |

## The blocking constraint: Flatpak always isolates the PID namespace

`flatpak run` unshares the PID namespace, and this is **not user-controllable**:
`--unshare` accepts only `network` and `ipc`, never `pid`. There is no metadata
key to share the host PID namespace; only `--host` would, which is not sandboxing
at all.

## Experiment

A Flatpak-equivalent sandbox was constructed with `bwrap` (the tool Flatpak
itself uses) using the most generous set of permissions that a Flatpak could
plausibly request: PID isolation on, host `/proc` mounted, host `/sys`, `/run`
and `/var` exposed, `/dev` provided.

```
$ bwrap --unshare-pid --proc /proc --dev /dev \
        --dev-bind /usr /usr --dev-bind /lib /lib --dev-bind /lib64 /lib64 \
        --dev-bind /bin /bin --dev-bind /etc /etc \
        --dev-bind /sys /sys --dev-bind /run /run --dev-bind /var /var \
        sh -c '...'
```

Results:

| Check | Result | Meaning |
| --- | --- | --- |
| Processes visible in the namespace | **5** (PID 1 is `bwrap`, not `systemd`) | host processes are invisible |
| Host `/proc` entries outside the sandbox | 1062 | — |
| `ss -e` endpoint rows | **140** | endpoints are still readable |
| `ss -p` rows containing `pid=` | **0** | no owning PID is ever reported |
| `/sys/fs/cgroup/.../cgroup.procs` readable values | **0** (invalid) | cgroup → PID recovery fails |
| `/proc/<pid>` for a host PID | **does not exist** | all process inspection fails |

## Consequence

Inside a Flatpak sandbox PortScope would be able to list endpoints but would
**never** be able to link one to a process or a service:

- `ss -p` yields no PIDs because the owning processes are outside the namespace.
- `cgroup.procs` returns host PIDs that do not exist in the sandboxed `/proc`.
- The inode → `/proc/<pid>/fd` join cannot work for the same reason.

That would leave every row saying *unknown owner* and *no confirmable
association* — precisely the honest-but-useless state the product exists to
avoid. Shipping it would be worse than not shipping a Flatpak at all: the
permissions granted (host `/proc`, `/sys`, network, system D-Bus) would dismantle
the sandbox while still failing to deliver the feature.

## Additional, non-blocking problems

- **Journal access.** The host journal socket is not visible in the sandbox by
  default. A Flatpak would need `--filesystem=/run/systemd/journal` or a
  socket-activated proxy, which is a second bespoke permission.
- **`systemctl`/`journalctl` availability.** These are host tools; a Flatpak
  would have to either bundle them (a bad idea — they talk to the host systemd)
  or reach systemd purely over D-Bus.
- **Security narrative.** The permissions required would be far broader than a
  user expects from "inspect my ports", so the install-time permission list would
  be misleading relative to what the app appears to be.

## What would change this decision

A Flatpak could become viable if the function were re-architected around a
**host-side helper** (a small privileged-or-user service exposing a narrow,
auditable D-Bus API that the sandboxed UI talks to). That is:

- a significant new component, explicitly out of scope for v1.0
  (the taskbook forbids a daemon or privileged helper for v1.0);
- a security-sensitive design deserving its own document and review.

It is therefore recorded as a possible future direction, **not** as a deferred
task for v1.0.

## Reproducing this evaluation

```sh
# PID isolation is forced by Flatpak and cannot be turned off via --unshare.
$ flatpak run --help | grep -A2 -- '--unshare'
  --unshare=SUBSYSTEM   ... SUBSYSTEM must be one of: network, ipc.

# Endpoints remain visible; the owner link does not.
$ bwrap --unshare-pid --proc /proc --dev /dev \
        --dev-bind /usr /usr --dev-bind /lib /lib --dev-bind /lib64 /lib64 \
        --dev-bind /bin /bin --dev-bind /etc /etc \
        --dev-bind /sys /sys --dev-bind /run /run --dev-bind /var /var \
        sh -c 'ss -tulneH | wc -l; ss -tulnpHe | grep -c "pid="'
140
0
```
