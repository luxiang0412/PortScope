#!/bin/sh
# Install PortScope into a prefix (default: $HOME/.local).
#
# Deliberately unprivileged: PortScope runs as the invoking user, so installing
# it into the user's own prefix is the expected path. Use PREFIX=/usr with sudo
# only if you are building a system package.
#
#   ./scripts/install.sh                 # -> ~/.local
#   PREFIX=/usr ./scripts/install.sh     # system-wide (needs write access)
#   DESTDIR=/tmp/stage ./scripts/install.sh   # packaging
set -eu

PREFIX="${PREFIX:-$HOME/.local}"
DESTDIR="${DESTDIR:-}"
APPDIR="${DESTDIR}${PREFIX}"

BIN_DST="$APPDIR/bin"
DATA_DST="$APPDIR/share"

SRC_DIR="$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)"

say() { printf '%s\n' "$*"; }
die() { printf 'error: %s\n' "$*" >&2; exit 1; }

[ -x "$SRC_DIR/target/release/portscope" ] \
  || die "target/release/portscope not found. Run: cargo build --release -p portscope-gtk"

say "Installing PortScope to $APPDIR"

# --- binaries -------------------------------------------------------------
install -d "$BIN_DST"
install -m 755 "$SRC_DIR/target/release/portscope" "$BIN_DST/portscope"
if [ -x "$SRC_DIR/target/release/portscope-cli" ]; then
    install -m 755 "$SRC_DIR/target/release/portscope-cli" "$BIN_DST/portscope-cli"
fi

# --- translations ---------------------------------------------------------
# The app looks for <prefix>/share/locale/<lang>/LC_MESSAGES/portscope.mo first.
for po in "$SRC_DIR"/crates/gtk/po/*.po; do
    [ -f "$po" ] || continue
    lang="$(basename "$po" .po)"
    case "$lang" in
        en*) continue ;;   # English is the source language; no catalogue needed
    esac
    mo="$DATA_DST/locale/$lang/LC_MESSAGES/portscope.mo"
    install -d "$(dirname "$mo")"
    if command -v msgfmt >/dev/null 2>&1; then
        msgfmt -o "$mo" "$po"
        say "  translation: $lang"
    else
        say "  WARNING: msgfmt not found; skipping translation $lang"
    fi
done

# --- desktop entry, metainfo, icons --------------------------------------
install -d "$DATA_DST/applications" "$DATA_DST/metainfo"
install -m 644 "$SRC_DIR/data/desktop/io.github.portscope.app.desktop" \
    "$DATA_DST/applications/io.github.portscope.app.desktop"
install -m 644 "$SRC_DIR/data/metainfo/io.github.portscope.app.metainfo.xml" \
    "$DATA_DST/metainfo/io.github.portscope.app.metainfo.xml"

for size in 16 24 32 48 64 128 256; do
    src="$SRC_DIR/data/icons/hicolor/${size}x${size}/apps/io.github.portscope.app.png"
    [ -f "$src" ] || continue
    dst="$DATA_DST/icons/hicolor/${size}x${size}/apps"
    install -d "$dst"
    install -m 644 "$src" "$dst/"
done
if [ -f "$SRC_DIR/data/icons/hicolor/scalable/apps/io.github.portscope.app.svg" ]; then
    install -d "$DATA_DST/icons/hicolor/scalable/apps"
    install -m 644 "$SRC_DIR/data/icons/hicolor/scalable/apps/io.github.portscope.app.svg" \
        "$DATA_DST/icons/hicolor/scalable/apps/"
fi

# --- caches (best effort; harmless if the tools are absent) --------------
if [ -z "$DESTDIR" ]; then
    command -v update-desktop-database >/dev/null 2>&1 \
        && update-desktop-database "$DATA_DST/applications" 2>/dev/null || true
    command -v gtk-update-icon-cache >/dev/null 2>&1 \
        && gtk-update-icon-cache -q -t "$DATA_DST/icons/hicolor" 2>/dev/null || true
    command -v update-mime-database >/dev/null 2>&1 || true
fi

say ""
say "Done."
case ":$PATH:" in
    *":$BIN_DST:"*) ;;
    *) say "NOTE: $BIN_DST is not on your PATH. Add it, or run $BIN_DST/portscope" ;;
esac
