#!/usr/bin/env bash
# Build Sidescreen-<version>.dmg and its Homebrew cask for a GitHub release.
#
# Usage: scripts/release.sh
#
# Signs with a Developer ID Application identity under the hardened
# runtime. Releases aren't notarized: users approve the app once in System
# Settings. NOTARIZE=1 notarizes and staples too, given notarytool
# credentials stored once under a keychain profile:
#
#   xcrun notarytool store-credentials sidescreen \
#     --apple-id <you@example.com> --team-id <TEAMID> --password <app-specific password>
#
# NOTARY_PROFILE picks another profile name.
set -euo pipefail

cd "$(dirname "$0")/.."
target="$(cd .. && pwd)/target"
version="$(awk -F'"' '/^version/ { print $2; exit }' Cargo.toml)"
profile="${NOTARY_PROFILE:-sidescreen}"

identity="${SIDESCREEN_SIGN_IDENTITY:-$(security find-identity -v -p codesigning |
  awk -F'"' 'index($2, "Developer ID Application") == 1 { print $2; exit }')}"
if [[ -z "$identity" ]]; then
  echo "error: no Developer ID Application identity; notarization needs one" >&2
  exit 1
fi

if [[ "${NOTARIZE:-}" == 1 ]] && ! xcrun notarytool history --keychain-profile "$profile" >/dev/null 2>&1; then
  echo "error: no notarytool credentials under the profile \"$profile\"; see the top of $0" >&2
  exit 1
fi

SIDESCREEN_SIGN_IDENTITY="$identity" scripts/bundle.sh release >/dev/null
app="$target/Sidescreen.app"
plist="$app/Contents/Info.plist"
/usr/libexec/PlistBuddy -c "Set :CFBundleShortVersionString $version" "$plist"
/usr/libexec/PlistBuddy -c "Set :CFBundleVersion $version" "$plist"
# Notarization wants the hardened runtime and a secure timestamp.
codesign --force --options runtime --timestamp --sign "$identity" "$app" 2>/dev/null
codesign --verify --strict "$app"

dmg="$target/Sidescreen-$version.dmg"
stage="$(mktemp -d)"
trap 'rm -rf "$stage"' EXIT
ditto "$app" "$stage/Sidescreen.app"
ln -s /Applications "$stage/Applications"
rm -f "$dmg"
hdiutil create -quiet -volname Sidescreen -srcfolder "$stage" -format UDZO -fs HFS+ "$dmg"
codesign --force --timestamp --sign "$identity" "$dmg"

if [[ "${NOTARIZE:-}" == 1 ]]; then
  xcrun notarytool submit "$dmg" --keychain-profile "$profile" --wait
  xcrun stapler staple "$dmg"
  spctl --assess --type open --context context:primary-signature --verbose "$dmg"
fi
sha="$(shasum -a 256 "$dmg" | cut -d' ' -f1)"

# The Homebrew cask for this release, for a tap repo's Casks folder.
repo="${SIDESCREEN_REPO:-fanzzzd/sidescreen}"
cask="$target/sidescreen.rb"
cat >"$cask" <<CASK
cask "sidescreen" do
  version "$version"
  sha256 "$sha"

  url "https://github.com/$repo/releases/download/v#{version}/Sidescreen-#{version}.dmg"
  name "Sidescreen"
  desc "Virtual displays for AI agents, so they work without taking your mouse"
  homepage "https://github.com/$repo"

  depends_on macos: :sequoia

  app "Sidescreen.app"

  zap trash: "~/Library/Application Support/Sidescreen"
end
CASK
echo "$dmg"
echo "$cask (sha256 $sha)"
