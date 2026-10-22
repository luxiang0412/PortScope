#!/bin/sh
# Build an RPM package for PortScope.
#
# Requires: cargo, rpmbuild (Debian/Ubuntu: `apt install rpm`).
# Does NOT require root. Output: dist/portscope-<version>-<release>.<dist>.<arch>.rpm
#
# Caveats that are stated rather than hidden:
#   * This builds on the host's toolchain. An RPM built on Ubuntu links against
#     Ubuntu's glibc, so it will usually only *install* on distributions with a
#     compatible or newer glibc. A Fedora-targeted RPM must be built on Fedora;
#     CI is the right place for that (see .github/workflows/).
#   * The spec's BuildRequires name Fedora packages. On Ubuntu the build works
#     because the libraries are present; the declared names matter when the spec
#     is built on Fedora, which is the supported path.
set -eu

ROOT="$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)"
cd "$ROOT"

umask 0022

VERSION="$(sed -n 's/^version = "\(.*\)"$/\1/p' Cargo.toml | head -1)"
[ -n "$VERSION" ] || { echo "error: cannot read version from Cargo.toml" >&2; exit 1; }

command -v rpmbuild >/dev/null 2>&1 || {
    echo "error: rpmbuild not found. Install it with: sudo apt install rpm" >&2
    exit 1
}

say() { printf '%s\n' "$*"; }

TOP="$ROOT/dist/rpm"
rm -rf "$TOP"
for d in BUILD RPMS SOURCES SPECS SRPMS BUILDROOT; do
    mkdir -p "$TOP/$d"
done

# --- source tarball -------------------------------------------------------
# rpmbuild insists on a Source0; vendoring the tree keeps the spec honest about
# what it builds.
say "==> Creating source tarball"
STAGE="$TOP/src/portscope-$VERSION"
mkdir -p "$STAGE"
for item in Cargo.toml Cargo.lock rust-toolchain.toml LICENSE README.md \
            crates data; do
    [ -e "$item" ] || continue
    cp -r "$item" "$STAGE/"
done
# Never ship build output or vendored target dirs inside the tarball.
find "$STAGE" -name target -type d -prune -exec rm -rf {} + 2>/dev/null || true
tar -C "$TOP/src" -czf "$TOP/SOURCES/portscope-$VERSION.tar.gz" "portscope-$VERSION"

# --- build ----------------------------------------------------------------
say "==> Building RPM"
# SOURCE_DATE_EPOCH makes the payload timestamps deterministic.
: "${SOURCE_DATE_EPOCH:=1792627200}"
export SOURCE_DATE_EPOCH

# `--nodeps` skips the BuildRequires check.
#
# That check is genuinely useful ON FEDORA, where the names resolve. On
# Debian/Ubuntu the Fedora package names (gtk4-devel, libadwaita-devel,
# systemd-rpm-macros) are not in the rpm database at all, so the check can only
# fail — it is not evidence that anything is missing. The build therefore runs
# with --nodeps off-Fedora and with the check enabled elsewhere, and the result
# is labelled accordingly so nobody mistakes this artifact for a Fedora build.
NODEPS=""
if ! rpm -q rpm >/dev/null 2>&1 || [ ! -f /etc/fedora-release ]; then
    NODEPS="--nodeps"
    say "NOTE: not a Fedora build host; skipping BuildRequires check (--nodeps)."
    say "      The resulting RPM links against this host's glibc and is NOT a"
    say "      substitute for a Fedora-built package."
fi

# rpmbuild only uses SOURCE_DATE_EPOCH for BUILDTIME when told to; without these
# two defines the payload is identical but the header timestamp (and therefore
# the whole file's sha256) changes on every build. Verified: two builds had
# byte-identical binaries and different BUILDTIME (1792652996 vs 1792653080).
rpmbuild -bb $NODEPS \
    --define "_topdir $TOP" \
    --define "debug_package %{nil}" \
    --define "_build_id_links none" \
    --define "__os_install_post %{nil}" \
    --define "_use_source_date_epoch_as_buildtime 1" \
    --define "_source_date_epoch_from_changelog 0" \
    --define "use_source_date_epoch_as_buildtime 1" \
    --define "clamp_mtime_to_source_date_epoch 1" \
    --define "_sourcedir $TOP/SOURCES" \
    --define "_buildhost reproducible" \
    "$ROOT/packaging/rpm/portscope.spec"

# --- collect --------------------------------------------------------------
mkdir -p "$ROOT/dist"
RPM_FILE="$(find "$TOP/RPMS" -name '*.rpm' | head -1)"
[ -n "$RPM_FILE" ] || { echo "error: rpmbuild produced no package" >&2; exit 1; }
cp "$RPM_FILE" "$ROOT/dist/"

say ""
say "Built: $ROOT/dist/$(basename "$RPM_FILE")"
rpm -qip "$ROOT/dist/$(basename "$RPM_FILE")" 2>/dev/null | sed -n '1,14p'
