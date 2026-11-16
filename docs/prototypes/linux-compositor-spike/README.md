# Linux compositor and AT-SPI2 isolation spike

`linux-compositor-spike` runs five live checks against an isolated headless `sway` compositor on a Linux desktop without moving the host pointer or changing host window focus.

## Run

```sh
cargo run --manifest-path docs/prototypes/linux-compositor-spike/Cargo.toml
```

The runner writes the measured JSON output to `last-run-report.json`.

## Checks

1. `systemd --user` transient scope survival: starts a helper daemon that places headless `sway`, rootless `Xwayland`, two private `dbus-daemon` buses, `at-spi2-registryd`, and a GTK4 window inside `agent-screen-spike-<pid>.scope`, kills the helper daemon with `SIGKILL`, and reattaches to the surviving sockets from `state.json`.
2. `wlr-screencopy-unstable-v1` and virtual input: reads `1280x800` frames over `wl_shm`, clicks and drags inside the GTK4 fixture with `zwlr_virtual_pointer_v1`, types ASCII and Chinese UTF-8 text through dynamic XKB keymaps on `zwp_virtual_keyboard_v1`, exercises `ctrl+c` and `ctrl+v` inside the display, and round-trips clipboard text over `zwlr_data_control_manager_v1`.
3. AT-SPI2 isolation and Xwayland `AT_SPI_BUS`: gives the display its own `XDG_RUNTIME_DIR` so Wayland and AT-SPI sockets cannot collide with `/run/user/<uid>/wayland-*` or `/run/user/<uid>/at-spi/bus_0`, verifies that an X11 Qt6 client misses the private bus before `AT_SPI_BUS` is written to the Xwayland root window and registers after `x11rb` sets `AT_SPI_BUS`, and checks GTK4 Wayland, Qt6 Wayland, Qt6 Xwayland, and Chromium accessibility trees including `password text` roles.
4. Private D-Bus services: serves a silent `org.freedesktop.Notifications` receiver on the display's session bus and omits `<standard_session_servicedirs/>` so `org.freedesktop.secrets` and `org.freedesktop.portal.Desktop` return `ServiceUnknown` immediately instead of blocking on D-Bus activation timeouts.
5. Host desktop non-interference: compares host Wayland sockets, `/run/user/<uid>/at-spi/bus_0` inode and mtime, `/tmp/.X11-unix/X0` inode, and Hyprland active window before, during, and after the run.
