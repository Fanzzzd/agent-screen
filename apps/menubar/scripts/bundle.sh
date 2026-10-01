#!/usr/bin/env bash
# Build Sidescreen.app from the SwiftPM executable.
#
# Usage: scripts/bundle.sh [debug|release]
#
# Signing: macOS ties Screen Recording and Accessibility grants to the code
# signature. An ad-hoc signature changes on every build and loses the grants,
# so we sign with the first Apple Development identity when one exists.
# Override with SIDESCREEN_SIGN_IDENTITY (use "-" for ad-hoc).
set -euo pipefail

cd "$(dirname "$0")/.."
config="${1:-debug}"

swift build -c "$config"
bin="$(swift build -c "$config" --show-bin-path)"

app=".build/Sidescreen.app"
rm -rf "$app"
mkdir -p "$app/Contents/MacOS" "$app/Contents/Resources"
cp "$bin/Sidescreen" "$app/Contents/MacOS/Sidescreen"
cp Resources/Info.plist "$app/Contents/Info.plist"
cp Resources/AppIcon.icns "$app/Contents/Resources/AppIcon.icns"

identity="${SIDESCREEN_SIGN_IDENTITY:-}"
if [[ -z "$identity" ]]; then
  identity="$(security find-identity -v -p codesigning | awk -F'"' '/Apple Development/ { print $2; exit }')"
fi
codesign --force --sign "${identity:--}" "$app" >/dev/null
echo "$PWD/$app (signed: ${identity:-ad-hoc})"
