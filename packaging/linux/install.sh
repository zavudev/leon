#!/bin/sh
# Install Leon for one Linux user, without root:
#
#   curl -fsSL https://github.com/zavudev/leon/releases/latest/download/install.sh | sh
#   sh install.sh --version 0.1.0
#   sh install.sh --uninstall
#
# Environment:
#   LEON_VERSION   the same as --version
#   LEON_BASE_URL  where the release files live
#   LEON_BIN_DIR   where the `leon` command goes (default: ~/.local/bin)

set -eu

REPO="zavudev/leon"
DEFAULT_BASE_URL="https://github.com/$REPO/releases/latest/download"
PINNED_BASE_URL="https://github.com/$REPO/releases/download"

say() { printf '%s\n' "$*"; }
warn() { printf 'install.sh: %s\n' "$*" >&2; }
die() { warn "$*"; exit 1; }

usage() {
  cat <<'USAGE'
Leon on Linux: install it under your home, or uninstall it.

  sh install.sh [--version <v>] [--base-url <url>] [--bin-dir <dir>]
  sh install.sh --uninstall

  --version <v>      install v<v> instead of the latest release
  --base-url <url>   where the release files live (default: GitHub Releases)
  --bin-dir <dir>    where the `leon` command goes (default: ~/.local/bin)
  --uninstall        remove the application files installed by this script
  -h, --help         show this text

Leon projects and settings are not removed by --uninstall.
USAGE
}

version="${LEON_VERSION:-}"
base_url="${LEON_BASE_URL:-}"
bin_dir="${LEON_BIN_DIR:-$HOME/.local/bin}"
uninstall=false

while [ $# -gt 0 ]; do
  case "$1" in
    --version) [ $# -ge 2 ] || die "--version needs a value"; version="$2"; shift 2 ;;
    --base-url) [ $# -ge 2 ] || die "--base-url needs a value"; base_url="$2"; shift 2 ;;
    --bin-dir) [ $# -ge 2 ] || die "--bin-dir needs a value"; bin_dir="$2"; shift 2 ;;
    --uninstall) uninstall=true; shift ;;
    -h|--help) usage; exit 0 ;;
    *) die "unknown option: $1 (--help lists them)" ;;
  esac
done

data_home="${XDG_DATA_HOME:-$HOME/.local/share}"
binary="$bin_dir/leon"
desktop_file="$data_home/applications/dev.zavu.leon.desktop"
doc_dir="$data_home/doc/leon"

icon_file() {
  printf '%s/icons/hicolor/%sx%s/apps/dev.zavu.leon.png' "$data_home" "$1" "$1"
}

if [ "$uninstall" = "true" ]; then
  found=false
  for file in "$binary" "$desktop_file" "$doc_dir/LICENSE" "$doc_dir/NOTICE"; do
    if [ -e "$file" ]; then
      rm -f "$file"
      say "removed $file"
      found=true
    fi
  done
  for size in 16 32 48 64 128 256 512; do
    file="$(icon_file "$size")"
    if [ -e "$file" ]; then
      rm -f "$file"
      say "removed $file"
      found=true
    fi
  done
  rmdir "$doc_dir" 2>/dev/null || true
  if [ "$found" = "false" ]; then
    say "nothing to remove"
  fi
  if command -v update-desktop-database >/dev/null 2>&1; then
    update-desktop-database "$data_home/applications" >/dev/null 2>&1 || true
  fi
  if command -v gtk-update-icon-cache >/dev/null 2>&1; then
    gtk-update-icon-cache -f -t "$data_home/icons/hicolor" >/dev/null 2>&1 || true
  fi
  say "your projects and settings were not removed"
  exit 0
fi

[ "$(uname -s)" = "Linux" ] || die "this installer is for Linux"
arch="$(uname -m)"
[ "$arch" = "x86_64" ] || die "there is no build for $arch yet (x86_64 only); see https://github.com/$REPO/releases"
if [ -e /lib/ld-musl-x86_64.so.1 ] || [ -e /lib/ld-musl-aarch64.so.1 ]; then
  die "this looks like a musl system (Alpine?): Leon needs glibc"
fi
libc="$(getconf GNU_LIBC_VERSION 2>/dev/null | awk '{print $2}' || true)"
if [ -n "$libc" ] && ! awk -v have="$libc" 'BEGIN {
  split(have, parts, ".")
  exit !(parts[1] + 0 > 2 || (parts[1] + 0 == 2 && parts[2] + 0 >= 35))
}'; then
  die "glibc $libc is older than 2.35 and the build will not start here (Ubuntu 22.04 or a similarly recent distribution is needed)"
fi

tmp="$(mktemp -d "${TMPDIR:-/tmp}/leon.XXXXXX")"
trap 'rm -rf "$tmp"' EXIT INT TERM

download() { # <url> <file>
  if command -v curl >/dev/null 2>&1; then
    curl -fsSL "$1" -o "$2"
  elif command -v wget >/dev/null 2>&1; then
    wget -qO "$2" "$1"
  else
    die "curl or wget is needed"
  fi
}

if [ -n "$version" ]; then
  base="${base_url:-$PINNED_BASE_URL/v$version}"
else
  base="${base_url:-$DEFAULT_BASE_URL}"
fi

download "$base/SHA256SUMS" "$tmp/SHA256SUMS" || die "could not download $base/SHA256SUMS"

if [ -n "$version" ]; then
  archive="leon-$version-linux-x86_64.tar.gz"
else
  archives="$(awk '$2 ~ /^leon-[^/]+-linux-x86_64\.tar\.gz$/ { print $2 }' "$tmp/SHA256SUMS")"
  archive="$(printf '%s\n' "$archives" | sed -n '1p')"
  [ -n "$archive" ] || die "SHA256SUMS names no Linux x86_64 archive"
  [ "$(printf '%s\n' "$archives" | sed -n '2p')" = "" ] || die "SHA256SUMS names more than one Linux x86_64 archive"
fi

expected="$(awk -v file="$archive" '$2 == file { print $1 }' "$tmp/SHA256SUMS" | sed -n '1p')"
[ -n "$expected" ] || die "SHA256SUMS has no checksum for $archive"
case "$expected" in
  *[!0-9a-fA-F]*|'') die "SHA256SUMS has an invalid checksum for $archive" ;;
esac
[ "${#expected}" -eq 64 ] || die "SHA256SUMS has an invalid checksum for $archive"

say "downloading $archive"
download "$base/$archive" "$tmp/$archive" || die "could not download $base/$archive"

if command -v sha256sum >/dev/null 2>&1; then
  actual="$(sha256sum "$tmp/$archive" | awk '{print $1}')"
elif command -v shasum >/dev/null 2>&1; then
  actual="$(shasum -a 256 "$tmp/$archive" | awk '{print $1}')"
elif command -v openssl >/dev/null 2>&1; then
  actual="$(openssl dgst -sha256 "$tmp/$archive" | awk '{print $NF}')"
else
  die "no sha256sum, shasum or openssl is available to verify the download"
fi
[ "$(printf '%s' "$actual" | tr 'A-F' 'a-f')" = "$(printf '%s' "$expected" | tr 'A-F' 'a-f')" ] ||
  die "the SHA-256 does not match SHA256SUMS: refusing to install the download"

mkdir "$tmp/unpack"
tar -xzf "$tmp/$archive" -C "$tmp/unpack" || die "could not unpack $archive"
release_dir="$tmp/unpack/${archive%.tar.gz}"
source_bin="$release_dir/leon"
source_desktop="$release_dir/share/applications/dev.zavu.leon.desktop"
[ -f "$source_bin" ] || die "the archive has no leon binary"
[ -f "$source_desktop" ] || die "the archive has no desktop entry"
[ -f "$release_dir/LICENSE" ] || die "the archive has no LICENSE"
[ -f "$release_dir/NOTICE" ] || die "the archive has no NOTICE"
for size in 16 32 48 64 128 256 512; do
  [ -f "$release_dir/share/icons/hicolor/${size}x${size}/apps/dev.zavu.leon.png" ] ||
    die "the archive has no ${size}x${size} icon"
done

missing="$(ldd "$source_bin" 2>/dev/null | awk '/not found/ {print $1}' | sort -u | tr '\n' ' ' || true)"
if [ -n "$missing" ]; then
  warn "missing libraries: $missing"
  warn "install the matching Wayland/X11, XKB and fontconfig runtime packages for your distribution"
fi

mkdir -p "$bin_dir" "$(dirname "$desktop_file")" "$doc_dir"
cp "$source_bin" "$binary"
chmod 755 "$binary"
cp "$release_dir/LICENSE" "$doc_dir/LICENSE"
cp "$release_dir/NOTICE" "$doc_dir/NOTICE"
for size in 16 32 48 64 128 256 512; do
  target_icon="$(icon_file "$size")"
  mkdir -p "$(dirname "$target_icon")"
  cp "$release_dir/share/icons/hicolor/${size}x${size}/apps/dev.zavu.leon.png" "$target_icon"
done
# GUI sessions do not consistently inherit ~/.local/bin in PATH.
awk -v binary="$binary" '/^Exec=/ { print "Exec=\"" binary "\""; next } { print }' \
  "$source_desktop" >"$desktop_file"

if command -v update-desktop-database >/dev/null 2>&1; then
  update-desktop-database "$(dirname "$desktop_file")" >/dev/null 2>&1 || true
fi
if command -v gtk-update-icon-cache >/dev/null 2>&1; then
  gtk-update-icon-cache -f -t "$data_home/icons/hicolor" >/dev/null 2>&1 || true
fi

say "Leon is installed:"
say "  $binary"
say "open it from your applications menu, or run it from a terminal"
case ":${PATH}:" in
  *":$bin_dir:"*) ;;
  *)
    say ""
    say "$bin_dir is not in your PATH; to run Leon from a terminal:"
    say "  export PATH=\"$bin_dir:\$PATH\""
    ;;
esac
say ""
say "Run this installer again to upgrade Leon."
