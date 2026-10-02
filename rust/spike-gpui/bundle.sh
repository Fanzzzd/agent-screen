#!/usr/bin/env bash
# Bundle the spike as an app signed like the Swift build, with the same
# bundle identifier, so it inherits Sidescreen's Screen Recording grant.
set -euo pipefail
cd "$(dirname "$0")/.."
cargo build --release -p spike-gpui
app="target/SidescreenSpike.app"
rm -rf "$app"
mkdir -p "$app/Contents/MacOS"
cp target/release/spike-gpui "$app/Contents/MacOS/spike-gpui"
swiftc -O spike-gpui/tools/busy.swift -o "$app/Contents/MacOS/busy"
cp ../apps/menubar/Resources/Info.plist "$app/Contents/Info.plist"
plutil -replace CFBundleExecutable -string spike-gpui "$app/Contents/Info.plist"
plutil -replace CFBundleName -string "Sidescreen Spike" "$app/Contents/Info.plist"
plutil -remove CFBundleIconFile "$app/Contents/Info.plist"
identity="$(security find-identity -v -p codesigning | awk -F'"' '/Apple Development/ { print $2; exit }')"
codesign --force --sign "${identity:--}" "$app" >/dev/null
echo "$PWD/$app"
