#!/bin/sh
# The cargo runner of the macOS targets (.cargo/config.toml): `cargo run -p
# leon` starts Leon from a development bundle, target/<profile>/Leon.app, so
# macOS names it "Leon" with its icon in the Dock, Activity Monitor and the
# application menu. It receives the executable and its arguments.
#
# Everything that is not the application binary (test binaries, examples,
# other crates' tools) is `exec`ed as it is, at once. So are the runs that
# open no window: --help, --version and --diagnose print to the terminal and
# need no bundle. The bundled run is an `exec` too: standard streams, exit
# status, signals (Ctrl+C) and arguments are those of the binary itself.
set -eu

exe="$1"

# Not the application: straight through.
case "$exe" in
  */leon | leon) ;;
  *) exec "$@" ;;
esac
dir="$(dirname "$exe")"
case "$(basename "$dir")" in
  deps | examples | build | .fingerprint) exec "$@" ;;
esac
# Runs without a window.
for arg in "$@"; do
  case "$arg" in
    -h | --help | -V | --version | --diagnose) exec "$@" ;;
  esac
done

here="$(cd "$(dirname "$0")" && pwd)"
root="$(cd "$here/.." && pwd)"
app="$dir/Leon.app"
inner="$app/Contents/MacOS/Leon"

# The bundle is built whole when it is missing or older than the script that
# writes it; otherwise only the executable is refreshed, and only when the
# built binary is newer.
if [ ! -f "$app/Contents/Info.plist" ] || [ ! -f "$inner" ] \
  || [ "$root/packaging/macos/bundle.sh" -nt "$app/Contents/Info.plist" ]; then
  version="$(awk -F'"' '/^version *=/ { print $2; exit }' "$root/Cargo.toml")"
  LEON_DEVELOPMENT_BUILD=1 "$root/packaging/macos/bundle.sh" "$exe" "$version" "$dir" >/dev/null
elif [ "$exe" -nt "$inner" ]; then
  rm -f "$inner"
  ln "$exe" "$inner" 2>/dev/null || cp "$exe" "$inner"
fi

shift
exec "$inner" "$@"
