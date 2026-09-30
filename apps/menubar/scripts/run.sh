#!/usr/bin/env bash
# Rebuild, then restart the app. Logs stream to this terminal.
set -euo pipefail

cd "$(dirname "$0")/.."
scripts/bundle.sh "${1:-debug}"
pkill -x Sidescreen 2>/dev/null || true
exec .build/Sidescreen.app/Contents/MacOS/Sidescreen
