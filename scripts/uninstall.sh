#!/bin/sh
# Uninstall PortScope from a prefix (default: $HOME/.local).
#
# Removes only the files install.sh creates. User settings
# ($XDG_CONFIG_HOME/portscope) are left in place unless --purge is given, because
# deleting a preference the user set is not the uninstaller's call.
set -eu

PREFIX="${PREFIX:-$HOME/.local}"
DESTDIR="${DESTDIR:-}"
APPDIR="${DESTDIR}${PREFIX}"
PURGE=0
[ "${1:-}" = "--purge" ] && PURGE=1

say() { printf '%s\n' "$*"; }

say "Removing PortScope from $APPDIR"

rm -f "$APPDIR/bin/portscope" "$APPDIR/bin/portscope-cli"
rm -f "$APPDIR/share/applications/io.github.portscope.app.desktop"
rm -f "$APPDIR/share/metainfo/io.github.portscope.app.metainfo.xml"

for size in 16 24 32 48 64 128 256; do
    rm -f "$APPDIR/share/icons/hicolor/${size}x${size}/apps/io.github.portscope.app.png"
done
rm -f "$APPDIR/share/icons/hicolor/scalable/apps/io.github.portscope.app.svg"

# Generated caches. They are shared with other applications, so only remove
# them when they exist and after refreshing them, rather than deleting a file
# that other apps still rely on.
if [ -z "$DESTDIR" ]; then
    command -v update-desktop-database >/dev/null 2>&1 \
        && update-desktop-database "$APPDIR/share/applications" 2>/dev/null || true
    command -v gtk-update-icon-cache >/dev/null 2>&1 \
        && gtk-update-icon-cache -q -t "$APPDIR/share/icons/hicolor" 2>/dev/null || true
fi
# If nothing else is installed there, the caches are ours to remove.
if [ -d "$APPDIR/share/applications" ] \
   && [ "$(ls -A "$APPDIR/share/applications" | grep -v 'mimeinfo.cache' | wc -l)" -eq 0 ]; then
    rm -f "$APPDIR/share/applications/mimeinfo.cache"
fi
if [ -d "$APPDIR/share/icons/hicolor" ] \
   && [ "$(find "$APPDIR/share/icons/hicolor" -name '*.png' -o -name '*.svg' | wc -l)" -eq 0 ]; then
    rm -f "$APPDIR/share/icons/hicolor/icon-theme.cache"
fi

for po in "$PWD"/crates/gtk/po/*.po; do
    [ -f "$po" ] || continue
    lang="$(basename "$po" .po)"
    case "$lang" in en*) continue ;; esac
    rm -f "$APPDIR/share/locale/$lang/LC_MESSAGES/portscope.mo"
done

if [ "$PURGE" = "1" ]; then
    cfg="${XDG_CONFIG_HOME:-$HOME/.config}/portscope"
    rm -rf "$cfg"
    say "Purged settings: $cfg"
else
    say "Kept settings in ${XDG_CONFIG_HOME:-$HOME/.config}/portscope (use --purge to remove)"
fi

say "Done."
