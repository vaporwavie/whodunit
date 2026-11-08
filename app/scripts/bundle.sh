#!/bin/zsh
# Builds Agent Week and installs it in ~/Applications: a chat window and a menu bar icon while it runs.
set -euo pipefail
cd "${0:A:h}/.."
cargo build --release
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT

icns() {
  local png=$1 out=$2
  local set="$work/$(basename "$png" .png).iconset"
  mkdir -p "$set"
  for size in 16 32 128 256 512; do
    sips -z $size $size "$png" --out "$set/icon_${size}x${size}.png" >/dev/null
    sips -z $((size * 2)) $((size * 2)) "$png" --out "$set/icon_${size}x${size}@2x.png" >/dev/null
  done
  iconutil -c icns "$set" -o "$out"
}

bundle() {
  local name=$1 id=$2
  local app="$HOME/Applications/$name.app"
  rm -rf "$app"
  mkdir -p "$app/Contents/MacOS" "$app/Contents/Resources"
  cp target/release/agent-week "$app/Contents/MacOS/agent-week"
  icns assets/icons/full.png "$app/Contents/Resources/AppIcon.icns"
  cat > "$app/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
  <key>CFBundleName</key><string>$name</string>
  <key>CFBundleIdentifier</key><string>$id</string>
  <key>CFBundleExecutable</key><string>agent-week</string>
  <key>CFBundleIconFile</key><string>AppIcon</string>
  <key>CFBundlePackageType</key><string>APPL</string>
  <key>CFBundleShortVersionString</key><string>0.1.0</string>
  <key>NSHighResolutionCapable</key><true/>
</dict></plist>
PLIST
  codesign --force --sign - "$app" >/dev/null 2>&1 || true
  echo "$app"
}

bundle "Agent Week" technology.erfolg.agent-week
