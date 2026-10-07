#!/usr/bin/env bash
# Captures frames of the nested headless output at a steady rate until a
# .stop file appears in the frame directory.
#
#   scripts/media/record.sh <frames-dir> [fps]
set -u
MEDIA_ROOT="${MEDIA_ROOT:-/tmp/leon-media}"
ROOT=$(cd "$(dirname "$0")/../.." && pwd)
# shellcheck source=/dev/null
. "$ROOT/scripts/media/rig.sh"

DIR="$1"
FPS="${2:-20}"
rm -rf "$DIR"
mkdir -p "$DIR"

INTERVAL=$(python3 -c "print(1.0/$FPS)")
i=0
while [ ! -f "$DIR/.stop" ]; do
    START=$(date +%s.%N)
    WAYLAND_DISPLAY="$NESTED_DISPLAY" grim -o HEADLESS-1 "$DIR/$(printf '%05d' "$i").png" 2>/dev/null || true
    i=$((i + 1))
    NOW=$(date +%s.%N)
    python3 - "$START" "$NOW" "$INTERVAL" <<'EOF'
import sys, time
start, now, interval = float(sys.argv[1]), float(sys.argv[2]), float(sys.argv[3])
rest = interval - (now - start)
if rest > 0:
    time.sleep(rest)
EOF
done
echo "captured $i frames in $DIR"
