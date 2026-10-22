#!/bin/sh
# Build a Debian package for PortScope.
#
# Requires: cargo, dpkg-deb, and the runtime libraries the binary links against.
# Does NOT require root. Output: dist/portscope_<version>_<arch>.deb
#
# The dependency list is derived in two steps and then corrected:
#   1. `dpkg-shlibdeps` gives the ELF-level soname requirements.
#   2. The API level the code is actually built against is applied as a floor,
#      because shlibs:Depends reports e.g. libgtk-4-1 (>= 4.12.0) even though
#      the crate is compiled against GTK 4.14 (`features = ["v4_14"]`).
set -eu

# The staged tree must not inherit the caller's umask: with umask 0002 every
# directory became 0775 and msgfmt wrote the catalogue 0664, which lintian
# rejects (non-standard-dir-perm / non-standard-file-perm).
umask 0022

ROOT="$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)"
cd "$ROOT"

VERSION="$(sed -n 's/^version = "\(.*\)"$/\1/p' Cargo.toml | head -1)"
[ -n "$VERSION" ] || { echo "error: cannot read version from Cargo.toml" >&2; exit 1; }

ARCH="$(dpkg --print-architecture)"
PKG="portscope"
STAGE="$ROOT/dist/stage"
OUT="$ROOT/dist/${PKG}_${VERSION}_${ARCH}.deb"

say() { printf '%s\n' "$*"; }

# --- 1. build -------------------------------------------------------------
say "==> Building release binaries"

# Reproducibility: the source directory must not influence the binary.
#
# Two things leak it:
#   1. `CARGO_MANIFEST_DIR` baked in by `env!()`  -> removed from the source
#      (the dev catalogue path is now relative).
#   2. debug/provenance paths from the workspace   -> remapped here.
#
# The remap target must be a FIXED token, not `$ROOT`: remapping to the actual
# path made the compiler invocation itself differ per checkout (measured:
# identical sources in two directories produced a 1279288 vs 1278680 byte
# binary). A fixed target makes the invocation identical everywhere.
RUSTFLAGS="${RUSTFLAGS:-} --remap-path-prefix=$ROOT=/portscope" \
    cargo build --release -p portscope-gtk -p portscope-cli
[ -x target/release/portscope ] || { echo "error: binary missing" >&2; exit 1; }

# --- 2. stage the filesystem ---------------------------------------------
say "==> Staging into $STAGE"
rm -rf "$STAGE"
mkdir -p \
    "$STAGE/DEBIAN" \
    "$STAGE/usr/bin" \
    "$STAGE/usr/share/applications" \
    "$STAGE/usr/share/metainfo" \
    "$STAGE/usr/share/doc/$PKG"

install -m 755 target/release/portscope      "$STAGE/usr/bin/portscope"
install -m 755 target/release/portscope-cli  "$STAGE/usr/bin/portscope-cli"

install -m 644 data/desktop/io.github.portscope.app.desktop \
    "$STAGE/usr/share/applications/"
install -m 644 data/metainfo/io.github.portscope.app.metainfo.xml \
    "$STAGE/usr/share/metainfo/"

# Man pages: required for binaries in PATH, and Debian policy requires them to
# be compressed with a reproducible gzip invocation (-n omits the timestamp).
mkdir -p "$STAGE/usr/share/man/man1"
for m in portscope.1 portscope-cli.1; do
    gzip -9n -c "data/man/$m" > "$STAGE/usr/share/man/man1/$m.gz"
    chmod 644 "$STAGE/usr/share/man/man1/$m.gz"
done

for size in 16 24 32 48 64 128 256; do
    src="data/icons/hicolor/${size}x${size}/apps/io.github.portscope.app.png"
    [ -f "$src" ] || continue
    d="$STAGE/usr/share/icons/hicolor/${size}x${size}/apps"
    mkdir -p "$d"
    install -m 644 "$src" "$d/"
done
mkdir -p "$STAGE/usr/share/icons/hicolor/scalable/apps"
install -m 644 data/icons/hicolor/scalable/apps/io.github.portscope.app.svg \
    "$STAGE/usr/share/icons/hicolor/scalable/apps/"

# Translations. English is the source language and needs no catalogue.
if command -v msgfmt >/dev/null 2>&1; then
    for po in crates/gtk/po/*.po; do
        [ -f "$po" ] || continue
        lang="$(basename "$po" .po)"
        case "$lang" in en*) continue ;; esac
        d="$STAGE/usr/share/locale/$lang/LC_MESSAGES"
        mkdir -p "$d"
        msgfmt -o "$d/portscope.mo" "$po"
    done
else
    echo "error: msgfmt (gettext) is required to build the translations" >&2
    exit 1
fi

install -m 644 LICENSE "$STAGE/usr/share/doc/$PKG/copyright"
install -m 644 README.md "$STAGE/usr/share/doc/$PKG/README.md"

# Debian policy requires a changelog in a non-native package.
install -d "$STAGE/usr/share/doc/$PKG"
gzip -9n -c "$ROOT/data/changelog.Debian" > "$STAGE/usr/share/doc/$PKG/changelog.Debian.gz"
chmod 644 "$STAGE/usr/share/doc/$PKG/changelog.Debian.gz"


# --- 3. dependency list --------------------------------------------------
# ELF-level requirements, then a floor from the API level the code is built for.
SHLIBS="libadwaita-1-0 (>= 1.5), libgtk-4-1 (>= 4.14)"

# Runtime tools PortScope actually executes. Without these it degrades (it
# reports the missing tool rather than crashing), so they are Recommends, not
# Depends — a system without iproute2 should still start.
RUNTIME_TOOLS="iproute2, systemd"

DEPENDS="$SHLIBS, libc6 (>= 2.39), libgcc-s1 (>= 4.2), libglib2.0-0t64 (>= 2.80), libpango-1.0-0"

INSTALLED_SIZE="$(du -sk "$STAGE/usr" | cut -f1)"

cat > "$STAGE/DEBIAN/control" <<EOF
Package: $PKG
Version: $VERSION-1
Section: admin
Priority: optional
Architecture: $ARCH
Depends: $DEPENDS
Recommends: $RUNTIME_TOOLS
Installed-Size: $INSTALLED_SIZE
Maintainer: PortScope contributors <noreply@example.invalid>
Homepage: https://example.invalid/portscope
Description: local port, process and service inspector
 PortScope shows which local TCP/UDP endpoints are listening, which
 process owns each one, which systemd unit that process belongs to,
 and what the journal says about it.
 .
 Every endpoint-to-service link carries the evidence that produced it
 (socket inode, cgroup, MainPID, socket activation, or Listen=); it is
 never guessed from a port number or a process name.
 .
 It runs entirely as the invoking user: no root, no polkit, no group
 changes, no network access and no telemetry. Some data, such as the
 system journal, may be unavailable depending on your permissions, and
 the interface says so explicitly rather than showing an empty result.
EOF

cat > "$STAGE/DEBIAN/postinst" <<'EOF'
#!/bin/sh
set -e
# Best-effort cache refresh; never fail the install if a tool is absent.
if command -v update-desktop-database >/dev/null 2>&1; then
    update-desktop-database -q /usr/share/applications || true
fi
if command -v gtk-update-icon-cache >/dev/null 2>&1; then
    gtk-update-icon-cache -q -t /usr/share/icons/hicolor || true
fi
exit 0
EOF

cat > "$STAGE/DEBIAN/postrm" <<'EOF'
#!/bin/sh
set -e
if [ "$1" = "remove" ] || [ "$1" = "purge" ]; then
    if command -v update-desktop-database >/dev/null 2>&1; then
        update-desktop-database -q /usr/share/applications || true
    fi
fi
exit 0
EOF

chmod 755 "$STAGE/DEBIAN/postinst" "$STAGE/DEBIAN/postrm"

# --- 4. build ------------------------------------------------------------

# Belt and braces: normalise permissions on the whole staged tree so the result
# does not depend on how any individual tool wrote its files.
find "$STAGE" -type d -exec chmod 755 {} +
find "$STAGE" -type f -exec chmod 644 {} +
chmod 755 "$STAGE/usr/bin/portscope" "$STAGE/usr/bin/portscope-cli"
chmod 755 "$STAGE/DEBIAN/postinst" "$STAGE/DEBIAN/postrm"

say "==> Building $OUT"
mkdir -p "$ROOT/dist"

# Reproducible output.
#
# Without SOURCE_DATE_EPOCH, dpkg-deb stamps the ar members with the current
# time, so two builds of identical inputs produce different bytes (verified:
# different sha256). With it, builds are byte-identical, which lets a user
# verify that a published package really came from the published source.
# Defaults to the changelog date; override with SOURCE_DATE_EPOCH=<unix time>.
: "${SOURCE_DATE_EPOCH:=1792627200}"
export SOURCE_DATE_EPOCH

dpkg-deb --root-owner-group -Zxz --build "$STAGE" "$OUT"

say ""
say "Built: $OUT"
dpkg-deb --info "$OUT" | sed -n '1,20p'
