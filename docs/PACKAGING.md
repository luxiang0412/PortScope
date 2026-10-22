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

## 1a. What can be built where — measured on this machine

| Target | Buildable here? | Why |
| --- | --- | --- |
| `.deb` | **yes** | `dpkg-deb` present; done (see §3a) |
| `.rpm` | **not installed**, but installable | `rpm` 4.18 is in the apt archive; `rpmbuild` arrives with it |
| Flatpak | **not installed**, but installable | `flatpak` is present; `flatpak-builder` is in the archive |
| Wayland verification | **partially** | no Wayland login session, but a headless mutter compositor works (see Phase 5 log) |
| Debian 13 package | **no** | needs a Debian 13 host/VM; `libc6` differs, so a binary built here will not install there |

**Nothing here requires CI to be useful.** The limits are host-side, and what CI
adds is *repeatability and other distributions*, not capability:

| Task | Host | CI |
| --- | --- | --- |
| `.deb` build + lintian | yes | yes, and reproducible on a clean runner |
| `.rpm` build | yes, after `apt install rpm` | yes, or better on a Fedora/`rpm` container |
| Flatpak build | yes, after `flatpak-builder` | yes, with the GNOME runtime from Flathub |
| Debian 13 test | no | **no** — a container is not a host; the taskbook forbids claiming it |
| Wayland visual acceptance | no | **no** — deliverable only as an artifact, not a real session |

CI is worth adding for the `.deb` and `.rpm` so that a release artefact can be
rebuilt from a clean checkout and verified, not because the local machine cannot
do it.

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

## 3a. Debian package — implemented and verified

```bash
./scripts/build-deb.sh          # -> dist/portscope_<version>_<arch>.deb
sudo dpkg -i dist/portscope_*.deb
sudo dpkg -r portscope
```

Verified on this machine: build → lintian → real `dpkg -i` → launch from
`/usr/bin` with real data → `dpkg -r` with no residue. Evidence:
`evidence/logs/phase6-deb-verify.log`.

**Dependencies are derived, then corrected.** `dpkg-shlibdeps` reports only the
ELF soname floors (`libgtk-4-1 (>= 4.12.0)`, `libadwaita-1-0 (>= 1.3~alpha)`),
which would let the package install on a system where the binary cannot run. The
script raises them to the API level actually compiled against:

| Requirement | Value | Why |
| --- | --- | --- |
| `libgtk-4-1` | `>= 4.14` | crate built with `features = ["v4_14"]` |
| `libadwaita-1-0` | `>= 1.5` | crate built with `features = ["v1_5"]` |
| `libc6` | `>= 2.39` | from `dpkg-shlibdeps` on this build |
| `Recommends: iproute2, systemd` | — | PortScope starts without them and reports the missing tool, so they must not be hard dependencies |

**libc floor — a real compatibility limit.** Because `libc6 (>= 2.39)` is
required:

| Distribution | libc | Installs? |
| --- | --- | --- |
| Ubuntu 24.04 | 2.39 | yes |
| Debian 13 | 2.41 | yes (expected; not yet tested on a real host) |
| Debian 12 | 2.36 | **no** |

A binary for an older libc must be built on that release. One binary cannot
serve all of them, and this package does not claim to.

**Reproducible.** `SOURCE_DATE_EPOCH` is set by the script, so two builds of the
same source are byte-identical (verified by comparing sha256 of two runs).
Without it, `dpkg-deb` stamps the archive members with the build time and every
build differs.

**lintian** reports one warning: `initial-upload-closes-no-bugs`. That is
expected for a first upload with no upstream bug tracker.

## 3b. Install/uninstall scripts — implemented and verified

`scripts/install.sh` / `scripts/uninstall.sh` install into `~/.local` by default
and need **no root**, matching the unprivileged model. Verified from a clean
clone: build → install (15 files) → launch the installed binary → it renders
real data in Chinese (catalogue found via `<prefix>/share/locale`) → uninstall
leaves nothing behind.

Where the app looks for translations:

```
<prefix>/share/locale/<lang>/LC_MESSAGES/portscope.mo   (installed)
crates/gtk/po/<lang>/LC_MESSAGES/portscope.mo           (development)
```

The installed path must be **canonical**: glibc gettext does not resolve `..`,
so `prefix/bin/../share/locale` silently missed the catalogue and the installed
app came up in English. The path is now canonicalised.

## 4. Packaging blockers to resolve before Phase 6

0. **No public repository URL yet.** `data/metainfo/...metainfo.xml` has no
   `<url type="homepage">` because no repository exists, so
   `appstreamcli validate` reports `url-homepage-missing`. A guessed URL was
   tried and rejected (`url-not-reachable`), which is worse. Add the real URL
   before release.
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
