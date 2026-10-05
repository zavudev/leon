#!/usr/bin/env bash
# Builds the macOS application bundle around a release binary.
#
#   cargo build --release -p leon
#   packaging/macos/bundle.sh target/release/leon 0.1.0 dist
#
# Writes "<output directory>/Leon.app". The bundle is not signed or
# notarised: a Mac that has quarantined it asks for "Open Anyway" once.
#
# The executable inside is "Contents/MacOS/Leon": macOS names the Dock tile,
# Activity Monitor and the application menu after the bundle and its
# executable, so the product's name must be the file's name too. The cargo
# binary and the command line stay `leon`.
#
# LEON_DEVELOPMENT_BUILD=1 marks the plist (key LeonDevelopmentBuild) as the
# bundle that scripts/cargo-runner-macos.sh keeps under target/.
set -euo pipefail

binary="$1"
version="$2"
out="$3"
here="$(cd "$(dirname "$0")" && pwd)"
root="$(cd "$here/../.." && pwd)"
icons="$root/crates/app/assets/icons"

app="$out/Leon.app"
rm -rf "$app"
mkdir -p "$app/Contents/MacOS" "$app/Contents/Resources"
cp "$binary" "$app/Contents/MacOS/Leon"
chmod 755 "$app/Contents/MacOS/Leon"

# The icon: regenerate it with scripts/generate-icons.sh.
cp "$icons/app-icon.icns" "$app/Contents/Resources/AppIcon.icns"

development=""
if [ "${LEON_DEVELOPMENT_BUILD:-}" = "1" ]; then
  development="    <key>LeonDevelopmentBuild</key>
    <true/>"
fi

cat > "$app/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>CFBundleName</key>
    <string>Leon</string>
    <key>CFBundleDisplayName</key>
    <string>Leon</string>
    <key>CFBundleIdentifier</key>
    <string>dev.zavu.leon</string>
    <key>CFBundleExecutable</key>
    <string>Leon</string>
    <key>CFBundleIconFile</key>
    <string>AppIcon</string>
    <key>CFBundlePackageType</key>
    <string>APPL</string>
    <key>CFBundleInfoDictionaryVersion</key>
    <string>6.0</string>
    <key>CFBundleShortVersionString</key>
    <string>${version}</string>
    <key>CFBundleVersion</key>
    <string>${version}</string>
    <key>LSMinimumSystemVersion</key>
    <string>11.0</string>
    <key>LSApplicationCategoryType</key>
    <string>public.app-category.developer-tools</string>
    <key>NSHighResolutionCapable</key>
    <true/>
${development}
</dict>
</plist>
PLIST

plutil -lint "$app/Contents/Info.plist" >/dev/null
echo "$app"
