#!/usr/bin/env bash
# Builds Leon, wraps it in Leon.app and opens it: the way to run it on macOS
# during development with the product's name everywhere (the application menu,
# the Dock, the application switcher come from the bundle's Info.plist; an
# unbundled executable is shown under its file name). `cargo run -p leon` does
# the same on its own through scripts/cargo-runner-macos.sh, with a development
# bundle under target/; this script is for the packaged layout in dist/.
#
#   scripts/run-macos.sh [--release]
set -euo pipefail

root="$(cd "$(dirname "$0")/.." && pwd)"
profile=debug
flags=()
if [ "${1:-}" = "--release" ]; then
  profile=release
  flags=(--release)
fi
version="$(cargo metadata --no-deps --format-version 1 --manifest-path "$root/Cargo.toml" \
  | python3 -c 'import json,sys; print([p["version"] for p in json.load(sys.stdin)["packages"] if p["name"]=="leon"][0])')"

cargo build -p leon "${flags[@]}" --manifest-path "$root/Cargo.toml"
"$root/packaging/macos/bundle.sh" "$root/target/$profile/leon" "$version" "$root/dist"
open "$root/dist/Leon.app"
