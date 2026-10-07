#!/usr/bin/env bash
# Build a release Sidescreen.app and install it in /Applications, then run it.
#
# Usage: scripts/install.sh
#
# The installed copy is the one to use day to day: debug builds in
# rust/target don't overwrite it. It keeps the Screen Recording and
# Accessibility grants, which follow the bundle id and signature. A running
# Sidescreen, Swift or Rust, quits first: both use the same control socket.
set -euo pipefail

cd "$(dirname "$0")/.."
scripts/bundle.sh release >/dev/null
built="$(cd .. && pwd)/target/Sidescreen.app"
installed=/Applications/Sidescreen.app

# A normal quit hands each display's windows back; a kill leaves them to
# macOS, which moves them onto the user's screen.
for app in "$installed" "$built" "$(cd ../.. && pwd)/apps/menubar/.build/Sidescreen.app"; do
  # Telling an app that isn't running to quit would launch it first.
  if pgrep -f "$app/Contents/MacOS/Sidescreen" >/dev/null; then
    osascript -e "tell application \"$app\" to quit" >/dev/null 2>&1 || true
  fi
done
for _ in $(seq 1 20); do
  pgrep -f "Sidescreen.app/Contents/MacOS/Sidescreen$" >/dev/null || break
  sleep 0.25
done

rm -rf "$installed"
ditto "$built" "$installed"
open "$installed"
echo "$installed"
