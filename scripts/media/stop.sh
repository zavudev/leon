#!/usr/bin/env bash
# Stops the nested compositor, the demo Leon and the capture surface that
# these scripts created. The person's own Hyprland session and their
# ~/.local/bin/leon are not touched.
set -u
MEDIA_ROOT="${MEDIA_ROOT:-/tmp/leon-media}"

for pid in $(pgrep -f "^Hyprland --config $MEDIA_ROOT/nested.conf" || true); do
    kill "$pid" 2>/dev/null || true
done
for pid in $(pgrep -f "^dbus-run-session -- env .*$MEDIA_ROOT/nested.conf" || true); do
    kill "$pid" 2>/dev/null || true
done
for pid in $(pgrep -f "^$MEDIA_ROOT/bin/leon" || true); do
    kill "$pid" 2>/dev/null || true
done
for pid in $(pgrep -f "bwrap .*$MEDIA_ROOT/bin/leon" || true); do
    kill "$pid" 2>/dev/null || true
done

# Remove the invisible output the guest window was parked on.
if [ -f "$MEDIA_ROOT/nested.env" ]; then
    # shellcheck source=/dev/null
    . "$MEDIA_ROOT/nested.env"
    if [ -n "${HOST_OUTPUT:-}" ]; then
        hyprctl output remove "$HOST_OUTPUT" >/dev/null 2>&1 || true
    fi
fi
rm -f "$MEDIA_ROOT/nested.env"
exit 0
