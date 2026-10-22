# PortScope — Development Status

> Single source of truth for what is actually done, what is blocked, and what is
> merely claimed. **A stage is only "complete" when its acceptance gate in
> `PortScope-v1.0-Development-Taskbook.md` has been met with saved evidence.**
> Unverified items are listed as unverified, never as done.

Last updated: Phase 0 (repository audit & decision freeze) — **awaiting product
owner approval before Phase 1 starts.**

---

## Current position

| Phase | State | Gate met? |
| --- | --- | --- |
| **Phase 0 — repo audit & decision freeze** | deliverables produced | **submitted for review** |
| Phase 1 — collection layer productionisation | **not started** | — |
| Phase 2 — GTK UI productionisation & layout fixes | **not started** | — |
| Phase 3 — gettext i18n & settings | **not started** | — |
| Phase 4 — diagnostics experience & privacy | **not started** | — |
| Phase 5 — tests, compatibility, visual acceptance | **not started** | — |
| Phase 6 — packaging, CI/CD, OSS release prep | **not started** | — |

---

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
| Non-`adm` user, full GUI run | **unverified** | only `nobody` *collection* probes exist. Test user is in `adm`. |
| Fractional scaling 125 % / 175 % | **unverified** | session offers 100/200 % + text scaling. |
| `hidepid=2` | **unverified** on a real host | code treats `/proc` reads as failable, but no `hidepid=2` host was tested. |
| gettext `.po` catalogue | **absent** | Spike used typed string tables; Phase 3 must produce `.po`. |
| Packaging (`.deb`/`.rpm`), CI, LICENSE | **absent** | Phase 6. |
| Command line in diagnostics export | **absent** | Phase 4 privacy work. |
| Timeouts / cancellation / output caps | **absent** | P0-D1, Phase 1. |

---

## Next gate

Phase 1 may begin **only after** the product owner approves:

1. The audit in `evidence/reports/PHASE0-AUDIT.md`.
2. The decisions in `docs/DECISIONS.md` §D-P0-1…§D-P0-7 — in particular the
   **LICENSE choice**, which this agent has deliberately not made.
3. Acceptance that P0-D1 is fixed first inside Phase 1.
