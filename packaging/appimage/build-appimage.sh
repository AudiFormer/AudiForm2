#!/usr/bin/env bash
# AudiForm 2 as an AppImage, from a Linux program already built.
#
#   cargo build --release -p audiform2
#   packaging/appimage/build-appimage.sh [program] [version]
#
# program defaults to target/release/audiform2, version to the workspace's.
# Needs appimagetool on PATH (or APPIMAGETOOL=/path/to/appimagetool):
# https://github.com/AppImage/appimagetool/releases
#
# The program links only glibc and ALSA (libasound2), which every desktop
# distribution has, so nothing else is bundled. An AppImage runs on
# distributions with the glibc it was built against or newer: build it on
# an old one (the release workflow uses Ubuntu 22.04).
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
BIN="${1:-$ROOT/target/release/audiform2}"
V="${2:-$(sed -n 's/^version = "\(.*\)"/\1/p' "$ROOT/Cargo.toml" | head -1)}"
ID=io.github.audiformer.AudiForm2
TOOL="${APPIMAGETOOL:-appimagetool}"
OUT="${OUT:-$PWD/AudiForm2-${V}-linux-x64.AppImage}"

# A program path given relative to the repository works from anywhere.
[ -e "$BIN" ] || [ ! -e "$ROOT/$BIN" ] || BIN="$ROOT/$BIN"
[ -f "$BIN" ] || {
    echo "No program at $BIN: build it first (cargo build --release -p audiform2)" >&2
    exit 1
}
chmod +x "$BIN" 2>/dev/null || true
command -v "$TOOL" >/dev/null || {
    echo "appimagetool not found: install it (paru -S appimagetool-bin) or set APPIMAGETOOL=/path/to/appimagetool" >&2
    exit 1
}

APPDIR="$(mktemp -d)/AudiForm2.AppDir"
trap 'rm -rf "$(dirname "$APPDIR")"' EXIT

install -Dm755 "$BIN" "$APPDIR/usr/bin/audiform2"
install -Dm644 "$ROOT/packaging/flatpak/$ID.desktop" "$APPDIR/usr/share/applications/$ID.desktop"
install -Dm644 "$ROOT/packaging/flatpak/$ID.metainfo.xml" "$APPDIR/usr/share/metainfo/$ID.appdata.xml"
for s in 64 128 256; do
    install -Dm644 "$ROOT/assets/icon/audiform2-$s.png" \
        "$APPDIR/usr/share/icons/hicolor/${s}x${s}/apps/$ID.png"
done

# appimagetool wants the desktop entry and icon at the top as well.
ln -s "usr/share/applications/$ID.desktop" "$APPDIR/$ID.desktop"
ln -s "usr/share/icons/hicolor/256x256/apps/$ID.png" "$APPDIR/$ID.png"
ln -s "$ID.png" "$APPDIR/.DirIcon"

cat > "$APPDIR/AppRun" <<'EOF'
#!/bin/sh
HERE="$(dirname "$(readlink -f "$0")")"
exec "$HERE/usr/bin/audiform2" "$@"
EOF
chmod +x "$APPDIR/AppRun"

ARCH=x86_64 "$TOOL" --no-appstream "$APPDIR" "$OUT"
echo "Built $OUT"
