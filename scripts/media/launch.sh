#!/usr/bin/env bash
# Launches this worktree's release build inside the nested compositor, with
# the demo home mounted at /home/ada and the demo store. The person's own
# Leon (~/.local/bin/leon) is never touched.
#
#   scripts/media/launch.sh            reuse the store
#   scripts/media/launch.sh --reseed   seed it again from scratch
set -euo pipefail

MEDIA_ROOT="${MEDIA_ROOT:-/tmp/leon-media}"
ROOT=$(cd "$(dirname "$0")/../.." && pwd)
LEON="$ROOT/target/release/leon"
LEON_COPY="$MEDIA_ROOT/bin/leon"

# shellcheck source=/dev/null
. "$ROOT/scripts/media/rig.sh"

# Only this build; the person's own Leon lives at ~/.local/bin/leon.
for pid in $(pgrep -f "^$LEON_COPY" || true); do kill "$pid" 2>/dev/null || true; done
for pid in $(pgrep -f "^$LEON" || true); do kill "$pid" 2>/dev/null || true; done
sleep 0.8

if [ ! -x "$LEON" ]; then
    echo "build it first: cargo build --release -p leon" >&2
    exit 1
fi

# The binary lives under the real home, which the sandbox replaces with a
# tmpfs, so run a copy from outside it.
mkdir -p "$MEDIA_ROOT/bin"
if [ ! -f "$LEON_COPY" ] || [ "$LEON" -nt "$LEON_COPY" ]; then
    cp "$LEON" "$LEON_COPY"
fi

if [ "${1:-}" = "--reseed" ]; then
    rm -rf "$MEDIA_ROOT/data"
fi
if [ ! -f "$MEDIA_ROOT/data/leon.db" ]; then
    (cd "$ROOT" && cargo run -q -p leon-core --example seed_demo -- "$MEDIA_ROOT/data" /home/ada)
fi

# A clean look for the media: dark Leon theme, no usage strip in the footer.
if [ ! -f "$MEDIA_ROOT/data/settings.json" ]; then
    cat > "$MEDIA_ROOT/data/settings.json" <<'JSON'
{
  "theme": "dark",
  "theme_id": "leon",
  "usage_bar": false
}
JSON
fi

nohup bwrap \
    --dev-bind / / \
    --tmpfs /home \
    --bind "$MEDIA_ROOT/home" /home/ada \
    --setenv HOME /home/ada \
    --setenv USER ada \
    --setenv HOSTNAME workstation \
    --setenv XDG_DATA_HOME /home/ada/.local/share \
    --setenv XDG_CONFIG_HOME /home/ada/.config \
    --setenv XDG_STATE_HOME /home/ada/.local/state \
    --setenv XDG_CACHE_HOME /home/ada/.cache \
    --setenv PATH /home/ada/bin:/usr/local/bin:/usr/bin:/bin \
    --setenv WAYLAND_DISPLAY "$NESTED_DISPLAY" \
    "$LEON_COPY" --data-dir "$MEDIA_ROOT/data" > "$MEDIA_ROOT/leon.log" 2>&1 &

for _ in $(seq 1 40); do
    sleep 0.25
    ADDR=$(nctl clients -j 2>/dev/null | python3 -c "
import json, sys
try: ws = json.load(sys.stdin)
except Exception: sys.exit(0)
for w in ws:
    if w.get('class') == 'dev.zavu.leon': print(w['address']); break
" 2>/dev/null)
    if [ -n "$ADDR" ]; then
        # Put the window on the headless output, fullscreen, so every
        # capture is the whole 1600x1000 surface.
        ws=$(nctl monitors -j | python3 -c "
import json, sys
for m in json.load(sys.stdin):
    if m['name'] == 'HEADLESS-1': print(m['activeWorkspace']['name']); break
" 2>/dev/null)
        if [ -n "${ws:-}" ]; then
            nctl dispatch movetoworkspacesilent "$ws,address:$ADDR" >/dev/null
        fi
        nctl dispatch focuswindow "address:$ADDR" >/dev/null
        nctl dispatch fullscreen "address:$ADDR" >/dev/null
        echo "leon up: $ADDR"
        exit 0
    fi
done

echo "leon did not open; see $MEDIA_ROOT/leon.log" >&2
tail -5 "$MEDIA_ROOT/leon.log" >&2
exit 1
