#!/usr/bin/env bash
# Build Agent Screen.app from the Rust binary.
#
# Usage: scripts/bundle.sh [debug|release]
#
# Signing: macOS ties Screen Recording and Accessibility grants to the code
# signature and bundle id. This bundle signs with the first Apple
# Development identity, else Developer ID Application, so
# builds keep the grants.
# Override with AGENTSCREEN_SIGN_IDENTITY (use "-" for ad-hoc).
set -euo pipefail

cd "$(dirname "$0")/.."
config="${1:-debug}"
flags=()
[[ "$config" == release ]] && flags=(--release)
cargo build -p agent-screen -p agent-screen-colorsync ${flags[@]+"${flags[@]}"}

target="$(cd ../.. && pwd)/target"
# Development builds are a separate app, "Agent Screen Dev", so they never
# take the installed app's privacy grants, socket or agent registration.
if [[ "$config" == release ]]; then
  app="$target/Agent Screen.app"
else
  app="$target/Agent Screen Dev.app"
fi
rm -rf "$app"
mkdir -p "$app/Contents/MacOS" "$app/Contents/Resources"
cp "$target/$config/AgentScreen" "$app/Contents/MacOS/AgentScreen"
# The ColorSync helper, copied to a root-owned place when the user sets it up (ADR 0009).
cp "$target/$config/agent-screen-colorsync" "$app/Contents/MacOS/agent-screen-colorsync"
cp resources/Info.plist "$app/Contents/Info.plist"
if [[ "$config" != release ]]; then
  plutil -replace CFBundleIdentifier -string app.agentscreen.AgentScreen.dev "$app/Contents/Info.plist"
  plutil -replace CFBundleName -string "Agent Screen Dev" "$app/Contents/Info.plist"
  plutil -replace CFBundleDisplayName -string "Agent Screen Dev" "$app/Contents/Info.plist"
fi
# AppIcon.icon is the Icon Composer source. macOS 26 draws the icon from
# Assets.car (Liquid Glass, Dark, Tinted, Clear); older macOS uses AppIcon.icns.
# After editing the .icon, rebuild Assets.car with:
#   xcrun actool resources/AppIcon.icon --compile <dir> --platform macosx \
#     --minimum-deployment-target 15.0 --app-icon AppIcon --output-partial-info-plist <dir>/p.plist
# and the Settings window's flat copy, assets/app-icon.png, from AppIcon.icns:
#   iconutil -c iconset resources/AppIcon.icns -o <dir>/i.iconset
#   cp <dir>/i.iconset/icon_128x128@2x.png assets/app-icon.png
cp resources/AppIcon.icns "$app/Contents/Resources/AppIcon.icns"
cp resources/Assets.car "$app/Contents/Resources/Assets.car"
cp resources/AgentDisplay.png "$app/Contents/Resources/AgentDisplay.png"
# Release builds update themselves through Sparkle (ADR 0012). Agent Screen
# isn't sandboxed, so Sparkle's XPC services, which only sandboxed apps
# need, stay out.
if [[ "$config" == release ]]; then
  sparkle="$(scripts/sparkle.sh)"
  mkdir -p "$app/Contents/Frameworks"
  ditto "$sparkle/Sparkle.framework" "$app/Contents/Frameworks/Sparkle.framework"
  rm -rf "$app/Contents/Frameworks/Sparkle.framework/Versions/B/XPCServices"
fi

identity="${AGENTSCREEN_SIGN_IDENTITY:-}"
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
codesign --force ${options[@]+"${options[@]}"} --sign "${identity:--}" "$app/Contents/MacOS/agent-screen-colorsync" >/dev/null
# Sparkle's parts inside out, then the framework; notarization wants each
# under the hardened runtime with a secure timestamp.
framework="$app/Contents/Frameworks/Sparkle.framework"
if [[ -d "$framework" ]]; then
  nested=("${options[@]+"${options[@]}"}")
  [[ -n "$identity" ]] && nested+=(--timestamp)
  for part in "$framework/Versions/B/Autoupdate" "$framework/Versions/B/Updater.app" "$framework"; do
    codesign --force ${nested[@]+"${nested[@]}"} --sign "${identity:--}" "$part" >/dev/null
  done
fi
codesign --force ${options[@]+"${options[@]}"} --sign "${identity:--}" "$app" >/dev/null
echo "$app (signed: ${identity:-ad-hoc})"
