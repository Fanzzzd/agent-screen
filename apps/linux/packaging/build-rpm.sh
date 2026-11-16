#!/usr/bin/env bash
# Builds target/agent-screen-<version>-1.<arch>.rpm using rpmbuild.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../../.." && pwd)"
cd "$ROOT"

VERSION="$(awk -F'"' '/^version/ { print $2; exit }' apps/linux/Cargo.toml)"
BIN="${AGENTSCREEN_BIN:-$ROOT/target/release/agent-screen}"

if [[ ! -x "$BIN" ]]; then
  cargo build --release -p agent-screen-linux
fi

TOPDIR="$(mktemp -d)"
trap 'rm -rf "$TOPDIR"' EXIT
mkdir -p "$TOPDIR"/{BUILD,RPMS,SOURCES,SPECS,SRPMS}

rpmbuild -bb \
  --define "_topdir $TOPDIR" \
  --define "_as_version $VERSION" \
  --define "_as_bin $BIN" \
  --define "_as_root $ROOT" \
  apps/linux/packaging/agent-screen.spec

find "$TOPDIR/RPMS" -name '*.rpm' -exec cp -f {} "$ROOT/target/" \;
ls -lh "$ROOT"/target/agent-screen-"$VERSION"-*.rpm
