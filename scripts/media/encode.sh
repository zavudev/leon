#!/usr/bin/env bash
# Encodes every recorded clip into an MP4 (for download) and an animated
# WebP (for the inline, autoplaying version).
#
#   scripts/media/encode.sh [output-dir]
set -euo pipefail

MEDIA_ROOT="${MEDIA_ROOT:-/tmp/leon-media}"
SRC="$MEDIA_ROOT/frames"
OUT="${1:-$MEDIA_ROOT/out}"
mkdir -p "$OUT"

encode() {
    local name=$1 dir=$2
    local frames=0 file
    for file in "$dir"/*.png; do
        [ -f "$file" ] && frames=$((frames + 1))
    done
    echo "$name: $frames frames"
    ffmpeg -y -loglevel error -framerate 20 -i "$dir/%05d.png" \
        -c:v libx264 -pix_fmt yuv420p -crf 24 -preset slow -movflags +faststart \
        "$OUT/$name.mp4"
    ffmpeg -y -loglevel error -framerate 20 -i "$dir/%05d.png" \
        -vf "scale=1200:-1:flags=lanczos" \
        -c:v libwebp_anim -lossless 0 -compression_level 6 -q:v 62 -loop 0 -preset default \
        "$OUT/$name.webp"
}

encode hero "$SRC/hero"
encode resume-a-session "$SRC/resume"
encode panes-and-tabs "$SRC/panes"
encode themes "$SRC/themes"

ls -la "$OUT"
