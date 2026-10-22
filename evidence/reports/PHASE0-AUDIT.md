# Phase 0 Audit — PortScope v1.0

**Date:** 2026-10-22
**Scope:** repository inventory, baseline re-verification, Spike 01 code review,
migration risk, and known-defect list. **No production feature code was written.**
**Verdict:** baseline reproduces; 5 defects found that the Spike report did not
record; 7 items blocked on the product owner or on real hardware.

---

## 1. Repository inventory

### 1.1 What was found

The production directory `~/project/private/PortScope/` contained **only** the
taskbook:

```
PortScope/
└── PortScope-v1.0-Development-Taskbook.md
```

No git repository (`fatal: not a git repository`), no `spikes/`, no `crates/`,
no `docs/`, no `evidence/`.

Spike 01 was recovered at **`~/project/private/spike_verify/`**, a separate git
repository:

| Item | Value |
| --- | --- |
| Branch | `master`, working tree **clean** |
| Commits | 3 (`Spike A` collection, `Spike B` GTK, `deliverable documents`) |
| Size | 130 MB (mostly the 23 PNGs and git history) |
| Layout | `spikes/{core,cli,gtk}`, `docs/` (empty), `notes/`, `scripts/`, `evidence/{logs,screenshots}`, `SPIKE-REPORT.md`, `ENVIRONMENT.md`, `DECISIONS.md` |
| `.gitignore` | `target/`, `*.log` except `evidence/logs/*.log` |

**Deviation from the taskbook's assumed layout:** the taskbook (§2, §3.2) assumes
a single repository containing `spikes/` *plus* the production tree. In reality
they are two separate repositories. Handled by D-P0-1: production starts here,
the spike tree is referenced, never moved or deleted.

### 1.2 What is missing entirely

`Cargo.toml` (workspace), `rust-toolchain.toml`, `LICENSE`, `README.md`,
`CONTRIBUTING.md`, `SECURITY.md`, `CODE_OF_CONDUCT.md`, `.github/`,
`crates/`, `data/`, `docs/` (all 9 named docs), `spikes/portscope-gtk/po/`,
and the whole packaging story. All expected at Phase 0 — listed so their
absence is a recorded fact, not an oversight.

---

## 2. Baseline re-verification

`spikes/*/target` was absent (gitignored, cleaned), so everything was rebuilt
from source. Raw logs in `evidence/logs/phase0-*`.

| Check | Claimed by Spike | Reproduced today | Verdict |
| --- | --- | --- | --- |
| core unit tests | 47 passed | **47 passed, 0 failed** | matches |
| core integration tests | 14 passed | **14 passed, 0 failed** | matches |
| skipped | 0 | **0** | matches |
| gtk i18n tests | 11 passed | **11 passed** | matches |
| GUI builds | yes | yes, **6 warnings** | matches |
| GUI launches on X11 | yes | yes, verified by screenshot | matches |
| live CLI snapshot | works | 142 endpoints / 159 processes / 305 services / 322 associations in 2 350 ms | matches |
| `cargo fmt --check` | not claimed | **fails in all 3 crates** | gap vs §5 gate |
| `cargo clippy -D warnings` | not claimed | **fails** (core 2, cli 2, gtk 9) | gap vs §5 gate |

**Conclusion:** the Spike's factual claims are accurate. It did not overstate
results. The reproducibility of 61/0 makes the core trustworthy enough to
migrate.

---

## 3. Code review findings

### 3.1 Strengths (verified, to be preserved)

1. **Association logic has no guessing path.** `assoc.rs` was read in full. Every
   join uses an observable key (socket inode in `/proc/<pid>/fd`, cgroup
   equality, `MainPID`, `.socket` `Triggers=`, `Listen=`). There is no
   name-based or port-based heuristic, and a test enforces the absence:
   `no_cgroup_means_no_unit_association_never_port_heuristic`.
2. **Error taxonomy is real.** `CollectionError` distinguishes
   `PermissionDenied` / `ProcessGone` / `ProcessIdentityChanged` / `UnitNotFound`
   / `NoJournalEntries` / `JournalPermissionDenied` / `Truncated` / `ParseError`
   with **stable codes** — exactly what taskbook §4.1 demands instead of
   collapsing failures into empty values.
3. **PID reuse is genuinely handled** via `ProcessIdentity{pid, start_time_ticks}`,
   with tests on both the accept and reject paths.
4. **Inactive socket units are not credited** to endpoints
   (`inactive_socket_unit_does_not_grab_endpoint`) — a subtle correctness win.
5. **journald outcome is decided from stderr**, because exit code is 0 for
   denial, emptiness and bad queries alike. Correct and documented.
6. **No shell invocation.** All external calls use explicit argv; no `sh -c`
   anywhere (grep-verified). This already satisfies taskbook §7.2 on that point.
7. **Core is GTK-free** and its 61 tests run headless on a machine where the GUI
   session is not reachable from the shell — the split earns its keep.

### 3.2 Defects (new findings)

#### P0-D1 — No timeout, cancellation, or output limit on production subprocesses (severity: high)

```rust
// endpoints.rs:15 — no deadline, no output cap
let out = Command::new(tool).args(args).output()

// journal.rs:60 — same
let out = std::process::Command::new("journalctl").args(&arg_refs).output()
```

`output()` blocks until the child exits and buffers all output in memory.
Consequences: a wedged `ss`/`systemctl`/`busctl`/`journalctl` hangs collection
indefinitely; an enormous journal slice is buffered without bound. **Directly
violates taskbook §4.4 and §7.2** ("设定超时、取消、输出大小上限和错误分类").
Scheduled as the first Phase 1 task. `CollectionError` needs `Timeout` and
`OutputTooLarge` variants.

#### P0-D2 — At 1024×600 CSS, 4 of 7 columns are unreachable (severity: high, UI)

Spike called this "rightmost columns can fall outside the viewport". Measured
directly, it is worse: at `GDK_SCALE=1` with the window forced to exactly
1024×600, only **Protocol / Local address / Port** are visible. State, Process,
PID and Systemd unit are gone, and no horizontal scrollbar appears
(`evidence/screenshots/narrow-1024-gdk1.png`). Blocks the §5.8 gate.

#### P0-D3 — The "1024×600 unreachable" conclusion was a 200 %-scale artifact (severity: informational)

`wmctrl -i -r <wid> -e 0,0,0,1024,600` at `GDK_SCALE=1` produced exactly
1024×600. The spike only tested at the session's native 200 %, where the layout's
minimum height clamps the window to ~1127×600 CSS. So the size *is* reachable;
the layout at that size is simply not yet usable. This is good news for Phase 2
and corrects the record.

#### P0-D4 — The GTK crate cannot display real data (severity: known, unstarted)

`main.rs` hard-wires `sample::sample()`; `portscope_core::collect()` is never
called from the GUI. Taskbook Phase 2's gate ("no longer only SampleData")
is entirely unstarted. Also means no GUI evidence exists for real-system states.

#### P0-D5 — `collect()` hardcodes `/system.slice/` and defaults to the wrong backend (severity: medium)

```rust
// collect.rs — ignores scope entirely
let cg = format!("/system.slice/{}", s.id);
...
backend: Backend::Systemctl,   // contradicts accepted D-Bus-primary decision (D4)
```

User-scope units are looked up under the system cgroup root, so user-scope
cgroup-procs expansion silently yields nothing. And the product path never
exercises D-Bus, so the Spike's "D-Bus primary" decision is not implemented in
the collector, only demonstrated in tests.

#### P0-D6 — Formatting and lint gates fail (severity: low, but blocks §5)

`cargo fmt --check` fails in all three crates; clippy reports 13 warnings across
the workspace. Taskbook §5 requires `fmt --check` and
`clippy --workspace --all-targets -- -D warnings`. Cheap to fix during Phase 1
migration, so no separate work item.

### 3.3 Dependency review

| Crate | Declared | Notes |
| --- | --- | --- |
| `gtk4` | `0.9`, features `v4_14`, `default-features = false` | matches system GTK 4.14.5; API pinned ≤ target |
| `libadwaita` | `0.7` (as `adw`), features `v1_5` | matches system 1.5.0 |
| `gettext-rs` | `0.7`, `gettext-system` | already present, so Phase 3 needs no new dep |
| `serde` / `serde_json` | `1` | minimal, auditable |

Dependency set is small and appropriate. No async runtime, no `unsafe` in
reviewed code. **No `license` field and no `LICENSE` file anywhere** — see
D-P0-7.

System baseline (measured): GTK 4.14.5, libadwaita 1.5.0, GLib 2.80.0,
iproute2 6.1.0 (no `ss --json`), systemd 255, gettext 0.21.

---

## 4. Environment support matrix

| Environment | Required by taskbook | Available now | Status |
| --- | --- | --- | --- |
| Ubuntu 24.04 + GNOME **X11** | regression | **yes** (`:1`, GNOME 46, 200 %) | testable |
| Ubuntu 24.04 + GNOME **Wayland** | release gate (§9.2) | **no** — X11 only | **blocked** |
| **Debian 13** | release gate (§9.1) | **no** — host is Ubuntu; container ≠ proof | **blocked** |
| KDE Plasma | launch + theme check | **no** | **blocked** |
| 100 % scale | must test | yes, via `GDK_SCALE=1` | testable |
| 200 % scale | must test | yes (native) | testable |
| 125 % / 150 % / 175 % | where available | **no** native fractional; text-scaling only | partially testable |
| EN / 简体中文 | must test | both locales + CJK fonts installed | testable |
| light / dark | must test | both available | testable |
| non-`adm` user, full GUI | §9.3 | **no** — test user is in `adm`; only `nobody` collection probes exist | **blocked** |
| `hidepid=2` host | §9.4 | **no** | **blocked** |

**Honest statement:** of the two formal release gates (Debian 13, Wayland),
**neither can be satisfied on this machine.** Phase 5 cannot be completed here
without additional real hardware or VMs. This is stated now so it cannot be
discovered late and quietly skipped.

---

## 5. Migration risk register

| # | Risk | Likelihood | Impact | Mitigation |
| --- | --- | --- | --- | --- |
| R1 | Importing P0-D1 (no timeouts) into production as if finished | high if unmanaged | hangs in product | fix first in Phase 1, before UI work |
| R2 | Losing the 61-test coverage during the port | medium | silent regressions | move tests with their modules; CI runs on every commit |
| R3 | Edition 2024 vs distro rustc (Ubuntu 24.04 ships **1.75.0**, edition 2024 needs ≥1.85) | **certain** | distro-native package cannot build | owner decision D-P0-5; may need edition 2021 for distro builds or a vendored toolchain |
| R4 | GTK prototype's monolithic `main.rs` (1 376 lines) resists Phase 2 work | high | slow, error-prone UI phase | split into modules during Phase 2 before adding features |
| R5 | Layout fix regresses the working wide-window layout (D9's 360 px pane) | medium | UI relaunch | keep the allocation-instrumentation technique; screenshot审查 each change |
| R6 | gettext runtime switch leaves stale widget text (already observed in the spike) | high (proven) | i18n gate fails | rebuild/rebind is mandatory; encode spike's 11 assertions as tests |
| R7 | Association rules silently weakened while refactoring | low | **product-defining integrity loss** | the no-heuristic test must keep passing; add a mutation check |
| R8 | Release gates unreachable on available hardware | certain | cannot ship v1.0 | request real Debian 13 + Wayland environments now |

---

## 6. Phase 0 gate self-assessment

| Gate requirement | State |
| --- | --- |
| Spike evidence preserved | **yes** — spike repo untouched, tree clean |
| Existing tests reproducible, differences explained | **yes** — 61 + 11 reproduced exactly; fmt/clippy gaps newly recorded |
| Production structure and migration plan reviewed | **proposed** — `docs/DECISIONS.md` D-P0-1…3, awaiting approval |
| Open questions marked open, not resolved | **yes** — 7 items in DECISIONS §D |

**Phase 0 deliverables are ready for product owner review.**
