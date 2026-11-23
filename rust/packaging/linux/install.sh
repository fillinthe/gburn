#!/bin/sh
# Puts GBurn into ~/.local (no root): the program, a menu entry and its icon. Run from the unpacked folder.
#   sh install.sh          install        sh install.sh remove     take it out again
set -e
HERE="$(cd "$(dirname "$0")" && pwd)"
BIN="$HOME/.local/bin"; APPS="$HOME/.local/share/applications"; ICONS="$HOME/.local/share/icons/hicolor/512x512/apps"
if [ "${1:-}" = "remove" ]; then
  rm -f "$BIN/gburn" "$APPS/gburn.desktop" "$ICONS/gburn.png"
  echo "GBurn removed from ~/.local"; exit 0
fi
mkdir -p "$BIN" "$APPS" "$ICONS"
cp "$HERE/gburn" "$BIN/gburn" && chmod +x "$BIN/gburn"
cp "$HERE/gburn.png" "$ICONS/gburn.png"
# the icon by its full path: a bare name needs an icon-theme cache that Xfce and others don't refresh for ~/.local
sed -e "s|^Exec=gburn|Exec=$BIN/gburn|" -e "s|^Icon=gburn|Icon=$ICONS/gburn.png|" "$HERE/gburn.desktop" > "$APPS/gburn.desktop"
command -v update-desktop-database >/dev/null && update-desktop-database "$APPS" 2>/dev/null || true
echo "GBurn installed: $BIN/gburn (menu entry added). Needs OpenGL (libGL) and, for file dialogs, an xdg-desktop-portal."
case ":$PATH:" in *":$BIN:"*) ;; *) echo "Note: $BIN is not on your PATH; the menu entry works anyway." ;; esac
