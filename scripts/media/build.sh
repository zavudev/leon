#!/usr/bin/env bash
# Regenerates everything the README shows: the demo workspace, every still,
# every clip, and the encoded files in docs/media.
#
#   cargo build --release -p leon
#   scripts/media/build.sh
#
# Needs a running Hyprland session (nested.sh hides its guest on a special
# workspace) and the tools the README lists: bwrap, grim, wtype, hyprctl,
# ffmpeg, python3, ImageMagick optional.
set -euo pipefail

MEDIA_ROOT="${MEDIA_ROOT:-/tmp/leon-media}"
ROOT=$(cd "$(dirname "$0")/../.." && pwd)
export MEDIA_ROOT

if [ -f "$MEDIA_ROOT/nested.env" ]; then
    # Reuse only a live guest; a stale env file after a server restart is
    # not evidence that the compositor still exists.
    # shellcheck source=/dev/null
    if ! ( . "$MEDIA_ROOT/nested.env"; HYPRLAND_INSTANCE_SIGNATURE="$NSIG" hyprctl monitors >/dev/null 2>&1 ); then
        rm -f "$MEDIA_ROOT/nested.env"
    fi
fi
if [ ! -f "$MEDIA_ROOT/nested.env" ]; then
    bash "$ROOT/scripts/media/nested.sh"
fi

bash "$ROOT/scripts/media/prepare-demo.sh"
bash "$ROOT/scripts/media/stills.sh"
bash "$ROOT/scripts/media/clips.sh" all
bash "$ROOT/scripts/media/encode.sh" "$MEDIA_ROOT/out"

mkdir -p "$ROOT/docs/media/video"
cp "$MEDIA_ROOT"/capture/0*.png "$ROOT/docs/media/"
cp "$MEDIA_ROOT"/capture/10-*.png "$ROOT/docs/media/"
cp "$MEDIA_ROOT"/out/*.mp4 "$MEDIA_ROOT"/out/*.webp "$ROOT/docs/media/video/"

echo
echo "docs/media now holds:"
ls -la "$ROOT/docs/media" "$ROOT/docs/media/video"
