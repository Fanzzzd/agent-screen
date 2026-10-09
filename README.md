# Sidescreen

A menu bar app that gives AI agents their own virtual displays on macOS. Hover the menu bar icon to see live thumbnails of every virtual display, or click it to keep them up until you click elsewhere. Click a thumbnail to open a floating panel where you can operate that display without your real cursor leaving your screen.

## Layout

```
rust/sidescreen/      the app in Rust: GPUI UI, control socket, `Sidescreen mcp`
rust/sidescreen-mcp/  the MCP server (rmcp), built into the app binary
rust/tests/mcp/       end-to-end scripts that act as an MCP client
rust/docs/            Linux and Windows feasibility notes
apps/menubar/         the Swift app the Rust one replaces (SwiftPM, macOS 15+)
docs/                 ADRs and the AGENTS.md convention
```

The Rust app has reached parity with the Swift app on macOS; see [ADR 0005](docs/adr/0005-rust-app-and-mcp.md). Both use the bundle id `dev.sidescreen.menubar` and the same control socket, so run one at a time.

## Develop

```sh
rust/sidescreen/scripts/bundle.sh [release]   # build rust/target/Sidescreen.app
bun run dev                                   # Swift: build, sign, restart the app
```

The Rust toolchain is pinned in `rust/rust-toolchain.toml`. Each bundle script signs with your first Apple Development identity. macOS ties the Screen Recording and Accessibility grants to that signature, so the grants survive rebuilds. An ad-hoc signature would lose them every build.

Development switches for the Rust app:

- `SIDESCREEN_DEBUG=1` enables the debug control methods in a release build, for measuring.
- `SIDESCREEN_APPEARANCE=light` or `dark` forces the app's appearance in debug builds, for screenshots.

## How it works

- **Virtual displays** come from the private `CGVirtualDisplay` API, the same one DeskPad and BetterDisplay use. Each display is HiDPI, 1440 × 900 points by default. The app parks it diagonally off the bottom-right corner of your physical screens so the real cursor rarely wanders onto it.
- **Live frames** come from ScreenCaptureKit as 420f pixel buffers that GPUI draws without a copy. One stream per display runs at the highest frame rate and size any view asks for: 60 fps at full size for the Viewer, 12 fps at 416 px wide for thumbnails.
- **Input** in the panel goes to the target process through SkyLight's `SLEventPostToPid`, stamped with window-routing fields, plus a focus-without-raise handshake. The recipe is ported from [trycua/cua](https://github.com/trycua/cua) (MIT). Your cursor stays put and your frontmost app keeps its place.

## Install

`rust/sidescreen/scripts/install.sh` builds a release app, quits any running Sidescreen, installs it as `/Applications/Sidescreen.app` and opens it. Debug builds in `rust/target` don't touch the installed copy. Grant it Screen & System Audio Recording and Accessibility in System Settings, Privacy & Security. An app started from a terminal borrows the terminal's grants instead, so a build that works from the shell can still lack its own.

## Agent tools

`Sidescreen mcp` is an MCP server over stdio. Register it with your client, for example `claude mcp add sidescreen -- /Applications/Sidescreen.app/Contents/MacOS/Sidescreen mcp`. It works with either app.

| Tool | What it does |
| --- | --- |
| `create_display`, `end_display` | Get the session's Agent Display (one per session) or finish with it. |
| `launch_app`, `place_window`, `list_windows` | Put windows on the display and list them. |
| `computer` | Anthropic-style actions on the whole display. Every action returns a screenshot. |
| `get_window_state`, `act_on_element` | One window: screenshot plus accessibility tree, and actions on numbered elements. |
| `set_disposition` | Keep or close a window when the user dismisses the display. |
| `wait_for_control`, `request_help` | Wait out a Takeover, or ask the user to step in. |

`docs/agents/AGENTS.md` is the convention for agents. Put it in a project's `AGENTS.md` or in `~/.codex/AGENTS.md`. Codex then places windows on its display first and can operate them with either Sidescreen's tools or its built-in computer use. In a test run, Codex with built-in computer use computed 7+8 in Calculator on its Agent Display. The user's cursor and frontmost app did not change.

## The user's side

- **Peek.** Hover the menu bar icon, or click it to keep Peek up. Displays in use come first, then those waiting for review. The narrow + creates a display of your own.
- **Takeover.** Your first click, scroll or key press in the Viewer, or Take Over (接手操作), pauses the agent. Its input tools fail until you press Let … Continue (让 … 继续). Collapsing the Viewer keeps the Takeover.
- **Help Request.** `request_help` sends a notification and shows the reason in Peek and the Viewer. Clicking the notification opens the display in Takeover.
- **Agent Pointer.** The Viewer and Peek draw an arrow where the agent last acted. It is not on the display, so agent screenshots don't contain it.
- **Handover.** Drag one of your windows onto the Viewer, or pick it under Windows (窗口) ▸ Hand Over to … (交给 …). Windows ▸ Take Back Window (取回窗口) brings any window back. A handed-over window returns to where it was.
- **Review.** When the agent stops using a display, the display closes by itself if it is empty, and otherwise shows Needs Review (待查看) until you press Review Windows… (处理窗口…). See [ADR 0004](docs/adr/0004-display-lifecycle.md).
- **Dismissal.** Closing a display asks on the Viewer: it lists the windows with the agent's Disposition preselected, and you can change any of them. Esc cancels, ⌘Return confirms. Adding or closing a display keeps Peek and the Viewer open; the Viewer moves to the next display. Quitting Sidescreen applies the same plan without asking.

## Test

```sh
cd rust && cargo test                 # unit tests
bun rust/tests/mcp/control.ts         # Takeover and Help Request, needs a debug build running
bun rust/tests/mcp/smoke.ts           # end to end with Calculator
bun rust/tests/e2e/run.ts             # the user's side: scrolling, the menu bar strip, permission setup
```

`rust/tests/e2e` posts real pointer events, so it borrows your cursor. It waits until you have been idle for 45 s with no Agent Display open (`E2E_NOW=1` skips the wait), then puts the cursor, the front app, the AeroSpace workspace and the installed Sidescreen back. Name cases to run some: `scroll`, `strip`, `permissions`. The permission case runs a copy under the bundle id `dev.sidescreen.e2e-ungranted`, so it starts with no grants and never touches yours; what it checks ends where only a person can flip the switch. Captures go to `rust/target/e2e`. `probe tap` in `tests/e2e/tools` records real trackpad events for `hid replay`, which is how `fixtures/trackpad.log` was made.

The scripts start `rust/target/Sidescreen.app/Contents/MacOS/Sidescreen mcp`; `SIDESCREEN_MCP="<binary> [args]"` picks another server. Quit Calculator after `smoke.ts`: it marks the window to keep, and quitting the app would hand it back to your screen.

Debug builds accept `debugTakeover` and `debugShow` on the control socket, so scripts can drive the UI without a real click. `debugShow` takes a `surface`: peek, click, menu, hide, select, viewer, collapse, create, dismiss, answer, primary, windows, help, focus, quit, setup, setup-close, assist-screen, assist-ax, assist-preview, assist-preview-unlisted or assist-hide, and a `displayId` where one applies. `debugViewer` returns the Viewer's frame and its display image in screen points. For click-level UI checks, run `codex exec` with Codex's `computer-use` server on the app `dev.sidescreen.menubar`. It clicks through the Viewer without moving your cursor. It cannot reach the menu bar item.

## Known limits

- Private APIs only. This cannot ship on the App Store and may break on a future macOS.
- The menu bar strip in the Viewer mirrors your own front app. A click on it opens the menus of the app in front on the Agent Display instead; status items there take no clicks.
- The first click on a window only activates it, the same as a real click on an inactive window.
- Clicks on an empty part of a display reach nothing. Passing them to Finder's desktop would trigger "click wallpaper to reveal desktop" and slide every window on every display aside.
- `launch_app` on a running single-window app (Calculator) gets no new window. Quit it first or use `place_window`.
- A window that refuses to close on dismissal, such as an Open panel or one with unsaved changes, is left to macOS, which moves it to the main display.
- Canvas apps and games that filter per-process events may ignore forwarded input.
- GPUI redraws the Viewer on every new frame: a release build uses 3.3% CPU with the Viewer open on a 60 fps display, against 1.6% for the Swift app. Debug builds use about twice that, mostly in layout.
- With the menu bar icon hidden, for example by a menu bar manager or behind the notch, Peek and the Viewer drop from the top center of the screen under the pointer.
