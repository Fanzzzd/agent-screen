#!/usr/bin/env bash
# Builds target/agent-screen_<version>_<arch>.deb from target/release/agent-screen
# (or $AGENTSCREEN_BIN when already built).
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../../.." && pwd)"
cd "$ROOT"

VERSION="$(awk -F'"' '/^version/ { print $2; exit }' apps/linux/Cargo.toml)"
BIN="${AGENTSCREEN_BIN:-$ROOT/target/release/agent-screen}"

if [[ ! -x "$BIN" ]]; then
  cargo build --release -p agent-screen-linux
fi

UNAME_M="$(uname -m)"
case "$UNAME_M" in
  x86_64) DEB_ARCH="amd64" ;;
  aarch64|arm64) DEB_ARCH="arm64" ;;
  *) DEB_ARCH="$(dpkg --print-architecture)" ;;
esac

STAGE="$(mktemp -d)"
trap 'rm -rf "$STAGE"' EXIT

PKG_DIR="$STAGE/agent-screen_${VERSION}_${DEB_ARCH}"
mkdir -p \
  "$PKG_DIR/DEBIAN" \
  "$PKG_DIR/usr/bin" \
  "$PKG_DIR/usr/lib/systemd/user" \
  "$PKG_DIR/usr/share/applications" \
  "$PKG_DIR/usr/share/doc/agent-screen"

install -m 0755 "$BIN" "$PKG_DIR/usr/bin/agent-screen"
install -m 0644 apps/linux/packaging/systemd/agent-screen.service "$PKG_DIR/usr/lib/systemd/user/agent-screen.service"
install -m 0644 apps/linux/packaging/applications/app.agentscreen.AgentScreen.desktop "$PKG_DIR/usr/share/applications/app.agentscreen.AgentScreen.desktop"
install -m 0644 LICENSE "$PKG_DIR/usr/share/doc/agent-screen/copyright"

cat > "$PKG_DIR/DEBIAN/control" <<EOF
Package: agent-screen
Version: ${VERSION}
Section: devel
Priority: optional
Architecture: ${DEB_ARCH}
Maintainer: Agent Screen Contributors <https://github.com/Fanzzzd/agent-screen>
Depends: sway, xwayland, dbus, at-spi2-core, libwayland-client0, libxkbcommon0, libxkbcommon-x11-0, libxcb1, libfontconfig1, libvulkan1
Recommends: ffmpeg
Homepage: https://agentscreen.app
Description: Isolated Wayland displays for AI agents
 Agent Screen gives each AI coding agent its own isolated Wayland compositor,
 seat, clipboard, and AT-SPI2 bus so your mouse, keyboard, and window focus
 remain untouched while agents operate GUI apps and browsers.
EOF

OUT="$ROOT/target/agent-screen_${VERSION}_${DEB_ARCH}.deb"
if command -v dpkg-deb >/dev/null 2>&1; then
  dpkg-deb --root-owner-group --build "$PKG_DIR" "$OUT"
else
  printf '2.0\n' > "$STAGE/debian-binary"
  tar --owner=0 --group=0 -czf "$STAGE/control.tar.gz" -C "$PKG_DIR/DEBIAN" .
  rm -rf "$PKG_DIR/DEBIAN"
  tar --owner=0 --group=0 -czf "$STAGE/data.tar.gz" -C "$PKG_DIR" .
  rm -f "$OUT"
  ar rcs "$OUT" "$STAGE/debian-binary" "$STAGE/control.tar.gz" "$STAGE/data.tar.gz"
fi
echo "Built $OUT"
