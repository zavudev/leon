#!/usr/bin/env bash
# Starts a Hyprland nested inside the current session and gives the capture
# rig a place to live: the guest's own window is moved to a headless output
# of the host, which is invisible to the person but still rendered by their
# compositor (a hidden window stops receiving frames, and the guest freezes).
#
# Writes $MEDIA_ROOT/nested.env with the instance signature, the guest's
# socket and the host output the guest window sits on.
set -euo pipefail

MEDIA_ROOT="${MEDIA_ROOT:-/tmp/leon-media}"
mkdir -p "$MEDIA_ROOT"

cat > "$MEDIA_ROOT/nested.conf" <<'CONF'
# The guest compositor for the README media. It has no keybinds and no
# decoration: it exists to host one window at a known size.
monitor = WAYLAND-1, 800x600, 0x0, 1

general {
    border_size = 0
    gaps_in = 0
    gaps_out = 0
}

decoration {
    blur {
        enabled = false
    }
    shadow {
        enabled = false
    }
}

animations { enabled = false }

misc {
    disable_hyprland_logo = true
    disable_splash_rendering = true
    force_default_wallpaper = 0
}

cursor { no_hardware_cursors = true }

debug { enable_stdout_logs = true }
CONF

latest_instance() {
    find "${XDG_RUNTIME_DIR}/hypr" -mindepth 1 -maxdepth 1 -type d -printf '%T@ %f\n' 2>/dev/null \
        | sort -nr | sed -n '1s/^[^ ]* //p'
}

before=$(latest_instance)
nohup dbus-run-session -- env -u HYPRLAND_INSTANCE_SIGNATURE XDG_CURRENT_DESKTOP=Hyprland \
    HYPRLAND_NO_SD_VARS=1 HYPRLAND_NO_SD_NOTIFY=1 \
    Hyprland --config "$MEDIA_ROOT/nested.conf" > "$MEDIA_ROOT/nested.log" 2>&1 &

for _ in $(seq 1 60); do
    sleep 0.25
    sig=$(latest_instance)
    [ -n "$sig" ] && [ "$sig" != "$before" ] && break
done
[ -n "${sig:-}" ] || { echo "the nested compositor did not start" >&2; exit 1; }

lock="${XDG_RUNTIME_DIR}/hypr/$sig/hyprland.lock"
for _ in $(seq 1 40); do
    [ -s "$lock" ] && break
    sleep 0.25
done

pid=$(sed -n 1p "$lock")
socket=$(sed -n 2p "$lock")
kill -0 "$pid" 2>/dev/null || { echo "the nested compositor died; see $MEDIA_ROOT/nested.log" >&2; exit 1; }

# The capture surface: a headless output at a fixed size, inside the guest.
HYPRLAND_INSTANCE_SIGNATURE="$sig" hyprctl output create headless >/dev/null
HYPRLAND_INSTANCE_SIGNATURE="$sig" hyprctl keyword monitor HEADLESS-1,1600x1000@60,0x0,1 >/dev/null

# An invisible place on the host for the guest's own window: a headless
# output of the person's compositor. Rendered (so the guest keeps getting
# frames) but shown nowhere.
host_before=$(hyprctl monitors -j | python3 -c "import json, sys; print(' '.join(m['name'] for m in json.load(sys.stdin)))")
hyprctl output create headless >/dev/null
sleep 0.5
host_output=$(hyprctl monitors -j | python3 -c "
import json, sys
before = set('$host_before'.split())
for m in json.load(sys.stdin):
    if m['name'] not in before:
        print(m['name']); break
" 2>/dev/null || true)
host_ws=""
if [ -n "${host_output:-}" ]; then
    host_ws=$(hyprctl monitors -j | python3 -c "
import json, sys
for m in json.load(sys.stdin):
    if m['name'] == '$host_output': print(m['activeWorkspace']['name']); break
" 2>/dev/null || true)
fi

# Move the guest window (class "aquamarine") there; hosts with the Lua
# config manager take the first form, legacy configs the second.
if [ -n "${host_ws:-}" ]; then
    for _ in $(seq 1 40); do
        addr=$(hyprctl clients -j | python3 -c "
import json, sys
for w in json.load(sys.stdin):
    if w.get('class') == 'aquamarine' or 'aquamarine' in (w.get('title') or ''):
        print(w['address']); break
" 2>/dev/null || true)
        [ -n "${addr:-}" ] && break
        sleep 0.25
    done
    if [ -n "${addr:-}" ]; then
        hyprctl repl "hl.dispatch(hl.dsp.window.move{workspace='$host_ws', window='address:$addr'})" >/dev/null 2>&1 \
            || hyprctl dispatch movetoworkspacesilent "$host_ws,address:$addr" >/dev/null 2>&1 \
            || true
    fi
fi

cat > "$MEDIA_ROOT/nested.env" <<ENV
NSIG=$sig
NESTED_DISPLAY=$socket
HOST_OUTPUT=${host_output:-}
ENV

echo "nested compositor: $sig on $socket (host output: ${host_output:-none})"
