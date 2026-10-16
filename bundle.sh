#!/bin/zsh
# Builds "Music Host.app" and installs it to ~/Applications.
# `./bundle.sh --no-install` only builds dist/Music Host.app (used by CI).
set -e
cd "${0:h}"
[[ -f "$HOME/.cargo/env" ]] && source "$HOME/.cargo/env"
cargo build --release
APP="dist/Music Host.app"
rm -rf "$APP"; mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources"
cp target/release/music-host "$APP/Contents/MacOS/Music Host"

# Icon
ICONSET=$(mktemp -d)/icon.iconset; mkdir -p "$ICONSET"
for s in 16 32 128 256 512; do
  sips -z $s $s assets/icon.png --out "$ICONSET/icon_${s}x${s}.png" >/dev/null
  sips -z $((s*2)) $((s*2)) assets/icon.png --out "$ICONSET/icon_${s}x${s}@2x.png" >/dev/null
done
iconutil -c icns "$ICONSET" -o "$APP/Contents/Resources/AppIcon.icns"

VERSION=$(grep '^version' Cargo.toml | head -1 | cut -d'"' -f2)
cat > "$APP/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
  <key>CFBundleName</key><string>Music Host</string>
  <key>CFBundleDisplayName</key><string>Music Host</string>
  <key>CFBundleIdentifier</key><string>com.musichost.app</string>
  <key>CFBundleExecutable</key><string>Music Host</string>
  <key>CFBundleIconFile</key><string>AppIcon</string>
  <key>CFBundlePackageType</key><string>APPL</string>
  <key>CFBundleShortVersionString</key><string>$VERSION</string>
  <key>CFBundleVersion</key><string>$VERSION</string>
  <key>LSMinimumSystemVersion</key><string>11.0</string>
  <key>LSApplicationCategoryType</key><string>public.app-category.music</string>
  <key>NSHighResolutionCapable</key><true/>
</dict></plist>
PLIST
codesign --force --deep -s - "$APP" 2>/dev/null || true
[[ "$1" == "--no-install" ]] && { echo "Built: $APP"; exit 0; }

mkdir -p "$HOME/Applications"
rm -rf "$HOME/Applications/Music Host.app"
cp -R "$APP" "$HOME/Applications/"
echo "Installed: ~/Applications/Music Host.app"
