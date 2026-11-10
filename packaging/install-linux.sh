#!/bin/sh
# Installs Awra Studio for the current user (~/.local): the binary, the
# menu entry and the icon. Run after a release build.
set -e
here="$(cd "$(dirname "$0")" && pwd)"
bin="$here/../target/release/awra-studio"
[ -x "$bin" ] || { echo "Construis d'abord : cargo build --release -p awra-studio" >&2; exit 1; }
install -Dm755 "$bin" "$HOME/.local/bin/awra-studio"
install -Dm644 "$here/awra-studio.desktop" "$HOME/.local/share/applications/org.awra.Studio.desktop"
install -Dm644 "$here/org.awra.Studio.svg" "$HOME/.local/share/icons/hicolor/scalable/apps/org.awra.Studio.svg"
command -v update-desktop-database >/dev/null && update-desktop-database "$HOME/.local/share/applications" || true
echo "Awra Studio est installé (menu des applications, ou « awra-studio »)."
