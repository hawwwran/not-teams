#!/usr/bin/env bash
# Installs NotTeams for the current user only (no root needed): the program into ~/.local/bin,
# its menu entry and icon into ~/.local/share. Run it from anywhere after `cargo build --release`.
set -eu
here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
app="$here/.."
install -Dm755 "$app/target/release/notteams" "$HOME/.local/bin/notteams"
install -Dm644 "$here/com.hawwwran.notteams.desktop" "$HOME/.local/share/applications/com.hawwwran.notteams.desktop"
install -Dm644 "$here/com.hawwwran.notteams.svg" "$HOME/.local/share/icons/hicolor/scalable/apps/com.hawwwran.notteams.svg"
# Earlier builds were called teams-rs; their menu entry and icon would linger otherwise.
rm -f "$HOME/.local/share/applications/com.hawwwran.teams-rs.desktop" \
      "$HOME/.local/share/icons/hicolor/scalable/apps/com.hawwwran.teams-rs.svg"
update-desktop-database "$HOME/.local/share/applications" 2>/dev/null || true
gtk-update-icon-cache -q "$HOME/.local/share/icons/hicolor" 2>/dev/null || true
echo "NotTeams installed: find it in your app menu, or run 'notteams'."
