#!/bin/sh
# Fast, network-free contract test for the Linux installer.

set -eu

root="$(CDPATH='' cd -- "$(dirname -- "$0")/../.." && pwd)"
installer="$root/packaging/linux/install.sh"
packager="$root/packaging/linux/package.sh"
version="9.8.7"
tmp="$(mktemp -d "${TMPDIR:-/tmp}/leon-linux-test.XXXXXX")"
trap 'rm -rf "$tmp"' EXIT INT TERM

sh -n "$installer"
sh -n "$packager"

release="$tmp/release"
stage="$tmp/stage/leon-$version-linux-x86_64"
mkdir -p "$release" "$stage/share/applications"
cp /bin/true "$stage/leon"
cp "$root/crates/app/assets/linux/dev.zavu.leon.desktop" "$stage/share/applications/"
cp "$root/LICENSE" "$root/NOTICE" "$stage/"
for size in 16 32 48 64 128 256 512; do
  icon_dir="$stage/share/icons/hicolor/${size}x${size}/apps"
  mkdir -p "$icon_dir"
  cp "$root/crates/app/assets/icons/app-icon-$size.png" "$icon_dir/dev.zavu.leon.png"
done
tar -czf "$release/leon-$version-linux-x86_64.tar.gz" -C "$tmp/stage" "leon-$version-linux-x86_64"
(
  cd "$release"
  sha256sum "leon-$version-linux-x86_64.tar.gz" >SHA256SUMS
)

home="$tmp/home"
data="$tmp/data"
mkdir -p "$home" "$data/leon"
: >"$data/leon/keep"
HOME="$home" XDG_DATA_HOME="$data" LEON_BIN_DIR="$home/bin" \
  sh "$installer" --base-url "file://$release"

test -x "$home/bin/leon"
test "$(stat -c %a "$home/bin/leon")" = 755
grep -Fqx "Exec=\"$home/bin/leon\"" "$data/applications/dev.zavu.leon.desktop"
test -f "$data/doc/leon/LICENSE"
test -f "$data/doc/leon/NOTICE"
for size in 16 32 48 64 128 256 512; do
  test -f "$data/icons/hicolor/${size}x${size}/apps/dev.zavu.leon.png"
done

HOME="$home" XDG_DATA_HOME="$data" LEON_BIN_DIR="$home/bin" sh "$installer" --uninstall
test ! -e "$home/bin/leon"
test ! -e "$data/applications/dev.zavu.leon.desktop"
test -f "$data/leon/keep"

printf 'not the release\n' >>"$release/leon-$version-linux-x86_64.tar.gz"
bad_home="$tmp/bad-home"
mkdir -p "$bad_home"
if HOME="$bad_home" LEON_BIN_DIR="$bad_home/bin" \
  sh "$installer" --version "$version" --base-url "file://$release" >"$tmp/bad.out" 2>&1; then
  echo "installer accepted an archive whose checksum does not match" >&2
  exit 1
fi
grep -Fq "SHA-256 does not match" "$tmp/bad.out"
test ! -e "$bad_home/bin/leon"

fake_bin="$tmp/fake-bin"
mkdir "$fake_bin"
cat >"$fake_bin/getconf" <<'EOF'
#!/bin/sh
echo 'glibc 2.34'
EOF
chmod 755 "$fake_bin/getconf"
if HOME="$bad_home" PATH="$fake_bin:$PATH" \
  sh "$installer" --base-url "file://$release" >"$tmp/glibc.out" 2>&1; then
  echo "installer accepted an unsupported glibc" >&2
  exit 1
fi
grep -Fq "glibc 2.34 is older than 2.35" "$tmp/glibc.out"

echo "Linux installer tests passed"
