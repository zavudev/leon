#!/usr/bin/env bash
# Helpers to talk to the nested compositor and capture its headless output.
# Requires $MEDIA_ROOT/nested.env, written by nested.sh.
set -u
MEDIA_ROOT="${MEDIA_ROOT:-/tmp/leon-media}"
# shellcheck source=/dev/null
. "$MEDIA_ROOT/nested.env"

nctl() { HYPRLAND_INSTANCE_SIGNATURE="$NSIG" hyprctl "$@"; }
capture() { WAYLAND_DISPLAY="$NESTED_DISPLAY" grim -o HEADLESS-1 "$1"; }
