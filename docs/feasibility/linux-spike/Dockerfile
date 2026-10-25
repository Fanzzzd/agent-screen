# Linux spike, step 1: does an app launched into a private headless
# compositor open there, while the same app already runs on the "user's"
# compositor under the same home directory?
FROM debian:trixie
RUN apt-get update && DEBIAN_FRONTEND=noninteractive apt-get install -y --no-install-recommends \
      sway xwayland wtype wlrctl at-spi2-core python3-pyatspi dbus dbus-x11 grim jq procps ca-certificates \
      firefox-esr chromium gnome-text-editor kwrite qt6-wayland x11-apps xterm foot \
      fonts-dejavu-core && rm -rf /var/lib/apt/lists/*
RUN useradd -m agent
COPY isolation.sh /usr/local/bin/isolation.sh
RUN chmod +x /usr/local/bin/isolation.sh
USER agent
ENV XDG_RUNTIME_DIR=/tmp/runtime-agent
CMD ["/usr/local/bin/isolation.sh"]
