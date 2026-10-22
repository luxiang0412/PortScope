# ARCHITECTURE — PortScope

> Phase 0 draft, describing the **intended** production architecture and the
> parts inherited from Spike 01. Code does not exist in this tree yet; the
> current implementation lives at `~/project/private/spike_verify/spikes/` and is
> being migrated phase by phase (see `docs/DECISIONS.md` D-P0-3).

## Crate split

```
crates/
├── core/   portscope-core   GTK-free: model, collection, association, errors
├── cli/    portscope-cli    GTK-free: reproducible snapshots for tests/scripts
└── gtk/    portscope-gtk    GTK 4 + libadwaita presentation layer
```

The split is a hard rule, not a preference: **`core` must never depend on GTK.**
That is what lets every association rule be tested in CI with no display server,
and it is why the association logic in the Spike is trustworthy today.

| Layer | May depend on | Must not |
| --- | --- | --- |
| `core` | libc-free std, `serde` | GTK, libadwaita, `gtk` main loop |
| `cli` | `core` | GTK |
| `gtk` | `core` | spawning processes itself; blocking I/O on the main thread |

## Data flow

```
collect() ──► Snapshot { endpoints, processes, services, associations, errors, degraded }
                 │
                 ├─► CLI  : print / JSON, for reproducibility and evidence
                 └─► GTK  : worker thread → channel → main loop → list models
```

Rules:

- All blocking I/O and **all** subprocess calls happen off the GTK main thread.
  Results arrive via a channel and are applied on the main loop.
- The GUI holds the last successful `Snapshot` plus its `collected_at`, so a
  failed refresh can keep showing data explicitly marked **stale**.
- `Snapshot.errors` is not decoration: each error is surfaced in the UI with its
  stable code and a localised explanation.

## Association rules (product-defining)

An association is emitted **only** from an observable join key. Ranked by
strength, mirroring the Spike and taskbook §4.2:

| Rank | Evidence | How obtained | Privilege needed |
| --- | --- | --- | --- |
| 1 | `SocketInodeInProcFd` | socket `ino` (`ss -e`) appears in `/proc/<pid>/fd` | own processes; others if `/proc` readable |
| 2 | `CgroupUnitMatch` | endpoint `cgroup:` equals unit `ControlGroup` | none — `ss -e` exposes cgroup for all sockets |
| 3 | `MainPidMatch` | endpoint owner PID equals unit `MainPID` | none |
| 4 | `SocketActivation` | `.socket` unit `Triggers=` names the service | none |
| 5 | `SocketUnitListen` | unit `Listen=` address:port matches the endpoint | none |

**Forbidden, and enforced by tests:** matching on port number alone, process
name ↔ unit name, or unit description text. A test asserts that an endpoint with
no cgroup is *not* linked to `ssh.service` merely because it is port 22.

Conflict resolution:

- Multiple evidence kinds may apply; **all** are retained and ranked, so the UI
  can show why — never collapsed into one unexplainable verdict.
- A `.socket` cgroup resolves to its triggered `.service` and is labelled
  `SocketActivation`, not `CgroupUnitMatch`, because the socket fd belongs to the
  socket unit while the *process* belongs to the service.
- Inactive/absent socket units are never credited to an endpoint (tested).
- `init.scope` and `session-*.scope` describe *where* a process runs, not *which*
  unit owns a port, and are excluded from service presentation.
- User-level socket activation: `ss -e` reports the fd under
  `…/init.scope` (the user manager holds it), so the link is recovered from the
  socket unit's `Listen=` plus `Triggers=` instead of the cgroup.

## Process identity and PID reuse

`ProcessIdentity { pid, start_time_ticks }`, where `start_time_ticks` is field 22
of `/proc/<pid>/stat`. A PID whose start time changed is a **different process**:
the old link is dropped rather than silently re-pointed. Vanished processes are
reported as `ProcessGone`. This prevents a stale details pane from describing a
new, unrelated process.

## Field-level failure semantics

Taskbook §4.1 forbids collapsing distinct failures into empty values. The model
therefore distinguishes, per field:

| State | Representation |
| --- | --- |
| present | `Some(value)` |
| not queried | option untouched + collector did not run |
| permission denied | `None` + `CollectionError::PermissionDenied{resource, hint}` |
| object vanished | `CollectionError::ProcessGone` / row removed with `gone` flag |
| unsupported | `CollectionError::ToolUnsupported` |
| unknown | explicit `Unknown` variant where the domain has one |
| empty but known-empty | empty collection + **no** error |

`ss -p` omitting `users:` is **unknown**, never "no process". `active_state`
distinguishes `Inactive` from `Failed`, `NotFound` and `Unknown`.

## Subprocess policy

- Explicit argv only; **no `sh -c`**, no string concatenation of user input.
- Per-call **timeout**, **cancellation** and **output size cap**; results are
  classified into structured outcomes, not inferred from stderr text alone.
- Raw stderr is diagnostic context, never the product's error code.
- **Not yet implemented in the Spike code** — see audit P0-D1. This is the first
  Phase 1 task.

## Collection strategy (inherited, re-affirmed)

- Endpoints: `ss -tulnHe` primary; `/proc/net/{tcp,tcp6,udp,udp6}` fallback.
  **`ss --json` is unavailable on iproute2 6.1** (Ubuntu 24.04) and is not used.
- Services: D-Bus primary, `systemctl` fallback and cross-check. Missing units
  arrive as `LoadState=not-found` stubs rather than errors and are normalised to
  `UnitNotFound`.
- Journal: `journalctl -o json`, bounded by `--since` and `-n`; outcome decided
  from stderr because exit status is 0 for denial, emptiness and malformed
  queries alike.

## Threading model (intended)

```
main loop ──submit──► worker (blocking collect / journal query, cancellable)
    ▲                        │
    └──── channel result ────┘
```

A generation counter discards results from superseded requests so a slow older
refresh cannot overwrite a newer one, and a cancelled request is dropped.

## Localisation architecture

gettext `.po` → `.mo` → GResource. Runtime switching rebuilds/rebinds visible
widgets while preserving selection, filters and scroll position. Raw system
values are never passed through translation.

## Non-goals in architecture

No daemon, no privileged helper, no polkit action, no background service, no
async runtime, no database, no on-disk snapshot cache in v1.0.
