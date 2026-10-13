# Sidescreen on Linux: feasibility (2026-10-03)

Desk research only; no Linux machine was available. The research read upstream mutter,
xdg-desktop-portal-gnome, xdg-desktop-portal-kde and sway source (Sept/Oct 2026) and the freedesktop portal specs. Unverified items say so.

## Main finding

Every Wayland desktop can create a virtual output inside the user's
session. On GNOME, KDE and Hyprland, input injection drives the user's
single seat, so it moves the user's pointer and keyboard focus. No Wayland API
targets input at one window. Two setups avoid that: a second seat on sway,
or a private headless compositor per agent. Only the second works on every
desktop, so it is the recommendation.

## Capability × environment

| Capability                                      | GNOME 50+                                                      | KDE Plasma 6                                            | Sway / Hyprland                                                  | X11                                                           | Private headless compositor                     |
| ----------------------------------------------- | -------------------------------------------------------------- | ------------------------------------------------------- | ---------------------------------------------------------------- | ------------------------------------------------------------- | ----------------------------------------------- |
| Virtual display                                 | Works: portal `VIRTUAL`, or Mutter `RecordVirtual` (no dialog) | Works: portal → `zkde_screencast.stream_virtual_output` | Works: `swaymsg create_output`, `hyprctl output create headless` | Works: xrandr VIRTUAL or Xvfb                                 | Works                                           |
| Keep the cursor out                             | Partial: Mutter requires a shared edge (1 px minimum)          | Unverified                                              | Works: disjoint layout                                           | Works                                                         | Works                                           |
| Consent dialog                                  | Once, then `restore_token`                                     | Once, then token                                        | None                                                             | None                                                          | None                                            |
| Capture                                         | PipeWire                                                       | PipeWire                                                | Portal or `ext-image-copy-capture-v1`                            | XShm                                                          | Screencopy, no portal                           |
| Input without moving the user's cursor or focus | Blocked: EIS on the single seat                                | Blocked                                                 | Sway: second seat works. Hyprland: blocked                       | Partial: XTest moves the pointer; XSendEvent is often ignored | Works: own seat                                 |
| Enumerate and move windows                      | Partial: needs a Shell extension                               | Works: KWin scripting                                   | Works: IPC                                                       | Works: EWMH                                                   | Enumerate only; Wayland apps cannot be moved in |
| AT-SPI                                          | Partial: window-relative coordinates                           | Partial                                                 | Partial                                                          | Works                                                         | Works, per-agent bus                            |
| Tray                                            | Needs the AppIndicator extension                               | Works: SNI                                              | Works: SNI                                                       | Works                                                         | n/a                                             |
| Non-activating panel                            | Blocked: no layer-shell                                        | Works: layer-shell                                      | Works: layer-shell                                               | Works                                                         | n/a                                             |

GNOME 50 (March 2026) removed the X11 session, and Plasma 6.8 is Wayland-only.

## Recommended architecture

1. **Each Agent Display is a private headless Wayland compositor with Xwayland.**
   - Spike it with headless sway (`WLR_BACKENDS=headless`).
   - Then move to an in-process compositor on smithay, the library behind
     cosmic-comp and niri.
   - Agent apps get their own `WAYLAND_DISPLAY`, `DISPLAY` and a private D-Bus
     session, which also gives each agent its own AT-SPI bus.
2. **Capture.** The compositor hands frames straight to the GPUI Viewer. No
   portal, no PipeWire, no consent dialog.
3. **Input.** Sidescreen injects into the compositor's own seat, so the user's cursor and
   focus never move.
4. **Host UI.**
   - The Viewer is a GPUI window, and the tray uses `ksni`.
   - KDE and wlroots use a layer-shell panel; GPUI has `WindowKind::LayerShell`.
   - GNOME uses an ordinary window. A Shell extension is optional.
5. **Dropped capability.** Moving the user's existing windows onto an Agent
   Display does not work on Wayland. It could come back as an opt-in mode on KDE and sway.

**First target:** Ubuntu 26.04 with GNOME 50, the most users and the hardest host
UI. The nested engine does not depend on the desktop.

## Risks

1. **Single-instance apps.** Firefox, Chrome, VS Code, Electron and
   D-Bus-unique apps hand a new launch to the instance already open on the
   user's display. They need separate profiles and a private bus. This is the
   biggest product risk.
2. **Window moves.** The macOS flow of moving a window to the agent and back
   has no Wayland equivalent.
3. **GPU and performance** of headless compositors on Intel, AMD and NVIDIA.
4. **GNOME host UI.** No layer-shell, and no tray without an extension.
5. **AT-SPI quality.** Chromium needs accessibility forced on.

## Spike plan (Ubuntu 26.04, GNOME 50)

1. **App isolation (days 1–2).** Run headless sway with Xwayland under
   `dbus-run-session`. Launch Firefox, Chromium, an Electron app, VS Code,
   GTK4 and Qt6 while the user already has them open, and record which ones
   land in the agent display. This step is the go/no-go.
2. **Capture into a GPUI Viewer (days 3–4).** Use `ext-image-copy-capture-v1`.
   Measure fps and CPU.
3. **Input and AX (day 5).** Use `zwlr_virtual_pointer` and
   `zwp_virtual_keyboard`, confirm the GNOME cursor stays put, and read
   AT-SPI on the private bus.
4. **GNOME host UI (day 6).** Test the `ksni` tray, and check whether a GPUI
   window takes focus when it appears.

## Spike results (2026-10-04, Docker on Apple silicon)

`rust/spike-linux` runs two headless sway compositors for one user and one
`$HOME` in a Debian trixie container (aarch64, pixman renderer, no GPU). One
stands in for the user's desktop, the other for an Agent Display with its own
D-Bus session. Each app starts on the user's compositor first, then again on
the agent's. Run it with `docker build -t sidescreen-spike-linux rust/spike-linux`
and `docker run --rm --shm-size 1g sidescreen-spike-linux`.

**App isolation, step 1.** The second launch, with the agent's own D-Bus, landed as follows.

| App                      | Second launch                                                      | With its own profile      | With its own `HOME` |
| ------------------------ | ------------------------------------------------------------------ | ------------------------- | ------------------- |
| foot (Wayland)           | Agent                                                              | n/a                       | n/a                 |
| xterm (Xwayland)         | Agent                                                              | n/a                       | n/a                 |
| GNOME Text Editor (GTK4) | Agent                                                              | n/a                       | n/a                 |
| KWrite (Qt6)             | Agent                                                              | n/a                       | Agent               |
| Firefox ESR              | Refused: "Close Firefox" dialog on the agent                       | Agent (`--profile`)       | Agent               |
| Chromium                 | Handed to the user's instance: new window on the user's compositor | Agent (`--user-data-dir`) | Agent               |

The private D-Bus is enough for GTK and Qt apps, whose single-instance checks go through
the session bus. Browsers lock their profile directory, so they need
their own profile. A separate `HOME` for every agent app fixes all six, at the cost
of the user's settings and logins inside agent apps. The step is a go: launch agent apps
with a private D-Bus and their own `HOME`, and offer per-app profiles where
an agent needs the user's settings.

**Input, step 3.** `wtype` (virtual keyboard) and `wlrctl` (virtual pointer)
typed into a terminal on the agent's compositor. Keyboard focus on the user's
compositor stayed on the same window.

**AT-SPI, step 3.** The accessibility registry on the agent's D-Bus listed only
the agent's Text Editor, not the user's copy.

**Not covered yet.** Capture frame rate into a GPUI Viewer (step 2), the GNOME host
UI (step 4), Electron and VS Code, and GPU renderers.

ashpd 0.13, pipewire 0.10, reis 0.7 (libei), zbus 5.19, atspi 0.30,
ksni 0.3.6, smithay-client-toolkit 0.21, wayland-protocols-wlr 0.3, x11rb 0.14.

## Sources

- https://flatpak.github.io/xdg-desktop-portal/docs/doc-org.freedesktop.portal.ScreenCast.html
- https://flatpak.github.io/xdg-desktop-portal/docs/doc-org.freedesktop.portal.RemoteDesktop.html
- https://gitlab.gnome.org/GNOME/mutter (RecordVirtual, `mtk-rectangle.c`, `meta-remote-desktop-session.c`)
- https://gitlab.gnome.org/GNOME/mutter/-/merge_requests/1698
- https://invent.kde.org/plasma/xdg-desktop-portal-kde
- https://wayland.app/protocols/kde-zkde-screencast-unstable-v1
- https://github.com/swaywm/sway (`create_output.c`, `sway-input.5.scd`)
- https://discourse.gnome.org/t/unable-to-call-the-remote-getwindows-gnome-method-via-dbus/21201
- https://www.theregister.com/2025/11/28/kde_6_8_wayland_only/
