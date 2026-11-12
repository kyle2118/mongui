#!/bin/sh
# Builds mongui and wraps it in a double-clickable macOS app bundle: ./mongui.app
set -e
cd "$(dirname "$0")/.."
cargo build --release
APP=mongui.app
rm -rf "$APP"
mkdir -p "$APP/Contents/MacOS"
cp target/release/mongui "$APP/Contents/MacOS/mongui"
cat > "$APP/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
  <key>CFBundleName</key><string>mongui</string>
  <key>CFBundleIdentifier</key><string>local.mongui</string>
  <key>CFBundleExecutable</key><string>mongui</string>
  <key>CFBundlePackageType</key><string>APPL</string>
  <key>CFBundleShortVersionString</key><string>0.1.0</string>
  <key>NSHighResolutionCapable</key><true/>
</dict></plist>
PLIST
echo "Built $APP"
