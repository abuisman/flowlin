#!/bin/sh
# Install a release build without meson.
#   ./build-aux/install.sh            -> ~/.local  (no root needed)
#   sudo ./build-aux/install.sh /usr/local
set -eu
PREFIX="${1:-$HOME/.local}"
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"
[ -x target/release/flowlin ] || cargo build --release

install -Dm755 target/release/flowlin "$PREFIX/bin/flowlin"
install -Dm644 data/eu.ronkatil.Flowlin.desktop "$PREFIX/share/applications/eu.ronkatil.Flowlin.desktop"
install -Dm644 data/eu.ronkatil.Flowlin.metainfo.xml "$PREFIX/share/metainfo/eu.ronkatil.Flowlin.metainfo.xml"
for icon in data/icons/hicolor/*x*/apps/eu.ronkatil.Flowlin.png; do
  install -Dm644 "$icon" "$PREFIX/share/${icon#data/}"
done
install -Dm644 data/icons/hicolor/symbolic/apps/eu.ronkatil.Flowlin-symbolic.svg \
  "$PREFIX/share/icons/hicolor/symbolic/apps/eu.ronkatil.Flowlin-symbolic.svg"
install -Dm644 data/eu.ronkatil.Flowlin.gschema.xml "$PREFIX/share/glib-2.0/schemas/eu.ronkatil.Flowlin.gschema.xml"
glib-compile-schemas "$PREFIX/share/glib-2.0/schemas"
command -v update-desktop-database >/dev/null && update-desktop-database -q "$PREFIX/share/applications" || true
command -v gtk-update-icon-cache >/dev/null && gtk-update-icon-cache -qtf "$PREFIX/share/icons/hicolor" || true

case "$PREFIX" in
  "$HOME"/*) echo "Installed to $PREFIX. If schemas are not found, add $PREFIX/share to XDG_DATA_DIRS." ;;
  *) echo "Installed to $PREFIX." ;;
esac
