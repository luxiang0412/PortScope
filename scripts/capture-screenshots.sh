#!/bin/sh
# Regenerate the public screenshots from FICTIONAL data.
#
# Every screenshot in the repository must be safe to publish. Real-data captures
# contain private IP addresses and the developer's own unit names, so the app is
# run with PORTSCOPE_DEMO_DATA=1, which substitutes a fixed fictional dataset
# (docs address ranges only) and marks the status bar with "Sample data".
set -eu

ROOT="$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)"
cd "$ROOT"

: "${DISPLAY:=:1}"
: "${XAUTHORITY:=$HOME/.Xauthority}"
export DISPLAY XAUTHORITY

BIN="target/release/portscope"
OUT="evidence/screenshots"
[ -x "$BIN" ] || { echo "build first: cargo build --release -p portscope-gtk" >&2; exit 1; }
command -v wmctrl >/dev/null || { echo "wmctrl required" >&2; exit 1; }
command -v import >/dev/null || { echo "ImageMagick 'import' required" >&2; exit 1; }
mkdir -p "$OUT"

shot() {  # shot <file> <width> <height>
    w=$(wmctrl -lx | awk '/portscope.portscope/{print $1}' | head -1)
    [ -n "$w" ] || { echo "window not found" >&2; return 1; }
    wmctrl -i -r "$w" -e "0,0,0,$2,$3"
    sleep 2
    import -window "$w" "$OUT/$1"
    echo "  $1"
}

run() {  # run <lang>
    pkill -x portscope 2>/dev/null || true
    sleep 1
    GDK_SCALE=1 LC_ALL="$1" PORTSCOPE_DEMO_DATA=1 "$BIN" >/dev/null 2>&1 &
    sleep 10
}

echo "==> English"
run en_US.UTF-8
shot phase-public-en-ports-1400.png 1400 860
shot phase-public-en-ports-1024.png 1024 600
shot phase-public-en-ports-1680.png 1680 900

echo "==> 简体中文"
run zh_CN.UTF-8
shot phase-public-zh-ports-1400.png 1400 860
shot phase-public-zh-ports-1024.png 1024 600

pkill -x portscope 2>/dev/null || true
echo "Done."
