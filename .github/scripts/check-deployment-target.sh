#!/usr/bin/env bash
# Check that the macOS version the app says it needs is the one it is built
# for. LSMinimumSystemVersion in Info.plist is the single source: bundle.sh,
# release.sh and the macOS CI jobs export it as MACOSX_DEPLOYMENT_TARGET,
# which rustc writes into the binary's minos and the cc crate passes to
# clang. Unset, rustc writes 11.0 and the cc crate compiles apps/macos/native
# for the SDK (26.5 in November), so clang links a macOS 26-only API in the
# shims without the availability warning that says macOS 15 lacks it.
#
# Usage: check-deployment-target.sh binary...
#
# Fails when MACOSX_DEPLOYMENT_TARGET, if set, the runner of CI's
# starts-on-macos job, or the minos of any binary given differs from
# LSMinimumSystemVersion. At least one binary is required. Read-only.
set -euo pipefail

root="$(cd "$(dirname "$0")/../.." && pwd)"
plist="$root/apps/macos/resources/Info.plist"
want="$(/usr/libexec/PlistBuddy -c "Print :LSMinimumSystemVersion" "$plist")"
if [[ -z "$want" ]]; then
  echo "error: $plist has no LSMinimumSystemVersion" >&2
  exit 1
fi

# 15, 15.0 and 15.0.0 are the same version.
normal() { sed -E 's/(\.0)+$//' <<<"$1"; }
failed=0
mismatch() {
  echo "error: $1 is $2, LSMinimumSystemVersion is $want" >&2
  failed=1
}

if [[ -n "${MACOSX_DEPLOYMENT_TARGET:-}" && "$(normal "$MACOSX_DEPLOYMENT_TARGET")" != "$(normal "$want")" ]]; then
  mismatch MACOSX_DEPLOYMENT_TARGET "$MACOSX_DEPLOYMENT_TARGET"
fi

# The job that starts the release binary on the oldest macOS it supports.
runner="$(awk '/^  starts-on-macos-[0-9]+:/ { job = 1; next }
  job && /^  [^ ]/ { exit }
  job && /runs-on:/ { sub(/.*macos-/, ""); print; exit }' "$root/.github/workflows/ci.yml")"
if [[ "$runner" != "${want%%.*}" ]]; then
  mismatch "ci.yml's starts-on-macos runner" "macos-${runner:-missing}"
fi

if [[ $# -eq 0 ]]; then
  echo "usage: check-deployment-target.sh binary..." >&2
  exit 2
fi

for binary in "$@"; do
  # One minos per architecture of a universal binary.
  minos="$(vtool -show-build "$binary" | awk '$1 == "minos" { print $2 }' | sort -u)"
  if [[ -z "$minos" ]]; then
    mismatch "$binary's minos" missing
    continue
  fi
  for version in $minos; do
    [[ "$(normal "$version")" == "$(normal "$want")" ]] || mismatch "$binary's minos" "$version"
  done
done

[[ "$failed" == 0 ]] && echo "deployment target $want: Info.plist, environment, CI runner and $# binaries agree"
exit "$failed"
