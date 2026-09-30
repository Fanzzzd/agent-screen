# Sidescreen

A menu bar app that gives AI agents their own virtual displays on macOS. Hover the menu bar icon to see live thumbnails of every virtual display. Click to open a floating panel where you can operate that display without your real cursor leaving your screen.

## Layout

```
apps/menubar/     Swift menu bar app (SwiftPM, macOS 15+, Liquid Glass on macOS 26)
packages/         TypeScript packages (planned: MCP server in Effect)
```

## Develop

```sh
bun run dev                          # build, sign, restart the app
apps/menubar/scripts/bundle.sh       # build .build/Sidescreen.app only
```

The bundle script signs with your first Apple Development identity. macOS ties the Screen Recording and Accessibility grants to that signature, so the grants survive rebuilds. An ad-hoc signature would lose them every build.

Development switches:

- `SIDESCREEN_DEMO=panel` or `preview` opens the UI 1.5 s after launch.
- `Sidescreen --probe displays | pull <id> | click <x> <y> | type <text>` drives the input path from a terminal without the UI.

## How it works

- **Virtual displays** come from the private `CGVirtualDisplay` API, the same one DeskPad and BetterDisplay use. Each display is HiDPI, 1600 × 1000 points by default. The app parks it diagonally off the bottom-right corner of your physical screens so the real cursor rarely wanders onto it.
- **Live frames** come from ScreenCaptureKit. `StreamHub` runs one stream per display at the highest frame rate any viewer asks for: 60 fps for the open panel, 12 for thumbnails, and a 1 fps floor.
- **Input** in the panel goes to the target process through SkyLight's `SLEventPostToPid`, stamped with window-routing fields, plus a focus-without-raise handshake. The recipe is ported from [trycua/cua](https://github.com/trycua/cua) (MIT). Your cursor stays put and your frontmost app keeps its place.

## Known limits

- Private APIs only. This cannot ship on the App Store and may break on a future macOS.
- The virtual display's own menu bar and status items do not take clicks from the panel.
- The first click on a window only activates it, the same as a real click on an inactive window.
- Canvas apps and games that filter per-process events may ignore forwarded input.
- IOSurface-backed layers ignore ancestor transforms. The open and close morph therefore animates a still-frame proxy and swaps in the live view when it lands.
