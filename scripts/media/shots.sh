#!/usr/bin/env bash
# Keyboard helpers for the capture scripts.
#
# Keys are sent straight to the application's window with Hyprland's
# `sendshortcut` dispatcher: it needs no focus, and wtype's modifier handling
# leaks state in the guest. The names are xkb keysyms, so they are
# case-sensitive ("Return", "space", "Up").
set -u
MEDIA_ROOT="${MEDIA_ROOT:-/tmp/leon-media}"
ROOT=$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)
# shellcheck source=/dev/null
. "$ROOT/scripts/media/rig.sh"

OUT="$MEDIA_ROOT/capture"
mkdir -p "$OUT"
APP='class:^(dev.zavu.leon)$'

# key <MODS> <KEYSYM>
key() { nctl dispatch sendshortcut "$1,$2,$APP" >/dev/null 2>&1; sleep 0.12; }
k() { key "" "$1"; }
combo() { key "$1 $2" "$3"; }
ctrl() { key CTRL "$1"; }
text() {
    local s=$1 i c
    for ((i = 0; i < ${#s}; i++)); do
        c=${s:i:1}
        case "$c" in
            " ") key "" space ;;
            "-") key "" minus ;;
            "?") key SHIFT slash ;;
            *) key "" "$c" ;;
        esac
    done
}
pause() { sleep "${1:-0.6}"; }
park_cursor() { nctl dispatch movecursor 1590 985 >/dev/null 2>&1 || true; }
shot() { park_cursor; sleep 0.4; capture "$OUT/$1"; echo "shot: $1"; }

# Go to a project, worktree, session or machine by name.
goto() { combo ctrl shift k; pause 0.5; text "$1"; pause 0.8; k Return; pause 1.2; }

# A live agent session in the given worktree, started through the UI.
new_session() { goto "$1"; combo ctrl shift a; pause 1.2; k Return; pause 3.5; }

# A shell in the selected worktree.
open_shell() { combo ctrl shift t; pause 2.0; }

# Press a key in the focused terminal (the mock agents wait for one).
nudge() { k space; pause "${1:-0.5}"; }

back_to_sidebar() { combo ctrl shift s; pause 0.5; }

# Starts the frame recorder in the background; returns its pid through
# $RECORDER and stops it with stop_recording.
start_recording() {
    bash "$ROOT/scripts/media/record.sh" "$1" 20 &
    RECORDER=$!
}
stop_recording() {
    touch "$1/.stop"
    wait "$RECORDER"
}
