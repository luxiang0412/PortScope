# Phase 0 Report — 仓库盘点与决策冻结

**Date:** 2026-10-22
**Instruction executed:** taskbook §12 — Phase 0 only. No Phase 1 work started.
**Verdict:** Phase 0 deliverables complete and submitted for review. **No
production code was written.**

---

## 1. 阶段目标

Verify the real state of the repository before production work: locate the Spike
01 artefact, reproduce its claimed test baseline, review its core implementation
and dependency versions, freeze the technical/product decisions, and record every
open question as open. Do not overwrite or delete existing Spike evidence. Do not
begin Phase 1.

## 2. 实际完成

1. **Located the Spike.** It was **not** in the project directory. The project
   directory held *only* the taskbook, with no git repository. Spike 01 was found
   at `~/project/private/spike_verify/` — a separate git repo, 3 commits, clean
   tree, 130 MB, with `SPIKE-REPORT.md`, `ENVIRONMENT.md`, `DECISIONS.md`, 23
   screenshots and probe logs.
2. **Reproduced the baseline from source.** `target/` was absent, so everything
   was rebuilt. Results matched the Spike exactly: core 47 + 14 = **61 passed, 0
   failed, 0 skipped**; GTK **11 passed**.
3. **Verified the GUI really runs.** Built and launched it against the live X11
   session `:1`, confirmed the window exists and captured a screenshot. It renders
   correctly.
4. **Reviewed the core implementation** (`model.rs`, `assoc.rs`, `collect.rs`,
   `journal.rs`, `endpoints.rs`, `process.rs`, `services.rs`) against the taskbook.
5. **Measured the known layout defect independently** at 1024×600 / `GDK_SCALE=1`
   with `wmctrl` — and found it materially worse than the Spike reported.
6. **Re-ran the live collection** through the CLI: 142 endpoints / 159 processes
   / 305 services / 322 associations in 2 350 ms, `degraded=true`. Log redacted
   of private LAN data before committing.
7. **Produced the 9 required documents** under `docs/`, plus the audit report and
   6 evidence logs, and initialised the production git repo on `main`.
8. **Recorded 5 new defects** not present in the Spike report and 7 open items.

## 3. 修改文件

All new; nothing in the Spike tree was modified or deleted.

| Path | Purpose |
| --- | --- |
| `docs/STATUS.md` | authoritative phase/limitation state |
| `docs/DECISIONS.md` | Spike decisions re-affirmed + new D-P0-1…D-P0-8 |
| `docs/PRODUCT.md` | scope, workflow, non-goals |
| `docs/ARCHITECTURE.md` | crate split, data flow, association rules, subprocess policy |
| `docs/UI_UX_SPEC.md` | views, states, keyboard, size/scale acceptance, layout defects |
| `docs/I18N.md` | gettext target, never-translated list, glossary, CI checks |
| `docs/SECURITY.md` | privilege model, subprocess discipline, privacy, export rules |
| `docs/TESTING.md` | test layers, mandatory cases, environment matrix, evidence rules |
| `docs/PACKAGING.md` | format order, system requirements, blocking issues |
| `evidence/reports/PHASE0-AUDIT.md` | full audit incl. defect list and risk register |
| `evidence/logs/phase0-baseline-core-tests.log` | 61-test baseline output |
| `evidence/logs/phase0-baseline-gtk-tests.log` | 11-test baseline output |
| `evidence/logs/phase0-environment.log` | versions, session, user |
| `evidence/logs/phase0-clippy.log` | clippy baseline (13 warnings) |
| `evidence/logs/phase0-fmt.log` | `fmt --check` failures |
| `evidence/logs/phase0-live-snapshot.log` | live collection, **redacted** |
| a Phase 0 capture *(capture removed before publishing: contained LAN addresses)* | GUI launched today on X11 `:1` |
| a Phase 0 capture *(capture removed before publishing: contained LAN addresses)* | **defect P0-D2 evidence** |
| `.gitignore`, directory skeleton | production layout scaffolding |

## 4. 执行命令

```bash
# inventory
git status; git log --oneline      # spike repo: clean, 3 commits
find <home> -iname '*portscope*'

# environment
. /etc/os-release; uname -r; gnome-shell --version
pkg-config --modversion gtk4 libadwaita-1; rustc -V; cargo -V; ss -V
apt-cache policy rustc             # -> 1.75.0 (cannot compile edition 2024)

# baseline (rebuilt from scratch)
cargo test --manifest-path spikes/portscope-core/Cargo.toml -- --test-threads=1
cargo test --manifest-path spikes/portscope-gtk/Cargo.toml --lib
cargo clippy --manifest-path spikes/portscope-*/Cargo.toml --all-targets
cargo fmt --check                  # per crate

# live collection
cargo run --manifest-path spikes/portscope-cli/Cargo.toml -- snapshot

# GUI
cargo build --manifest-path spikes/portscope-gtk/Cargo.toml
DISPLAY=:1 XAUTHORITY=/run/user/1000/gdm/Xauthority ./spikes/portscope-gtk/target/debug/portscope-gtk
wmctrl -i -r 0x04400004 -e 0,0,0,1024,600   # force exact 1024x600 CSS
import -window 0x04400004 the Phase 0 narrow capture (removed before publishing)
```

## 5. 测试结果

- **通过：**
  - core unit tests: 47 passed, 0 failed
  - core integration tests: 14 passed, 0 failed
  - GTK i18n tests: 11 passed
  - total **72 passed, 0 failed**
  - GUI builds and launches on X11; live CLI collection succeeds
- **失败：**
  - `cargo fmt --check`: fails in all three crates
  - `cargo clippy -D warnings`: fails (core 2, cli 2, gtk 9 warnings)
- **跳过：** 0 — no test skipped. Confirmed by reading the test-result lines, not
  assumed from the Spike report.
- **原因：** fmt/clippy were not gated in the Spike; both are Phase 1 cleanups and
  are recorded as defect **P0-D6**, not silently accepted.

## 6. 真实运行证据

- **截图路径：**
  - a Phase 0 capture *(capture removed before publishing: contained LAN addresses)* — GUI launched today, X11 `:1`,
    Ubuntu 24.04.5, 200 % scale, English, light theme, fictional data
  - a Phase 0 capture *(capture removed before publishing: contained LAN addresses)* — window forced to exactly
    1024×600 CSS at `GDK_SCALE=1`; shows only 3 of 7 columns
- **日志路径：** `evidence/logs/phase0-*` (6 files, listed in §3)
- **环境信息：** Ubuntu 24.04.5 LTS; kernel 7.0.0-38-generic; GNOME Shell 46.0;
  session **X11 on `:1`** (no Wayland); GTK 4.14.5; libadwaita 1.5.0; rustc/cargo
  1.94.1; iproute2 6.1.0 (no `ss --json`); systemd 255; gettext 0.21; user
  `<user>` **in group `adm`** (so system journal is readable — the non-`adm` case
  remains unverified).

## 7. 未解决问题与风险

### New defects found (not in the Spike report)

| ID | Severity | Defect |
| --- | --- | --- |
| **P0-D1** | high | **No timeout, cancellation or output cap on any production subprocess** (`endpoints::run_capture`, `journal::query_journal` both use bare `.output()`). Directly violates §4.4/§7.2. A wedged `ss`/`journalctl` hangs collection forever. First Phase 1 task. |
| **P0-D2** | high | At 1024×600 CSS, **4 of 7 columns are unreachable** (State, Process, PID, Systemd unit) with no horizontal scrollbar. Worse than the Spike's "rightmost columns may overflow". |
| **P0-D3** | info | The "1024×600 unreachable" claim was a **200 %-scale artifact**; at `GDK_SCALE=1` the window reaches exactly 1024×600. The size *is* testable. |
| **P0-D4** | known | **GUI cannot show real data** — hard-wired to `sample::sample()`; `collect()` never called from the GUI. Entire Phase 2 gate unstarted. |
| **P0-D5** | medium | `collect()` hardcodes `format!("/system.slice/{}", s.id)` ignoring unit scope, and `CollectOptions::backend` defaults to `Systemctl`, contradicting the accepted D-Bus-primary decision. |
| **P0-D6** | low | `fmt --check` and `clippy -D warnings` both fail (13 warnings). |

### Blocked on the product owner

1. **LICENSE** — no licence exists anywhere; must not be assumed. Blocks Phase 6.
2. **Toolchain/MSRV policy** — Ubuntu 24.04 ships **rustc 1.75.0**, but edition
   2024 needs **≥ 1.85**. Taskbook §3.1 (edition 2024) conflicts with §6
   (distro-native packaging). Needs a decision; not resolvable by the agent.
3. **App id / GitHub owner** for the eventual metainfo/desktop id.

### Blocked on hardware

4. **Debian 13** real host or VM — formal release gate (§9.1).
5. **Wayland** session — formal release gate (§9.2). Only X11 is available.
6. **Non-`adm` user with a desktop session** — §9.3 acceptance.
7. **`hidepid=2` host** — §9.4 test.

**Risk:** both formal release gates (Debian 13, Wayland) are currently
unsatisfiable on this machine. This is stated now so it cannot surface late.

### Deviation from the taskbook's assumed structure

The taskbook assumes one repository containing `spikes/` *and* the production
tree. In reality these are two separate repos. Resolved by D-P0-1: production
starts here; the Spike is referenced and left untouched.

## 8. 与任务书的偏差

1. **Two repositories instead of one.** Handled by D-P0-1 as described above.
   No Spike file was moved, modified or deleted — verified by `git status` on the
   spike repo being clean.
2. **`ss --json` not used.** Not a deviation but a constraint: iproute2 6.1.0 on
   Ubuntu 24.04 does not support it. Text parsing is the only option for the
   primary target.
3. **Live snapshot log had to be redacted** before committing (real LAN IPs and
   interface names would have violated taskbook §6 on sensitive data). The
   structure of the finding is preserved.
4. **No `Cargo.toml` / workspace yet.** Deliberate: taskbook §12 limits this
   phase to audit and decision freeze, and D-P0-3 requires reviewed, phase-scoped
   migration rather than a bulk copy.
5. **`evidence/logs/phase0-environment.log` contains the test username and group
   list**, as the Spike's own `ENVIRONMENT.md` already does. It is environment
   evidence for the permission model, but flagging it for the owner in case it
   should be scrubbed before public release.

## 9. 是否满足验收门槛

| Phase 0 gate | Met? |
| --- | --- |
| original Spike evidence preserved | **YES** — spike repo untouched, tree clean |
| existing tests reproducible, differences explained | **YES** — 61 + 11 reproduced; fmt/clippy gaps newly recorded, not hidden |
| production structure + migration plan reviewed | **SUBMITTED** — awaiting owner approval (D-P0-1…D-P0-3) |
| open questions marked open, not disguised as resolved | **YES** — 7 open items, incl. LICENSE and toolchain |

**Phase 0 gate: submitted for review.** I am not declaring it approved.

## 10. 建议下一步

**Stop here and wait for approval.** Specifically, I need from the product owner:

1. **Approve or amend** the production structure and the migration plan
   (`docs/DECISIONS.md` D-P0-1, D-P0-2, D-P0-3).
2. **Choose the LICENSE** (D-P0-7) — required before Phase 6, cheap to settle now.
3. **Resolve the edition-2024 vs distro-rustc conflict** (D-P0-5): Ubuntu 24.04
   ships rustc 1.75.0, which cannot build edition 2024 crates. Without a decision
   the packaging story cannot be planned.
4. **Provide or accept the absence of** Debian 13 / Wayland / non-`adm` /
   `hidepid=2` environments, since two of them are formal release gates.

On approval, **Phase 1** would begin by fixing **P0-D1** (timeouts, cancellation,
output caps, new error variants) *before* migrating other modules, because it is a
correctness issue that would otherwise be imported as finished.

I have not started Phase 1.
