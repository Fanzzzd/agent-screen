#!/usr/bin/env bash
# Build AgentScreen-<version>.dmg and its Homebrew cask for a GitHub release.
#
# Usage: scripts/release.sh
#
# Signs with a Developer ID Application identity under the hardened
# runtime, then notarizes and staples the DMG. Notarization takes an App
# Store Connect API key: NOTARY_KEY (the .p8 file), NOTARY_KEY_ID and
# NOTARY_ISSUER, as CI passes them, or one stored once under a keychain
# profile:
#
#   xcrun notarytool store-credentials agent-screen \
#     --key <AuthKey_ID.p8> --key-id <ID> --issuer <issuer id>
#
# NOTARY_PROFILE picks another profile name. NOTARIZE=0 skips it, for a
# local build no one downloads.
#
# The DMG is also the update Sparkle downloads (ADR 0012): appcast.xml,
# beside it, describes it, signed with the EdDSA key whose public half is
# SUPublicEDKey in Info.plist. SPARKLE_ED_KEY holds the private key, as CI
# passes it, or SPARKLE_ED_KEY_FILE a file with it. Without either the
# appcast is left out, for a local build.
set -euo pipefail

cd "$(dirname "$0")/.."
target="$(cd ../.. && pwd)/target"
version="$(awk -F'"' '/^version/ { print $2; exit }' Cargo.toml)"
profile="${NOTARY_PROFILE:-agent-screen}"

identity="${AGENTSCREEN_SIGN_IDENTITY:-$(security find-identity -v -p codesigning |
  awk -F'"' 'index($2, "Developer ID Application") == 1 { print $2; exit }')}"
if [[ -z "$identity" ]]; then
  echo "error: no Developer ID Application identity; notarization needs one" >&2
  exit 1
fi

if [[ -n "${NOTARY_KEY_ID:-}" ]]; then
  notary=(--key "$NOTARY_KEY" --key-id "$NOTARY_KEY_ID" --issuer "$NOTARY_ISSUER")
else
  notary=(--keychain-profile "$profile")
fi
if [[ "${NOTARIZE:-1}" != 0 ]] && ! xcrun notarytool history "${notary[@]}" >/dev/null 2>&1; then
  echo "error: no notarytool credentials; see the top of $0" >&2
  exit 1
fi

AGENTSCREEN_SIGN_IDENTITY="$identity" scripts/bundle.sh release >/dev/null
app="$target/Agent Screen.app"
plist="$app/Contents/Info.plist"
/usr/libexec/PlistBuddy -c "Set :CFBundleShortVersionString $version" "$plist"
/usr/libexec/PlistBuddy -c "Set :CFBundleVersion $version" "$plist"
# Notarization wants the hardened runtime and a secure timestamp.
codesign --force --options runtime --timestamp --sign "$identity" "$app" 2>/dev/null
codesign --verify --strict "$app"

dmg="$target/AgentScreen-$version.dmg"
stage="$(mktemp -d)"
trap 'rm -rf "$stage"' EXIT
ditto "$app" "$stage/Agent Screen.app"
ln -s /Applications "$stage/Applications"
rm -f "$dmg"
hdiutil create -quiet -volname "Agent Screen" -srcfolder "$stage" -format UDZO -fs HFS+ "$dmg"
codesign --force --timestamp --sign "$identity" "$dmg"

if [[ "${NOTARIZE:-1}" != 0 ]]; then
  xcrun notarytool submit "$dmg" "${notary[@]}" --wait
  xcrun stapler staple "$dmg"
  spctl --assess --type open --context context:primary-signature --verbose "$dmg"
fi
sha="$(shasum -a 256 "$dmg" | cut -d' ' -f1)"
repo="${AGENTSCREEN_REPO:-fanzzzd/agent-screen}"

# The update feed: the latest release's appcast.xml names this DMG.
appcast="$target/appcast.xml"
rm -f "$appcast"
key_file="${SPARKLE_ED_KEY_FILE:-}"
if [[ -z "$key_file" && -n "${SPARKLE_ED_KEY:-}" ]]; then
  key_file="$(mktemp)"
  printf '%s\n' "$SPARKLE_ED_KEY" >"$key_file"
  trap 'rm -rf "$stage" "$key_file"' EXIT
fi
if [[ -n "$key_file" ]]; then
  sparkle="$(scripts/sparkle.sh)"
  signature="$("$sparkle/bin/sign_update" --ed-key-file "$key_file" "$dmg")"
  minimum="$(/usr/libexec/PlistBuddy -c "Print :LSMinimumSystemVersion" "$plist")"
  cat >"$appcast" <<APPCAST
<?xml version="1.0" encoding="utf-8"?>
<rss version="2.0" xmlns:sparkle="http://www.andymatuschak.org/xml-namespaces/sparkle">
  <channel>
    <title>Agent Screen</title>
    <item>
      <title>Agent Screen $version</title>
      <pubDate>$(LC_ALL=C date -u '+%a, %d %b %Y %H:%M:%S +0000')</pubDate>
      <sparkle:version>$version</sparkle:version>
      <sparkle:shortVersionString>$version</sparkle:shortVersionString>
      <sparkle:minimumSystemVersion>$minimum</sparkle:minimumSystemVersion>
      <sparkle:releaseNotesLink>https://github.com/$repo/releases/tag/v$version</sparkle:releaseNotesLink>
      <enclosure url="${AGENTSCREEN_UPDATE_URL:-https://github.com/$repo/releases/download/v$version/AgentScreen-$version.dmg}" $signature type="application/octet-stream"/>
    </item>
  </channel>
</rss>
APPCAST
  echo "$appcast"
else
  echo "warning: no SPARKLE_ED_KEY or SPARKLE_ED_KEY_FILE; no appcast.xml" >&2
fi

# The Homebrew cask for this release, for a tap repo's Casks folder.
cask="$target/agent-screen.rb"
cat >"$cask" <<CASK
cask "agent-screen" do
  version "$version"
  sha256 "$sha"

  url "https://github.com/$repo/releases/download/v#{version}/AgentScreen-#{version}.dmg"
  name "Agent Screen"
  desc "Virtual displays for AI agents, so they work without taking your mouse"
  homepage "https://agentscreen.app"

  depends_on macos: :sequoia

  livecheck do
    url :url
    strategy :github_latest
  end

  # Agent Screen updates itself (Sparkle), and brew leaves it to.
  auto_updates true

  app "Agent Screen.app"

  uninstall quit: "app.agentscreen.AgentScreen"

  zap trash: [
    "~/Library/Application Support/Agent Screen",
    "~/Library/Logs/Agent Screen",
  ]
end
CASK
echo "$dmg"
echo "$cask (sha256 $sha)"
