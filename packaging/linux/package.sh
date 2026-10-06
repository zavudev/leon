#!/bin/sh
# Build Leon's .deb and .rpm from the Linux release archive with nFPM:
#
#   packaging/linux/package.sh [--version <v>] [--dist <dir>] [--out <dir>]
#
# Install nFPM with:
#   go install github.com/goreleaser/nfpm/v2/cmd/nfpm@v2.47.0

set -eu

root="$(CDPATH='' cd -- "$(dirname -- "$0")/../.." && pwd)"
version=""
dist="$root/dist"
out="$root/dist"

while [ $# -gt 0 ]; do
  case "$1" in
    --version) [ $# -ge 2 ] || { echo "--version needs a value" >&2; exit 2; }; version="$2"; shift 2 ;;
    --dist) [ $# -ge 2 ] || { echo "--dist needs a value" >&2; exit 2; }; dist="$2"; shift 2 ;;
    --out) [ $# -ge 2 ] || { echo "--out needs a value" >&2; exit 2; }; out="$2"; shift 2 ;;
    -h|--help) sed -n '2,9p' "$0"; exit 0 ;;
    *) echo "unknown option: $1 (--help lists them)" >&2; exit 2 ;;
  esac
done

if [ -z "$version" ]; then
  version="$(sed -n '/^\[workspace.package\]/,/^\[/p' "$root/Cargo.toml" | sed -n 's/^version[[:space:]]*=[[:space:]]*"\([^"]*\)".*/\1/p' | sed -n '1p')"
fi
[ -n "$version" ] || { echo "no version in $root/Cargo.toml; pass --version" >&2; exit 2; }

if command -v nfpm >/dev/null 2>&1; then
  nfpm="nfpm"
elif command -v go >/dev/null 2>&1 && [ -x "$(go env GOPATH 2>/dev/null)/bin/nfpm" ]; then
  nfpm="$(go env GOPATH)/bin/nfpm"
else
  echo "nfpm is needed in PATH: go install github.com/goreleaser/nfpm/v2/cmd/nfpm@v2.47.0" >&2
  exit 2
fi

archive="$dist/leon-$version-linux-x86_64.tar.gz"
[ -f "$archive" ] || { echo "$archive is missing (build the Linux release archive first)" >&2; exit 2; }

work="$(mktemp -d "${TMPDIR:-/tmp}/leon-packages.XXXXXX")"
trap 'rm -rf "$work"' EXIT INT TERM
tar -xzf "$archive" -C "$work"
linux_dir="$work/leon-$version-linux-x86_64"
for file in \
  leon share/applications/dev.zavu.leon.desktop LICENSE NOTICE \
  share/icons/hicolor/16x16/apps/dev.zavu.leon.png \
  share/icons/hicolor/32x32/apps/dev.zavu.leon.png \
  share/icons/hicolor/48x48/apps/dev.zavu.leon.png \
  share/icons/hicolor/64x64/apps/dev.zavu.leon.png \
  share/icons/hicolor/128x128/apps/dev.zavu.leon.png \
  share/icons/hicolor/256x256/apps/dev.zavu.leon.png \
  share/icons/hicolor/512x512/apps/dev.zavu.leon.png; do
  [ -f "$linux_dir/$file" ] || { echo "$archive has no $file" >&2; exit 2; }
done

sed -e "s|\${VERSION}|$version|g" -e "s|\${LINUX_DIR}|$linux_dir|g" \
  "$root/packaging/linux/nfpm.yaml" >"$work/nfpm.yaml"

mkdir -p "$out"
for packager in deb rpm; do
  "$nfpm" package --config "$work/nfpm.yaml" --packager "$packager" --target "$out/"
done

package_version="$(printf '%s' "$version" | sed 's/-/~/')"
deb="$out/leon_${package_version}_amd64.deb"
[ -f "$deb" ] || { echo "nFPM did not create $deb" >&2; exit 2; }
set -- "$out"/leon-"$package_version"-*.x86_64.rpm
[ -f "$1" ] || { echo "nFPM did not create the x86_64 RPM" >&2; exit 2; }
printf '%s\n' "$deb" "$1"
