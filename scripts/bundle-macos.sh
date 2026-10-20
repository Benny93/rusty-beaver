#!/bin/sh
# Build target/release/RustyBeaver.app (icon from assets/rusty-beaver.icns, see scripts/pixel_icon.py).
set -eu
cd "$(dirname "$0")/.."
cargo build --release
APP=target/release/RustyBeaver.app
rm -rf "$APP"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources"
cp assets/rusty-beaver.icns "$APP/Contents/Resources/rusty-beaver.icns"
cp target/release/rusty-beaver "$APP/Contents/MacOS/rusty-beaver"
cat > "$APP/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
  <key>CFBundleName</key><string>Rusty Beaver</string>
  <key>CFBundleDisplayName</key><string>Rusty Beaver</string>
  <key>CFBundleIdentifier</key><string>dev.rustybeaver.app</string>
  <key>CFBundleExecutable</key><string>rusty-beaver</string>
  <key>CFBundleIconFile</key><string>rusty-beaver</string>
  <key>CFBundlePackageType</key><string>APPL</string>
  <key>CFBundleShortVersionString</key><string>$(grep '^version' Cargo.toml | cut -d'"' -f2)</string>
  <key>LSMinimumSystemVersion</key><string>11.0</string>
  <key>NSHighResolutionCapable</key><true/>
</dict></plist>
PLIST
echo "built $APP"
