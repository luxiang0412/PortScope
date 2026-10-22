# Phase 1 Report — 系统采集层生产化

**Date:** 2026-10-22
**Instruction executed:** Phase 1 only, after product owner approval of Phase 0
(A: "批准 Phase 0，按 A 开始 Phase 1"; LICENSE = MIT; edition 2024 approved;
Debian 13 deferred).
**Verdict:** Phase 1 gate met — 70 tests pass, fmt and clippy clean, P0-D1 fixed
and verified live. **Phase 2 not started.**

---

## 1. 阶段目标

Move the reviewed collection code into the production `core` crate, fix the
defects Phase 0 found (P0-D1 unbounded subprocess, P0-D5 cgroup scope +
backend default, P0-D6 lint/format gates), lock down error/degradation
semantics, and prove the result with reproducible tests.

## 2. 实际完成

1. **Created the production workspace**: `Cargo.toml` (workspace, resolver 3,
   edition 2024, `license = "MIT"`, `rust-version = "1.85"`),
   `rust-toolchain.toml` pinned to 1.94.1, `LICENSE` (MIT).
2. **Wrote the new bounded executor** `crates/core/src/exec.rs` — the P0-D1 fix.
   Timeout, cancellation, output caps, non-zero exit as data.
3. **Ported and corrected** `model`, `endpoints`, `process`, `services`,
   `journal`, `assoc`, `collect` into `crates/core`.
4. **Added three error variants**: `Timeout`, `Cancelled`, `OutputTooLarge`,
   each with a stable machine code (`timeout`, `cancelled`, `output_too_large`).
5. **Made collection cancellable**: `collect_endpoints_cancellable`,
   `query_journal_cancellable`, `collect_cancellable`, with non-cancellable
   convenience wrappers for tests and the CLI.
6. **Fixed P0-D5**: `unit_cgroup_root(scope)` (`/system.slice` vs
   `/user.slice`) and `Backend::DBus` as the default.
7. **Rebuilt the CLI** as the diagnostic front end: `snapshot [--json PATH]`,
   `journal [UNIT] [scope]`, `compare-backends`, `--help`.
8. **Removed all `unsafe`**: the Spike's `geteuid` FFI shim replaced by an
   effective-uid read from `/proc/self/status`; crate sets
   `unsafe_code = "forbid"`.
9. **Cleared the lint gates**: `cargo fmt --all --check` clean,
   `cargo clippy --workspace --all-targets -- -D warnings` clean.

## 3. 修改文件

All new in this tree (the Spike tree was not modified).

| Path | Change |
| --- | --- |
| `Cargo.toml` | workspace, edition 2024, MIT, MSRV 1.85 |
| `rust-toolchain.toml` | pin 1.94.1 + rustfmt/clippy |
| `LICENSE` | MIT (owner decision #1) |
| `crates/core/Cargo.toml` | core manifest, `unsafe_code = "forbid"` |
| `crates/core/src/lib.rs` | module wiring, `#![forbid(unsafe_code)]` |
| **`crates/core/src/exec.rs`** | **new: bounded subprocess executor (P0-D1)** |
| `crates/core/src/model.rs` | 3 new `CollectionError` variants + codes + Display |
| `crates/core/src/endpoints.rs` | routed through bounded exec; cancellable variant |
| `crates/core/src/journal.rs` | routed through bounded exec; cancellable variant |
| `crates/core/src/collect.rs` | `unit_cgroup_root` (P0-D5), D-Bus default, cancellable |
| `crates/core/src/process.rs` | ported unchanged |
| `crates/core/src/services.rs` | ported unchanged |
| `crates/core/src/assoc.rs` | ported unchanged (association rules untouched) |
| `crates/core/tests/integration.rs` | ported; FFI shim → `/proc/self/status` |
| `crates/cli/Cargo.toml`, `crates/cli/src/main.rs` | rebuilt diagnostic CLI |
| `docs/DECISIONS.md` | added §A2 D-P1-1…D-P1-4 |
| `docs/STATUS.md` | Phase 1 result, defect closures, P1-D1 opened |
| `evidence/logs/phase1-*` | 4 logs |

## 4. 执行命令

```bash
cargo build
cargo test -- --test-threads=1
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo run -q -p portscope-cli -- snapshot
cargo run -q -p portscope-cli -- compare-backends
# timeout proof: temporary example running /bin/sleep 60 with a 300 ms limit
```

## 5. 测试结果

- **通过：**
  - `portscope-core` unit: **56 passed** (47 ported + 9 new `exec` tests)
  - `portscope-core` integration: **14 passed**
  - **total 70 passed, 0 failed**
  - `cargo fmt --all --check`: clean
  - `cargo clippy --workspace --all-targets -- -D warnings`: clean
- **失败：** 0
- **跳过：** 0
- **原因：** n/a. No test is skipped and none logs to `PORTSCOPE_SKIP_LOG`.
  Every integration test creates its own ephemeral listener / throw-away
  `--user` unit, so none depends on a service that happens to run here.

### New tests added (9, all in `exec.rs`)

| Test | Proves |
| --- | --- |
| `captures_stdout_and_exit_zero` | happy path |
| `nonzero_exit_is_returned_not_raised` | exit code is data, not an error |
| `missing_binary_is_tool_missing` | `tool_missing` classification |
| `timeout_kills_the_child` | deadline enforced, does not wait |
| `cancellation_stops_run` | `Cancel` works from another thread |
| `oversized_stdout_is_capped_not_buffered` | `output_too_large`, child killed |
| `stderr_is_captured_for_diagnostics` | stderr retained, no shell |
| `run_checked_maps_failure_to_tool_failed` | error mapping |
| `run_checked_passes_through_stdout` | success path |

## 6. 真实运行证据

- **日志路径：**
  - `evidence/logs/phase1-tests.log` — full test output, 70 passed
  - `evidence/logs/phase1-clippy.log` — clean with `-D warnings`
  - `evidence/logs/phase1-fmt.log` — clean
  - `evidence/logs/phase1-live.log` — live snapshot + backend comparison
    (**redacted** of private LAN data before commit)
- **实测数据（真实系统）：**
  - snapshot: 142 endpoints / 159 processes / 305 services / 321 associations,
    `degraded=true`, **7 927 ms**
  - backend comparison: `systemctl` 221 units / 2 909 ms vs D-Bus 40 units /
    1 130 ms, **40 agree / 0 disagree**
  - timeout proof: `/bin/sleep 60` with a 300 ms limit → killed at **302 ms**,
    reported `timeout`
- **截图路径：** n/a — Phase 1 has no GUI. No screenshot is claimed.
- **环境信息：** Ubuntu 24.04.5, X11 `:1`, GNOME 46, rustc 1.94.1, GTK 4.14.5,
  libadwaita 1.5.0, iproute2 6.1.0, systemd 255, user in `adm`.

## 7. 未解决问题与风险

### New: P1-D1 — full D-Bus collection costs 7–8 s (open, blocks 5 s auto-refresh)

Measured root cause: the D-Bus path spawns **one `busctl` process per unit per
interface**, ~**27 ms** per spawn (measured over 10 runs). With ~305 units × up
to 2 interfaces that is ~7–8 s for a full snapshot, versus 2.3 s in the Spike.

This is **not a correctness regression** — results are identical and every call
is now bounded — but it is a real performance limit. The Spike's "D-Bus is 3×
cheaper" was measured over **40 selected units**, not a full enumeration; the
claim was true for that comparison and does not generalise.

Candidate fixes for Phase 2 (to be decided with evidence):
1. one batched `busctl` `GetAll` per interface via `ListUnits`, instead of per
   unit;
2. keep `systemctl` for enumeration and D-Bus for detail;
3. cache unit properties within a refresh cycle.

**Recommendation:** resolve P1-D1 early in Phase 2, before wiring auto-refresh,
otherwise the 5 s default interval (§5.3) is unimplementable.

### Still open from Phase 0

| Item | State |
| --- | --- |
| P0-D2 layout 1024×600 (4 of 7 columns unreachable) | open — Phase 2 |
| P0-D4 GUI has no real-data path | open — Phase 2 |
| Debian 13 | **deferred by owner**, still unverified — formal release gate |
| Wayland | unverified — formal release gate |
| non-`adm` full GUI run | unverified |
| `hidepid=2` | unverified |
| gettext `.po` | absent — Phase 3 |
| Packaging / CI / README | absent — Phase 6 |

## 8. 与任务书的偏差

1. **`ss --json` still not used** — unavailable on iproute2 6.1.0, the primary
   target (§4.4 requirement satisfied by text parsing instead).
2. **Edition 2024 + pinned toolchain 1.94.1, MSRV 1.85.** Per owner approval.
   Note for packaging (§6): Ubuntu 24.04 ships rustc 1.75, and the dependency
   tree already requires 1.85, so a distro-rustc build is impossible regardless
   of edition. `docs/PACKAGING.md` must be revised accordingly in Phase 6 — not
   silently left as a stale "native rustc" option.
3. **D-Bus is the default backend but is slower for full enumeration** (P1-D1).
   The taskbook's §4.5 prefers D-Bus; that preference is kept, and the cost is
   recorded rather than hidden by reverting quietly.
4. **`CollectOptions::only_units` still uses the `systemctl`-shaped unit list**
   for filtering; unchanged from the Spike. Behaviour is correct for both
   backends but means a filtered collection still enumerates to filter.

## 9. 是否满足验收门槛

| Phase 1 gate | Met? |
| --- | --- |
| core tests pass, no unexplained failures | **YES** — 70/70, 0 skipped |
| tests do not depend on machine-present services | **YES** — ephemeral listeners, throw-away user units |
| association rules contain no name-guessing path | **YES** — `assoc.rs` ported unchanged; the port-22 no-cgroup test still passes |
| GUI-free tests run without a display server | **YES** — no GTK dependency in core/cli |
| `docs/STATUS.md` lists every remaining degraded limit | **YES** — incl. new P1-D1 |
| timeouts / cancellation / output caps implemented (§4.4, §7.2) | **YES** — verified live at 302 ms |
| diagnostic snapshot + reproducible CLI | **YES** — `portscope-cli snapshot [--json]` |

**Phase 1 gate: met, submitted for review.** I am not declaring Phase 2 started.

## 10. 建议下一步

1. **Approve Phase 1.**
2. Authorise Phase 2, with **P1-D1 resolved first** (batched D-Bus call), because
   auto-refresh at 5 s depends on it.
3. Then Phase 2 core work: wire real `collect()` into the GTK app (P0-D4) and fix
   the 1024×600 layout (P0-D2), which are the two items that make the app
   actually usable.

The app is **still not usable as a GUI** — Phase 2 is what produces a real
interface. The CLI is now a working, reproducible diagnostic tool.
