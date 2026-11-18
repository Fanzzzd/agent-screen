#!/usr/bin/env bash
# Fetch Sparkle, the update framework release builds embed (ADR 0012), into
# target/, checked against a pinned hash. Prints the folder holding
# Sparkle.framework and bin/ (sign_update, generate_appcast).
#
# Usage: scripts/sparkle.sh
set -euo pipefail

version=2.10.1
sha256=1a856d2a9edb8e628de88121d7be04cf0e2167f8641650b78fca00b14244cf27

cd "$(dirname "$0")/.."
dir="$(cd ../.. && pwd)/target/sparkle-$version"
if [[ ! -d "$dir/Sparkle.framework" ]]; then
  archive="$(mktemp -d)/Sparkle-$version.tar.xz"
  curl -fsSL -o "$archive" "https://github.com/sparkle-project/Sparkle/releases/download/$version/Sparkle-$version.tar.xz"
  got="$(shasum -a 256 "$archive" | cut -d' ' -f1)"
  if [[ "$got" != "$sha256" ]]; then
    echo "error: Sparkle-$version.tar.xz is $got, expected $sha256" >&2
    exit 1
  fi
  rm -rf "$dir"
  mkdir -p "$dir"
  tar -xf "$archive" -C "$dir"
  rm -f "$archive"
fi
echo "$dir"
