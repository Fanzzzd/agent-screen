#!/usr/bin/env bash
# Build Sidescreen.app from the Rust binary.
#
# Usage: scripts/bundle.sh [debug|release]
#
# Signing: macOS ties Screen Recording and Accessibility grants to the code
# signature and bundle id. This bundle uses the Swift app's Info.plist and
# the first Apple Development identity, so both builds share the grants.
# Override with SIDESCREEN_SIGN_IDENTITY (use "-" for ad-hoc).
set -euo pipefail

cd "$(dirname "$0")/.."
config="${1:-debug}"
flags=()
[[ "$config" == release ]] && flags=(--release)
cargo build -p sidescreen ${flags[@]+"${flags[@]}"}

target="$(cd .. && pwd)/target"
app="$target/Sidescreen.app"
rm -rf "$app"
mkdir -p "$app/Contents/MacOS" "$app/Contents/Resources"
cp "$target/$config/Sidescreen" "$app/Contents/MacOS/Sidescreen"
cp ../../apps/menubar/Resources/Info.plist "$app/Contents/Info.plist"
cp ../../apps/menubar/Resources/AppIcon.icns "$app/Contents/Resources/AppIcon.icns"

identity="${SIDESCREEN_SIGN_IDENTITY:-}"
if [[ -z "$identity" ]]; then
  identity="$(security find-identity -v -p codesigning | awk -F'"' '/Apple Development/ { print $2; exit }')"
fi
codesign --force --sign "${identity:--}" "$app" >/dev/null
echo "$app (signed: ${identity:-ad-hoc})"
