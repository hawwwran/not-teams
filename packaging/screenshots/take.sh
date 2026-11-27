#!/usr/bin/env bash
# Retakes the store listing's screenshots from NotTeams' demo mode: made-up people and chats,
# no network, and settings, cache and data in a scratch folder of their own (src/demo.rs).
# Run after `cargo build --release`, with the mouse pointer off the screen's centre so no
# message shows its hover toolbar. The metainfo points at these files on GitHub's main.
set -euo pipefail
here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
app="$here/../../target/release/notteams"
scratch=$(mktemp -d)
trap 'rm -rf "$scratch"' EXIT

shot() {
    local name=$1
    shift
    env NOTTEAMS_DEMO=1 NOTTEAMS_NO_DESKTOP_ENTRY=1 NOTTEAMS_WINDOW_SIZE=1280x800 NOTTEAMS_SHOT_SECS=6 "$@" \
        "$app" --screenshot "$scratch/$name.png"
    # The app reports a failed save but still exits 0.
    if [ ! -s "$scratch/$name.png" ]; then
        echo "No $name.png was written" >&2
        exit 1
    fi
    mv "$scratch/$name.png" "$here/$name.png"
}

shot chat NOTTEAMS_OPEN_CHAT=maya
shot calendar NOTTEAMS_OPEN_CALENDAR=1
echo "Saved chat.png and calendar.png in $here"
