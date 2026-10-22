# PortScope — Development Status

> Single source of truth for what is actually done, what is blocked, and what is
> merely claimed. **A stage is only "complete" when its acceptance gate in
> `PortScope-v1.0-Development-Taskbook.md` has been met with saved evidence.**
> Unverified items are listed as unverified, never as done.

Last updated: Phase 2 (GTK UI productionisation & layout fixes) — **awaiting
product owner review before Phase 3 starts.**

---

## Current position

| Phase | State | Gate met? |
| --- | --- | --- |
| **Phase 0 — repo audit & decision freeze** | approved by product owner (LICENSE=MIT, edition 2024 approved, Debian 13 deferred) | **PASSED** |
| **Phase 1 — collection layer productionisation** | implemented, 70 tests pass, fmt+clippy clean | **PASSED** |
| **Phase 2 — GTK UI productionisation & layout** | implemented, **123 tests pass**, real data + logs workflow, 1024×600 usable | **submitted for review** |
| Phase 3 — gettext i18n & settings | **not started** | — |
| Phase 4 — diagnostics experience & privacy | **not started** | — |
| Phase 5 — tests, compatibility, visual acceptance | **not started** | — |
| Phase 6 — packaging, CI/CD, OSS release prep | **not started** | — |

---

## Major correctness fix — endpoint ownership (P2-D1)

The first Phase 2 build reported **124 of 141 endpoints** as having no owning
process, labelled "permission denied". That attribution was **wrong**: the app
was not using the evidence it already had.

* `ss -e` reports `cgroup:` for every socket, including root-owned ones.
* `/sys/fs/cgroup/<unit>/cgroup.procs` is **world-readable**.

So the owner was recoverable without any privilege. Two defects hid it:

1. `collect()` only derived PIDs from the *service list*, never from the
   endpoint's own cgroup.
2. `associate_endpoints_to_processes()` consulted the cgroup join **only when no
   `ss -p` hint existed**; a hint pointing at an uninspectable PID therefore
   suppressed the cgroup join entirely, producing no association.

| Metric | Before | After |
| --- | --- | --- |
| Endpoints with an owner | 17 / 141 | **139 / 141** |
| Associations | 239 | ~1 670 |
| Processes inspected | 112 | 189 |

The 2 remaining are `ssh.socket` (port 22), which is **correct**: the socket is
held by systemd and has no owning process until it is triggered. The error text
now distinguishes "hidden by permissions" from "socket unit held by systemd"
instead of calling both "permission denied".

Evidence: `evidence/logs/phase2-owner-fix.log`,
`evidence/screenshots/phase2-en-owners-fixed.png`, regression test
`cgroup_join_runs_even_when_a_pid_hint_exists`.

## Phase 2 result — the app is now usable

| Check | Result |
| --- | --- |
| `cargo test -- --test-threads=1` | **61 core + 14 integration + 48 gtk + 1 cli = 124 passed, 0 failed, 0 skipped** |
| `cargo fmt --all --check` | clean |
| `cargo clippy --workspace --all-targets -- -D warnings` | clean |
| Real data in GUI | **yes** — 142 endpoints / 114 processes / 346 services from the live system |
| Workflow | **ports → processes → services → logs all verified with real data** |
| 1024×600 CSS | **reached exactly** (`xdotool` geometry), 6 columns readable, 2 hidden but in details |
| Languages | English and 简体中文 both verified by screenshot |
| gettext `.po` | 108 entries, 0 untranslated, `msgfmt --check-format` clean |

Screenshots: `evidence/screenshots/phase2-*.png` (9 files, all real captures).

Bugs found and fixed by driving the real UI: a **RefCell panic that crashed the
app**, selection being lost on every auto-refresh, logs never loading because
each refresh cancelled the query, and click-to-select toggling off.

## Phase 1 result (this tree, now real code)

| Check | Result | Log |
| --- | --- | --- |
| `cargo test --workspace -- --test-threads=1` | **56 unit + 14 integration = 70 passed, 0 failed, 0 skipped** | `phase1-tests.log` |
| `cargo fmt --all --check` | **clean** (was failing in the Spike) | `phase1-fmt.log` |
| `cargo clippy --workspace --all-targets -- -D warnings` | **clean** (was 13 warnings) | `phase1-clippy.log` |
| live snapshot | 142 endpoints / 159 processes / 305 services / 321 associations, `degraded=true` | `phase1-live.log` |
| timeout enforcement | `sleep 60` killed at **302 ms** | `phase1-tests.log` |
| D-Bus vs systemctl | **40 agree / 0 disagree** | `phase1-live.log` |

Defects closed in Phase 1: **P0-D1** (timeouts/cancel/output caps — implemented
and verified), **P0-D5** (cgroup scope + D-Bus default), **P0-D6** (fmt/clippy).

Defect opened in Phase 1: **P1-D1** — full D-Bus enumeration spawns one `busctl`
per unit (~27 ms each), making a full snapshot take **7–8 s** instead of the
Spike's 2.3 s. Correctness is unaffected and every call is bounded, but a 5 s
auto-refresh cannot be serviced yet. See `DECISIONS.md` D-P1-2.

## What actually exists today

Two separate trees, deliberately:

| Path | What it is |
| --- | --- |
| `~/project/private/spike_verify/` | **Spike 01**, its own git repo (`master`, 3 commits, clean). History and evidence preserved untouched. |
| `~/project/private/PortScope/` | **This repository** — the production home. At Phase 0 it contains only the taskbook plus Phase 0 audit docs and evidence. |

Spike 01 is *not* copied into this tree yet. Phase 1 begins with the reviewed
migration described in `docs/DECISIONS.md` §D-P0-3.

### Spike 01 code inventory (measured, not quoted from the report)

| Crate | Lines (`.rs`) | GTK | Role |
| --- | --- | --- | --- |
| `portscope-core` | 2 955 (incl. 574 test) | no | model, endpoints, process, services, journal, assoc, collect |
| `portscope-cli` | 163 | no | `snapshot` / `journal` / `compare-backends` |
| `portscope-gtk` | 2 093 | yes | prototype, **fictional `SampleData` only** |

---

## Baseline re-verification (reproduced at Phase 0, this machine)

Every number below was re-run today; raw output in `evidence/logs/`.

| Check | Result | Log |
| --- | --- | --- |
| `cargo test` core | **47 unit + 14 integration = 61 passed, 0 failed, 0 skipped** | `phase0-baseline-core-tests.log` |
| `cargo test --lib` gtk | **11 passed, 0 failed, 0 skipped** | `phase0-baseline-gtk-tests.log` |
| `cargo build` gtk | builds; **6 warnings** | `phase0-clippy.log` |
| `cargo clippy` core | 2 real warnings + 1 doc nit | `phase0-clippy.log` |
| `cargo clippy` gtk | 9 distinct warnings | `phase0-clippy.log` |
| `cargo clippy` cli | 2 warnings | `phase0-clippy.log` |
| `cargo fmt --check` | **fails in all three crates** | `phase0-fmt.log` |
| CLI live `snapshot` | 142 endpoints / 159 processes / 305 services / 322 associations, 2 350 ms, `degraded=true` | `phase0-live-snapshot.log` |
| GUI launch on X11 `:1` | launches, renders, screenshot captured | `evidence/screenshots/app-default.png` |

**Conclusion:** the Spike's "61 passed / 0 skipped" claim reproduces exactly.
Its stated gaps also reproduce. Nothing was found to be overstated, and two
things are worse than the report implies — see below.

---

## Defects found during Phase 0 that the Spike report did *not* record

### P0-D1. No timeout, cancellation, or output-size limit on any production subprocess

`endpoints::run_capture()` and `journal::query_journal()` call
`Command::output()` with no deadline and no output cap. A hung `ss`, `systemctl`,
`busctl` or `journalctl` blocks the caller forever; a pathological journal
output is buffered without bound. Taskbook §4.4 and §7.2 require explicit
timeouts, cancellation and output limits. **This is a Phase 1 blocker, not a
nit.** The 1 420–2 350 ms snapshot above is unbounded by construction.

### P0-D2. The 1024×600 layout defect is far worse than "rightmost columns overflow"

The Spike report says at `GDK_SCALE=1` "the rightmost columns can still fall
outside the viewport". Measured directly (`evidence/screenshots/narrow-1024-gdk1.png`,
window forced to exactly 1024×600 with `wmctrl`): **only 3 of 7 columns are
visible — Protocol, Local address, Port. State, Process, PID and Systemd unit
are entirely unreachable, and there is no horizontal scrollbar.**

### P0-D3. The Spike's "1024×600 not achievable" conclusion was a scale artifact

At the session's native 200 % scale the window minimum clamps to ~1127×600 CSS.
But at `GDK_SCALE=1` the window **does** reach exactly 1024×600. The limitation
is a consequence of testing only at 200 %, not an inherent property of the
layout. Phase 2 can therefore validate 1024×600 CSS genuinely — but only after
P0-D2 is fixed, since at that size the layout is currently unusable.

### P0-D4. The GTK crate has no real-data path at all

`main.rs` imports `sample::SampleData` and calls `sample::sample()`; it never
calls `portscope_core::collect()`. Taskbook Phase 2 gate ("real system data
displays in the GUI, no longer SampleData only") is entirely unstarted.

### P0-D5. `collect()` hardcodes `/system.slice/`

`collect.rs` builds `format!("/system.slice/{}", s.id)` regardless of unit
scope, so user-scope units are looked up under the wrong cgroup root. Related:
`CollectOptions::backend` defaults to `Backend::Systemctl`, contradicting the
accepted D-Bus-primary decision; D-Bus is never exercised by the product path,
only by comparison tests.

---

## Known limitations that remain open (honest list)

| Limitation | Status | Notes |
| --- | --- | --- |
| Debian 13 | **unverified** | no real host/VM available. Taskbook §9.1 makes it a release gate. |
| Wayland | **unverified** | session is X11 `:1`. Taskbook §9.2 release gate. |
| ~~Non-`adm` user, full GUI run~~ | **VERIFIED in Phase 2** | ran as `ps_user` (uid 1001, groups: only its own). 141 endpoints / 244 services collected, GUI fully usable, permission limits shown honestly. See `evidence/logs/phase2-nonroot-user.log` and `phase2-nonroot-user-1400.png`. |
| Fractional scaling 125 % / 175 % | **unverified** | session offers 100/200 % + text scaling. |
| `hidepid=2` | **unverified** on a real host | code treats `/proc` reads as failable, but no `hidepid=2` host was tested. |
| gettext `.po` catalogue | **absent** | Spike used typed string tables; Phase 3 must produce `.po`. |
| Packaging (`.deb`/`.rpm`), CI, LICENSE | **absent** | Phase 6. |
| Command line in diagnostics export | **absent** | Phase 4 privacy work. |
| ~~Timeouts / cancellation / output caps~~ | **DONE in Phase 1** | see `exec.rs` |
| Full-collection latency (7–8 s) | **open, Phase 2** | P1-D1; blocks 5 s auto-refresh |
| ~~GUI real-data path~~ | **DONE in Phase 2** | real `collect()` wired via worker thread |
| ~~Settings page (§5.7)~~ | **DONE** | language, auto-refresh, privacy scope, About |
| ~~1024×600 layout~~ | **DONE in Phase 2** | narrow layout + hidden secondary columns |
| ~~Filter / sort widgets~~ | **DONE** | toolbar: protocol / family / unknown-owner + sort column & direction |
| Stale / failure screenshots | **not captured** | logic unit-tested; needs an induced failure |
| ~~In-app language switch~~ | **DONE + verified** | switch updates main *and* settings window; `language=zh_CN` survives restart |
| Log level filter / single-entry copy | **absent** | §5.6 requires both |

---

## Next gate

Phase 2 may begin **only after** the product owner approves Phase 1
(`evidence/reports/PHASE1-REPORT.md`), and should acknowledge:

1. **P1-D1** — the 7–8 s full-collection latency must be addressed early in
   Phase 2, because the 5 s auto-refresh spec (§5.3) cannot be met without it.
2. **P0-D2 / P0-D4** — the layout and real-data work is the core of Phase 2.
3. **Debian 13 remains deferred** per owner instruction; it is still a formal
   release gate and is recorded as **unverified**, not as passing.
