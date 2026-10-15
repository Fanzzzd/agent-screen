#!/usr/bin/env bash
# Build Sidescreen.app from the Rust binary.
#
# Usage: scripts/bundle.sh [debug|release]
#
# Signing: macOS ties Screen Recording and Accessibility grants to the code
# signature and bundle id. This bundle signs with the first Apple
# Development identity, else Developer ID Application, so
# builds keep the grants.
# Override with SIDESCREEN_SIGN_IDENTITY (use "-" for ad-hoc).
set -euo pipefail

cd "$(dirname "$0")/.."
config="${1:-debug}"
flags=()
[[ "$config" == release ]] && flags=(--release)
cargo build -p sidescreen -p sidescreen-colorsync ${flags[@]+"${flags[@]}"}

target="$(cd .. && pwd)/target"
# Development builds are a separate app, "Sidescreen Dev", so they never
# take the installed app's privacy grants, socket or agent registration.
if [[ "$config" == release ]]; then
  app="$target/Sidescreen.app"
else
  app="$target/Sidescreen Dev.app"
fi
rm -rf "$app"
mkdir -p "$app/Contents/MacOS" "$app/Contents/Resources"
cp "$target/$config/Sidescreen" "$app/Contents/MacOS/Sidescreen"
# The ColorSync helper, copied to a root-owned place when the user sets it up (ADR 0009).
cp "$target/$config/sidescreen-colorsync" "$app/Contents/MacOS/sidescreen-colorsync"
cp resources/Info.plist "$app/Contents/Info.plist"
if [[ "$config" != release ]]; then
  plutil -replace CFBundleIdentifier -string dev.sidescreen.menubar.dev "$app/Contents/Info.plist"
  plutil -replace CFBundleName -string "Sidescreen Dev" "$app/Contents/Info.plist"
  plutil -replace CFBundleDisplayName -string "Sidescreen Dev" "$app/Contents/Info.plist"
fi
cp resources/AppIcon.icns "$app/Contents/Resources/AppIcon.icns"
cp resources/AgentDisplay.png "$app/Contents/Resources/AgentDisplay.png"

identity="${SIDESCREEN_SIGN_IDENTITY:-}"
if [[ -z "$identity" ]]; then
  identities="$(security find-identity -v -p codesigning)"
  for kind in "Apple Development" "Developer ID Application"; do
    identity="$(awk -F'"' -v kind="$kind" 'index($2, kind) == 1 { print $2; exit }' <<<"$identities")"
    [[ -n "$identity" ]] && break
  done
fi
if [[ -z "$identity" ]]; then
  # An ad-hoc signature changes with every build, and macOS drops the
  # grants of a bundle whose signature it no longer recognizes.
  echo "warning: no signing identity; ad-hoc signing, so privacy grants won't carry over" >&2
fi
# Release builds run under the hardened runtime, as the notarized DMG does,
# so the installed app behaves like the one users download.
options=()
[[ "$config" == release && -n "$identity" ]] && options=(--options runtime)
codesign --force ${options[@]+"${options[@]}"} --sign "${identity:--}" "$app/Contents/MacOS/sidescreen-colorsync" >/dev/null
codesign --force ${options[@]+"${options[@]}"} --sign "${identity:--}" "$app" >/dev/null
echo "$app (signed: ${identity:-ad-hoc})"
