Name:           agent-screen
Version:        %{_as_version}
Release:        1%{?dist}
Summary:        Isolated Wayland displays for AI agents
License:        MIT
URL:            https://agentscreen.app

Requires:       sway
Requires:       xorg-x11-server-Xwayland
Requires:       dbus-daemon
Requires:       at-spi2-core
Requires:       libwayland-client
Requires:       libxkbcommon
Requires:       libxkbcommon-x11
Requires:       libxcb
Requires:       fontconfig
Requires:       vulkan-loader
Recommends:     ffmpeg-free

%description
Agent Screen gives each AI coding agent its own isolated Wayland compositor,
seat, clipboard, and AT-SPI2 bus so your mouse, keyboard, and window focus
remain untouched while agents operate GUI apps and browsers.

%install
mkdir -p %{buildroot}%{_bindir}
mkdir -p %{buildroot}%{_userunitdir}
mkdir -p %{buildroot}%{_datadir}/applications
mkdir -p %{buildroot}%{_licensedir}/%{name}
install -m 0755 %{_as_bin} %{buildroot}%{_bindir}/agent-screen
install -m 0644 %{_as_root}/apps/linux/packaging/systemd/agent-screen.service %{buildroot}%{_userunitdir}/agent-screen.service
install -m 0644 %{_as_root}/apps/linux/packaging/applications/app.agentscreen.AgentScreen.desktop %{buildroot}%{_datadir}/applications/app.agentscreen.AgentScreen.desktop
install -m 0644 %{_as_root}/LICENSE %{buildroot}%{_licensedir}/%{name}/LICENSE

%files
%license %{_licensedir}/%{name}/LICENSE
%{_bindir}/agent-screen
%{_userunitdir}/agent-screen.service
%{_datadir}/applications/app.agentscreen.AgentScreen.desktop
