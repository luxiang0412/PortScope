# TESTING — PortScope

> Phase 0 draft. Defines what must be tested, how it is evidenced, and what a
> "pass" is allowed to mean. Every run that supports a claim is saved under
> `evidence/logs/`.

## 1. Principles

1. Tests must not depend on a service that happens to run on the machine.
   Listeners are created by the tests on ephemeral ports; systemd units are
   throw-away `--user` units created and removed by the tests.
2. **A skip must be logged** (reason + test name). A silent skip is treated as a
   failure of the test harness.
3. Fictional data is allowed for **deterministic UI/layout/screenshot** tests and
   must be labelled as such. Fictional data is **never** presented as proof that
   real collection works.
4. A compiled binary is not a passing feature. Only observed behaviour counts.
5. Cross-distribution claims require that distribution. "Builds on this machine"
   proves nothing about Debian 13.

## 2. Layers

| Layer | What | Display server needed | Runs in CI |
| --- | --- | --- | --- |
| Unit | parsers, model, association rules, error codes, evidence ranking | no | yes |
| Integration | live `ss`/`/proc`/systemd/journal on the host, self-contained fixtures | no | yes (Linux runner) |
| CLI acceptance | `snapshot` / `journal` reproducibility, JSON well-formedness | no | yes |
| i18n | extraction, completeness, plural correctness, raw-value invariance | no | yes |
| UI logic | state model, filtering, sorting, selection preservation across refresh | no (headless logic) | yes |
| GUI | real window on a real display: workflow, layout, states, screenshots | **yes** | virtual display only — not sufficient for Wayland/KDE sign-off |
| Manual | Wayland/GNOME, Debian 13, KDE, fractional scaling, non-`adm` user | **yes** | no |

## 3. Mandatory test cases

**Endpoints** — TCP/UDP; IPv4/IPv6; same port on TCP and UDP as distinct
endpoints; multiple listen addresses on one port; scoped IPv6 (`%iface`);
bracketed `[::]`; column-order variation; `Netid` present/absent; unknown state
preserved; unparsable line yields a structured error, not a panic; `/proc/net`
fallback tested independently.

**Process identity** — process exit reported as `ProcessGone`; PID reuse detected
via start time; `hidepid`-style unreadable `/proc` degrades; own-process
inspection as a normal user; foreign-owned socket shows UID and cgroup but no PID.

**Services** — system and user scope; active / inactive / failed /
not-found distinction; `.socket` activation link; user-level socket activation
whose cgroup is `init.scope`; session scopes excluded; D-Bus and `systemctl`
backends agree for the same unit.

**Journal** — entries / empty / denied / invalid query / failure all
distinguishable; entry cap enforced; time window enforced; the four states are
not collapsed.

**Subprocess** — timeout fires; cancellation works; non-zero exit is classified;
oversized output is capped rather than buffered; missing binary reported as
`ToolMissing`. *(These tests do not exist yet — Phase 1 deliverable, audit
P0-D1.)*

**Association integrity** — port-22-without-cgroup is **not** linked to
`ssh.service`; inactive socket units are not credited; evidence is ranked and
deduplicated; multiple evidence kinds are retained.

**Concurrency** — objects disappearing mid-collection; state changing between
snapshot and inspection; a superseded refresh cannot overwrite a newer one.

## 4. Static gates (taskbook §5)

```bash
cargo fmt --check
cargo clippy --workspace --all-targets -- -D warnings
```

Both currently **fail** (audit P0-D6). Exceptions, if any are ever needed, must be
listed individually with justification — never by lowering the gate globally.

## 5. i18n gates

`msgfmt --check-format` on every catalogue, no missing/fuzzy `zh_CN` entries, no
untranslated ASCII leaks where translation is required, raw technical values
provably unchanged across language switches.

## 6. Environment matrix (acceptance)

| Environment | Requirement | Availability at Phase 0 |
| --- | --- | --- |
| Ubuntu 24.04 + GNOME Wayland | **release gate** | **blocked** — X11 only |
| Ubuntu 24.04 + GNOME X11 | regression | available |
| Debian 13 + GNOME | **release gate** | **blocked** — no host/VM |
| KDE Plasma | launch, main interaction, theme/font | **blocked** |
| 100 % | required | available (`GDK_SCALE=1`) |
| 125 / 150 / 175 / 200 % | where available; native vs text scaling recorded separately | 200 % native, 150 % text only |
| EN / 简体中文, light / dark | all core pages | available |
| non-`adm` ordinary user | full GUI run | **blocked** |

**Containers may not be used** to claim Wayland, host journald, host systemd or
Debian 13 compatibility. If an environment is absent, the item is **blocked and
reported**, not simulated.

## 7. Evidence conventions

- Command + raw output → `evidence/logs/<phase>-<what>.log`.
- Screenshots → `evidence/screenshots/`, each with environment, window size,
  scale, language, theme, and the steps used to reach the state.
- Reports → `evidence/reports/`.
- Pixel-diff tooling may assist but **cannot** replace human usability review.
- A screenshot is only valid if it comes from the real application; the
  environment that produced it must be stated.

## 8. Reproducing the baseline (Phase 0)

```bash
cd ~/project/private/spike_verify
cargo test  --manifest-path spikes/portscope-core/Cargo.toml -- --test-threads=1
cargo test  --manifest-path spikes/portscope-gtk/Cargo.toml  --lib
cargo run   --manifest-path spikes/portscope-cli/Cargo.toml  -- snapshot
```

Expected: 47 + 14 core tests and 11 GTK tests passing, 0 skipped.
