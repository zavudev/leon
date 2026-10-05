#!/usr/bin/env bash
# Regenerates every application icon from the one source,
# crates/app/assets/brand/app-icon.svg (the glare mark on its ink tile, round 2
# of the brand work, the same file as brand/logo/final/app-icon.svg).
#
#   scripts/generate-icons.sh
#
# Needs rsvg-convert and python3 (librsvg, `brew install librsvg`); the .icns
# also needs iconutil, which only macOS has: elsewhere it is left as it is.
#
# The accent of the mark is ICON_ACCENT below, the one place the icon's colour
# is set (acid yellow, the owner's decision): change it and run this script
# again. The 16 and 32 px pictures use the fitted geometry of the mark
# (assets/brand/leon-mark-16.svg: no slot, taller eyes, heavier nose) in the
# same tile when ICON_FITTED_SMALL=1.
#
# Writes into crates/app/assets/icons:
#   app-icon-{16,32,48,64,128,256,512}.png   full-bleed square: Linux hicolor
#                                            sizes, the window icon (256)
#   app-icon-macos-{16..1024}.png            the artwork inset in the rounded
#                                            square of the macOS icon grid
#   app-icon.ico                             16 24 32 48 64 128 256, the icon
#                                            resource of the Windows executable
#   app-icon.icns                            the macOS icon, cut from the above
#
# The sizes are the lists in the tests of crates/app/src/brand.rs, which hold
# the files to them. Do not edit the generated files by hand.

# The accent of the mark on the icon's ink tile. The one colour literal outside
# the theme module: the icon is a raster, drawn before any theme exists.
ICON_ACCENT="#FFEA00"
# 1: draw the 16 and 32 px pictures with the fitted mark; 0: the full mark.
ICON_FITTED_SMALL=1

set -euo pipefail

root="$(cd "$(dirname "$0")/.." && pwd)"
source="$root/crates/app/assets/brand/app-icon.svg"
fitted="$root/crates/app/assets/brand/leon-mark-16.svg"
out="$root/crates/app/assets/icons"
work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT
mkdir -p "$out"

# The source with the accent applied to the mark (its `<path`), and its twin
# with the fitted geometry for the smallest sizes.
python3 - "$source" "$fitted" "$ICON_ACCENT" "$work" <<'PY'
import re, sys
source, fitted, accent, work = sys.argv[1:5]
svg = open(source).read()
tag = re.search(r'<path\b[^>]*>', svg).group(0)
coloured = re.sub(r'fill="[^"]*"', 'fill="' + accent + '"', tag, count=1)
open(work + "/full.svg", "w").write(svg.replace(tag, coloured))
d = re.search(r'\bd="([^"]*)"', open(fitted).read()).group(1)
small = re.sub(r'\bd="[^"]*"', 'd="' + d + '"', coloured, count=1)
open(work + "/small.svg", "w").write(svg.replace(tag, small))
PY
full="$work/full.svg"
small="$full"
if [ "$ICON_FITTED_SMALL" = 1 ]; then small="$work/small.svg"; fi
# The source of a size: the fitted mark up to 32 px, the full one above.
pick() { if [ "$1" -le 32 ]; then echo "$small"; else echo "$full"; fi; }

# The macOS template: Apple's icon grid draws the tile 824 px wide on a
# 1024 px canvas with a corner radius of about 22.4% of the tile, and leaves
# the margin transparent. The source is a square tile (its viewBox side is read
# from it), so it is clipped to that rounded square and inset.
macos() {
  python3 - "$1" "$2" <<'PY'
import re, sys
svg = open(sys.argv[1]).read()
side = float(re.search(r'viewBox="0 0 ([\d.]+) ', svg).group(1))
inner = re.search(r'<svg[^>]*>(.*)</svg>', svg, re.S).group(1)
# The macOS tile is a square the clip rounds: drop the source's own corners.
inner = re.sub(r'(<rect\b[^>]*?) rx="[^"]*"', r'\1', inner)
radius = 0.224 * side
open(sys.argv[2], "w").write(f"""<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 1024 1024">
<defs><clipPath id="tile"><rect width="{side}" height="{side}" rx="{radius}" ry="{radius}"/></clipPath></defs>
<g transform="translate(100 100) scale({824 / side})" clip-path="url(#tile)">{inner}</g>
</svg>
""")
PY
}
macos "$full" "$work/macos.svg"
macos "$small" "$work/macos-small.svg"

for size in 16 32 48 64 128 256 512; do
  rsvg-convert --width "$size" --height "$size" "$(pick "$size")" --output "$out/app-icon-$size.png"
done
for size in 16 32 64 128 256 512 1024; do
  src="$work/macos.svg"
  if [ "$size" -le 32 ]; then src="$work/macos-small.svg"; fi
  rsvg-convert --width "$size" --height "$size" "$src" --output "$out/app-icon-macos-$size.png"
done

# Windows: a PNG-compressed picture per size inside one .ico.
for size in 16 24 32 48 64 128 256; do
  rsvg-convert --width "$size" --height "$size" "$(pick "$size")" --output "$work/ico-$size.png"
done
python3 - "$work" "$out/app-icon.ico" <<'PY'
import struct, sys
work, target = sys.argv[1], sys.argv[2]
sizes = [16, 24, 32, 48, 64, 128, 256]
pictures = [open(f"{work}/ico-{size}.png", "rb").read() for size in sizes]
header = struct.pack("<HHH", 0, 1, len(sizes))
offset = 6 + 16 * len(sizes)
entries = b""
for size, data in zip(sizes, pictures):
    side = 0 if size == 256 else size
    entries += struct.pack("<BBBBHHII", side, side, 0, 0, 1, 32, len(data), offset)
    offset += len(data)
open(target, "wb").write(header + entries + b"".join(pictures))
PY

# macOS: an iconset is each size and its @2x.
if command -v iconutil >/dev/null; then
  set="$work/AppIcon.iconset"
  mkdir -p "$set"
  m="$out/app-icon-macos"
  cp "$m-16.png"   "$set/icon_16x16.png"
  cp "$m-32.png"   "$set/icon_16x16@2x.png"
  cp "$m-32.png"   "$set/icon_32x32.png"
  cp "$m-64.png"   "$set/icon_32x32@2x.png"
  cp "$m-128.png"  "$set/icon_128x128.png"
  cp "$m-256.png"  "$set/icon_128x128@2x.png"
  cp "$m-256.png"  "$set/icon_256x256.png"
  cp "$m-512.png"  "$set/icon_256x256@2x.png"
  cp "$m-512.png"  "$set/icon_512x512.png"
  cp "$m-1024.png" "$set/icon_512x512@2x.png"
  iconutil --convert icns --output "$out/app-icon.icns" "$set"
else
  echo "iconutil not found: app-icon.icns left as it is" >&2
fi
echo "icons written to $out"
