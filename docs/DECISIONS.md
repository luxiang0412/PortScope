# DECISIONS — PortScope production

Decision log for v1.0 production development, continuing from Spike 01's
`DECISIONS.md` (kept intact at `~/project/private/spike_verify/DECISIONS.md`).

Format: **what is decided, what is rejected, the evidence, and the owner.**
Status vocabulary:

- **Accepted** — evidence is strong enough to build on now.
- **Provisional** — buildable, must be re-verified at production scale.
- **Open** — needs a product owner decision; agent must not self-approve.

Taskbook authority: where this file conflicts with the taskbook, the taskbook
wins and the conflict is noted.

---

## A. Spike decisions carried forward unchanged

These Spike 01 decisions (D1–D11 in the spike log) are re-affirmed after
reviewing the code, not merely quoted. Each one is backed by code that exists
and tests that pass on this machine.

| Spike ID | Decision | Production status | Re-verification |
| --- | --- | --- | --- |
| D1 | `ss` text parsing + `/proc/net` fallback; **no `ss --json`** (iproute2 6.1) | **Accepted** | `ss -V` = 6.1.0 on target; parser tests pass |
| D2 | Association from evidence only (inode → cgroup → MainPID → socket activation → `Listen=`); never names or port numbers | **Accepted — core product constraint** | `assoc.rs` inspected: no name/port heuristic exists; `no_cgroup_means_no_unit_association_never_port_heuristic` asserts it |
| D3 | `ss -p` is a hint, not authority; missing PID ≠ "no process" | **Accepted** | `SSProcessField` evidence is only one of several; `uid: None` kept as unknown |
| D4 | D-Bus primary, `systemctl` fallback | **Accepted as intent, contradicted in code** — see P0-D5 / D-P0-4 | backend default is still `Systemctl` |
| D5 | journald outcome decided by stderr; queries bounded by `--since` + `-n` | **Accepted** | `journal.rs` inspected; 4 outcomes + `Denied` distinct |
| D6 | GTK-free core | **Accepted** | `portscope-core` has zero GTK deps; 61 tests run headless |
| D7 | Typed string tables in the spike; **`.po` required for production** | **Provisional → must change in Phase 3** | see D-P0-6 |
| D8 | Fictional data in the prototype | **Accepted for spike only**; production must show real data (P0-D4) | — |
| D9 | Fixed-width 360 px details column, not a split view | **Provisional** — interacts badly with narrow widths; revisit with P0-D2 | screenshot evidence |
| D10 | Character-based column sizing | **Accepted**, but insufficient alone — P0-D2 shows total loss of 4 columns at 1024×600 | `narrow-1024-gdk1.png` |
| D11 | Self-contained tests, no machine-dependent services; skips must log | **Accepted** | 0 skipped reproduced |

---

## B. New Phase 0 decisions

### D-P0-1. Production repository is this tree; Spike 01 stays where it is

**Accepted.**

- **Chosen:** `/home/luxiang/project/private/PortScope/` is the production
  repository. Spike 01 remains at `~/project/private/spike_verify/` with its
  own git history and all `evidence/`.
- **Rejected:** moving Spike 01 wholesale into this tree. The taskbook (§0.2,
  §3.2) explicitly says not to overwrite or delete Spike artefacts and to keep
  its evidence as history. Moving it would also mix a 3-commit spike history
  into the production history.
- **Consequence:** Phase 1 migrates *reviewed code*, not a directory tree. The
  production tree starts clean and takes only what survives review.
- **Evidence for this choice:** reproduced baseline (61 + 11 tests) proves the
  spike is sound enough to draw from; the audit (`PHASE0-AUDIT.md`) lists
  exactly what must change during the draw.

### D-P0-2. Production Cargo workspace with three crates

**Accepted.** Matches taskbook §3.1/§3.2.

```
crates/core   portscope-core   GTK-free
crates/cli    portscope-cli    GTK-free
crates/gtk    portscope-gtk    GTK 4 + libadwaita
```

The spike already proved this split (all association rules testable headlessly),
which matters because the target environment's GUI session is not reachable from
a plain shell. Moving from three independent `Cargo.toml`s to one workspace also
fixes a real spike inefficiency: three separate `target/` dirs and three
lockfiles.

### D-P0-3. Migration strategy: reviewed port, staged per phase

**Accepted.**

- **Chosen:** copy each module into the workspace during the phase that owns it,
  reviewing it against the taskbook as it moves; keep tests travelling with their
  module so coverage never dips.
- **Rejected:** (a) copy everything in Phase 1 at once — it would import known
  defects (P0-D1…D5) as if they were finished; (b) rewrite from scratch — it
  would discard 61 passing tests and the empirical findings behind them.
- **Map:**

| Phase | Modules moved | Review obligations |
| --- | --- | --- |
| 1 | `model.rs`, `endpoints.rs`, `process.rs`, `services.rs`, `journal.rs`, `assoc.rs`, `collect.rs` | add timeout/cancel/output caps (P0-D1); fix cgroup scope bug & backend default (P0-D5); edition 2024 |
| 2 | `main.rs` split into UI modules + real `collect()` wiring | fix layout (P0-D2), remove `SampleData` from the product path (P0-D4) |
| 3 | `i18n.rs` → gettext `.po` | D7 upgrade |
| 4 | journal UI + diagnostics export | new code |
| 6 | `data/`, packaging, CI | new code |

### D-P0-4. systemd backend: keep both, make D-Bus the product path

**Accepted (resolves the intent/code mismatch in P0-D5).**

- **Chosen:** `Backend::DBus` becomes the default for `CollectOptions`; the
  `systemctl` backend stays selectable and stays in CI as a cross-check
  (Spike measured 40/40 agreement, 233.7 ms vs 673.8 ms).
- **Constraint:** both backends must produce identical `ServiceInfo` for the
  same unit; a differential test asserts this. Where they diverge, the
  difference is a **bug**, not a preference.
- **Rejected:** dropping `systemctl` now. It is the fallback for environments
  where the user bus is unavailable, and it is the existing evidence source.

### D-P0-5. Rust edition 2024 and MSRV policy

**Provisional — needs owner confirmation.**

- **Measured:** `rustc 1.94.1`, and 1.90.0 / 1.96.1 toolchains are installed.
  Edition 2024 requires Rust ≥ 1.85. All available toolchains satisfy this.
- **Chosen:** edition 2024 (taskbook §3.1), pin the toolchain in
  `rust-toolchain.toml` **to be decided in Phase 1 after a trial build**, not
  guessed now.
- **Open question for owner:** is a fixed `rust-toolchain.toml` wanted (better
  reproducibility, worse for distro packagers who must use the distro rustc)?
  A distro package for Debian 13 / Ubuntu 24.04 must build with the rustc the
  distro ships (Ubuntu 24.04 ships 1.75, **which cannot compile edition 2024**).
  This is a genuine conflict between taskbook §3.1 (edition 2024) and
  §6 packaging (distro-native builds). **Flagged, not silently resolved.**

### D-P0-6. Localisation: gettext `.po`, replacing typed tables

**Accepted as the target (taskbook §6.1 and §9.7 are unambiguous).**

- **Chosen:** gettext `.po` catalogues for `en` (source) and `zh_CN`, compiled
  to `.mo`, loaded as a GResource. Runtime switch rebuilds/rebinds widgets while
  preserving selection, filters and scroll position.
- **Carried over from the spike:** its `Strings`-struct discipline is valuable
  and its 11 i18n tests encode real requirements (no ASCII leaks into Chinese,
  raw values never localised, endonyms untranslated). Those **test assertions**
  move into the gettext implementation rather than being dropped with the
  storage format.
- **Known technical risk:** the spike already proved `setlocale` switches
  language at runtime but **does not update already-rendered widgets**, so
  rebuild/rebind is mandatory either way. Not a downgrade; recorded so it is
  not rediscovered as a surprise.

### D-P0-7. LICENSE — **OPEN, owner decision required**

**Not decided by this agent, deliberately.** Taskbook §0.6 and §6 forbid
assuming a licence. Current state: the spike has **no LICENSE file** and no
`license` field in any `Cargo.toml`, so the code is currently "all rights
reserved" by default.

Relevant constraints, stated so the owner can choose with facts:

| Candidate | Fit for this project | Note |
| --- | --- | --- |
| `MIT` | simple, maximal reuse | no patent grant |
| `Apache-2.0` | patent grant, common in GTK/Rust | bulkier |
| `MIT OR Apache-2.0` | Rust ecosystem convention | dual |
| `GPL-3.0-or-later` | matches the GNOME ecosystem's spirit | copyleft; incompatibility with some corporate use |

**Blocking impact:** Phase 6 explicitly requires "licence clear". If unspecified
by Phase 6, that gate fails. Supply the choice in `docs/DECISIONS.md` and the
agent will add `LICENSE` + `license` fields to every crate.

### D-P0-8. Version number and release channel

**Provisional.** Workspace version `0.1.0` until feature-complete, then `1.0.0`
at the Phase 5/6 gate. Application id: `io.github.portscope.PortScope`
(placeholder — must match the eventual GitHub owner before Phase 6).

---

## C. Open questions carried from Spike 01, now answered by the taskbook

Spike `DECISIONS.md` §D12 listed five open questions. The taskbook answers all
five in §9; recording the mapping so they stop being "open":

| Spike question | Taskbook answer | Production status |
| --- | --- | --- |
| Debian 13 gating? | §9.1 — **yes, release gate**, real host/VM required, container ≠ proof | **blocked**, needs environment |
| Wayland? | §9.2 — **yes, release gate** | **blocked**, needs environment |
| Journal for non-`adm` users? | §9.3 — accept the limit; explain it; **never** auto-add groups | **decided** — build the permission-notice experience |
| `hidepid=2`? | §9.4 — graceful degradation, must be tested | **decided**; test still owed (needs a `hidepid=2` host) |
| `uid: None` sockets? | §9.5 — show unknown owner, flag degraded, **do not** infer container | **decided**, already implemented |

---

## D. Blocked items requiring the product owner or a real machine

1. **LICENSE choice** (D-P0-7) — blocks Phase 6.
2. **Debian 13 real host or VM** — blocks Phase 5 acceptance.
3. **Wayland session** — blocks Phase 5 acceptance.
4. **A non-`adm` test user with a working desktop session** — blocks the §9.3
   acceptance item.
5. **A `hidepid=2` host** — blocks the §9.4 test.
6. **Toolchain/MSRV policy** (D-P0-5), including the edition-2024 vs
   distro-rustc conflict.
7. **GitHub owner / app id** (D-P0-8).
