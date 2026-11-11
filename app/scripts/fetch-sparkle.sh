#!/bin/bash
set -euo pipefail
# Keep the framework and release tools on the same, checksum-pinned version.
version=2.10.1
sha256=1a856d2a9edb8e628de88121d7be04cf0e2167f8641650b78fca00b14244cf27
out=${1:?usage: fetch-sparkle.sh OUTPUT_DIRECTORY}
mkdir -p "$out"
archive=$(mktemp)
trap 'rm -f "$archive"' EXIT
curl --fail --location --retry 3 "https://github.com/sparkle-project/Sparkle/releases/download/$version/Sparkle-$version.tar.xz" -o "$archive"
printf '%s  %s\n' "$sha256" "$archive" | shasum -a 256 --check
# ditto is used later when embedding the framework to preserve its symlinks.
tar -xJf "$archive" -C "$out"
