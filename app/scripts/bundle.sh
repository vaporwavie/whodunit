#!/bin/zsh
# Builds Whodunit and installs it: ~/Applications/Whodunit.app on macOS, ~/.local (binary, icons, desktop entry) on Linux.
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
  cp target/release/whodunit "$app/Contents/MacOS/whodunit"
  icns assets/icons/app.png "$app/Contents/Resources/AppIcon.icns"
  cat > "$app/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
  <key>CFBundleName</key><string>$name</string>
  <key>CFBundleIdentifier</key><string>$id</string>
  <key>CFBundleExecutable</key><string>whodunit</string>
  <key>CFBundleIconFile</key><string>AppIcon</string>
  <key>CFBundlePackageType</key><string>APPL</string>
  <key>CFBundleShortVersionString</key><string>0.1.0</string>
  <key>NSHighResolutionCapable</key><true/>
</dict></plist>
PLIST
  codesign --force --sign - "$app" >/dev/null 2>&1 || true
  echo "$app"
}

install_linux() {
  local name=$1
  local share="${XDG_DATA_HOME:-$HOME/.local/share}"
  local bin="$HOME/.local/bin/whodunit"
  local desktop="$share/applications/whodunit.desktop"
  install -Dm755 target/release/whodunit "$bin"
  for size in 32 48 64 128 256 512; do
    local icon="$share/icons/hicolor/${size}x${size}/apps/whodunit.png"
    mkdir -p "${icon:h}"
    magick assets/icons/app.png -resize ${size}x${size} "$icon"
  done
  mkdir -p "${desktop:h}"
  cat > "$desktop" <<DESKTOP
[Desktop Entry]
Type=Application
Name=$name
Comment=Blind trials for coding agents in pi
Exec=$bin
Icon=whodunit
Terminal=false
Categories=Development;
StartupWMClass=whodunit
DESKTOP
  if (( $+commands[update-desktop-database] )); then
    update-desktop-database -q "${desktop:h}" || true
  fi
  echo "$bin"
  echo "$desktop"
}

case $OSTYPE in
  darwin*) bundle "Whodunit" io.github.vaporwavie.whodunit ;;
  linux*) install_linux "Whodunit" ;;
  *) echo "bundle.sh: unsupported OS $OSTYPE" >&2; exit 1 ;;
esac
