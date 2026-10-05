#!/usr/bin/env bash
# Lays out what the Linux release archive holds, in a fresh directory:
#
#   packaging/linux/stage.sh target/release/leon work/stage
#
#   leon                                                    the binary
#   share/applications/dev.zavu.leon.desktop                the desktop entry
#   share/icons/hicolor/<N>x<N>/apps/dev.zavu.leon.png      the icon, 16 to 512
#   INSTALL.txt, LICENSE, NOTICE
#
# `cargo xtask package` then packs the entries of that directory.
set -euo pipefail

binary="$1"
out="$2"
here="$(cd "$(dirname "$0")" && pwd)"
root="$(cd "$here/../.." && pwd)"
assets="$root/crates/app/assets"

rm -rf "$out"
mkdir -p "$out/share/applications"
cp "$binary" "$out/leon"
chmod 755 "$out/leon"
cp "$assets/linux/dev.zavu.leon.desktop" "$out/share/applications/"
for size in 16 32 48 64 128 256 512; do
  dir="$out/share/icons/hicolor/${size}x${size}/apps"
  mkdir -p "$dir"
  cp "$assets/icons/app-icon-$size.png" "$dir/dev.zavu.leon.png"
done
cp "$here/INSTALL.txt" "$root/LICENSE" "$root/NOTICE" "$out/"
