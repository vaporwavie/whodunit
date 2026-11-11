#!/bin/zsh
# Builds Whodunit and installs it: ~/Applications/Whodunit.app on macOS, ~/.local (binary, icons, desktop entry) on Linux.
set -euo pipefail
cd "${0:A:h}/.."
cargo build --release
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT

bundle() {
  local app="$HOME/Applications/Whodunit.app"
  # Source installs stay off the public update channel.
  python3 scripts/release.py package --binary target/release/whodunit --output "$work/Whodunit.app"
  mkdir -p "${app:h}"
  rm -rf "$app"
  mv "$work/Whodunit.app" "$app"
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
  darwin*) bundle ;;
  linux*) install_linux "Whodunit" ;;
  *) echo "bundle.sh: unsupported OS $OSTYPE" >&2; exit 1 ;;
esac
