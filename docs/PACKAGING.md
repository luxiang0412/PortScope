# PACKAGING — PortScope

> Phase 0 draft. Strategy only; **no package exists yet**. Phase 6 implements and
> validates this.

## 1. Principles

- Distro-native dependency resolution first. PortScope links against the system
  GTK 4 / libadwaita rather than bundling them.
- Do not promise binary portability that has not been proven. "Built here" is not
  "works on Debian 13".
- Flatpak only if the host process / systemd / journald access model can be
  supported safely and explainably — a sandbox that cannot see host processes
  would gut the product, so this needs explicit design, not a checkbox.
- AppImage is a possible later candidate; it must not sacrifice system
  integration for "one binary everywhere".

## 2. Order of delivery

| Order | Format | Notes |
| --- | --- | --- |
| 1 | `.deb` (Ubuntu 24.04, Debian 13) | primary supported path |
| 2 | `.rpm` (Fedora) | after `.deb` is proven |
| 3 | Flatpak | **conditional** on a viable host-access model |
| 4 | AppImage | later consideration only |

## 3. Known system requirements

Measured on the target baseline (Ubuntu 24.04):

| Dependency | Version | Why |
| --- | --- | --- |
| GTK | **4.14.5** | UI; code pinned to `v4_14` feature gates |
| libadwaita | **1.5.0** | UI; pinned to `v1_5` |
| GLib | 2.80.0 | transitively |
| systemd (libsystemd / D-Bus) | 255 | unit state, user manager |
| iproute2 (`ss`) | 6.1.0 | endpoints; **`ss --json` not assumed** |
| journald (`journalctl`) | 255 | logs |
| gettext runtime | 0.21 | localisation |

Minimum versions must be stated explicitly in the package metadata and README;
runtime `pkg-config`/`pkg-config --atleast-version` checks at build time.

## 4. Packaging blockers to resolve before Phase 6

1. **Rust toolchain vs edition 2024.** Ubuntu 24.04 ships **rustc 1.75.0**;
   edition 2024 requires **≥ 1.85**. A distro-native build is therefore
   impossible with the distro rustc. Options: target edition 2021 for distro
   packages, vendor a toolchain, or build with a newer toolchain in a controlled
   environment. **Owner decision required** (`docs/DECISIONS.md` D-P0-5).
   Do not paper over this at packaging time.
2. **LICENSE** must be chosen before any package metadata is written
   (taskbook §6, `docs/DECISIONS.md` D-P0-7).
3. **App id / GitHub owner** must be finalised for desktop file, metainfo and
   GResource paths.
4. Debian 13 and Fedora validation environments must exist.

## 5. Planned install layout (Debian/RPM)

```
/usr/bin/portscope
/usr/bin/portscope-cli
/usr/lib/<arch>/portscope/            # non-ELF data, .mo catalogues
/usr/share/applications/<app-id>.desktop
/usr/share/metainfo/<app-id>.metainfo.xml
/usr/share/icons/hicolor/...
/usr/share/doc/portscope/{README,changelog,copyright}
```

Uninstall must leave no user data behind: PortScope stores no snapshots, only a
small settings file (language, refresh interval) under `$XDG_CONFIG_HOME`.

## 6. Verification required for each package

- Install on a clean machine of that distribution; launch; complete the core
  workflow; uninstall; confirm no residue.
- Dependency list matches what the package actually needs (no undeclared libs).
- README instructions reproduce install/run/uninstall on a **new** machine.
- Checksums published for release artefacts.
- Known limitations (Debian 13, Wayland, non-`adm` journal access) stated in the
  release notes.

## 7. CI expectations (Phase 6)

Format, clippy, core+CLI tests, gettext validation, headless core tests on a
Linux runner, package build and install smoke-test, dependency/licence audit.
CI must pass from a **clean checkout**. CI screenshots do not substitute for
real Wayland/KDE acceptance.
