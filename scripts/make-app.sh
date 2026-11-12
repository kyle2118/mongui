#!/bin/sh
# Builds mongui and wraps it in a double-clickable macOS app bundle: ./mongui.app
set -e
cd "$(dirname "$0")/.."
cargo build --release
APP=mongui.app
rm -rf "$APP"
mkdir -p "$APP/Contents/MacOS"
cp target/release/mongui "$APP/Contents/MacOS/mongui"
# App icon: build an .iconset from assets/mongui-icon.png (1024x1024) and convert it to .icns.
mkdir -p "$APP/Contents/Resources"
ICONSET=$(mktemp -d)/mongui.iconset
mkdir -p "$ICONSET"
for s in 16 32 128 256 512; do
  sips -z $s $s assets/mongui-icon.png --out "$ICONSET/icon_${s}x${s}.png" >/dev/null
  sips -z $((s*2)) $((s*2)) assets/mongui-icon.png --out "$ICONSET/icon_${s}x${s}@2x.png" >/dev/null
done
iconutil -c icns "$ICONSET" -o "$APP/Contents/Resources/mongui.icns"
rm -rf "$(dirname "$ICONSET")"
cat > "$APP/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
  <key>CFBundleName</key><string>mongui</string>
  <key>CFBundleIdentifier</key><string>local.mongui</string>
  <key>CFBundleExecutable</key><string>mongui</string>
  <key>CFBundleIconFile</key><string>mongui</string>
  <key>CFBundlePackageType</key><string>APPL</string>
  <key>CFBundleShortVersionString</key><string>0.1.0</string>
  <key>NSHighResolutionCapable</key><true/>
</dict></plist>
PLIST
echo "Built $APP"
