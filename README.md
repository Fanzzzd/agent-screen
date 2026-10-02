# Sidescreen

A menu bar app that gives AI agents their own virtual displays on macOS. Hover the menu bar icon to see live thumbnails of every virtual display. Click to open a floating panel where you can operate that display without your real cursor leaving your screen.

## Layout

```
apps/menubar/     Swift menu bar app (SwiftPM, macOS 15+, Liquid Glass on macOS 26)
packages/mcp/     MCP server for agents (Effect v4)
docs/             ADRs and the AGENTS.md convention
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

- **Virtual displays** come from the private `CGVirtualDisplay` API, the same one DeskPad and BetterDisplay use. Each display is HiDPI, 1440 × 900 points by default. The app parks it diagonally off the bottom-right corner of your physical screens so the real cursor rarely wanders onto it.
- **Live frames** come from ScreenCaptureKit. `StreamHub` runs one stream per display at the highest frame rate any viewer asks for: 60 fps for the open panel, 12 for thumbnails, and a 1 fps floor.
- **Input** in the panel goes to the target process through SkyLight's `SLEventPostToPid`, stamped with window-routing fields, plus a focus-without-raise handshake. The recipe is ported from [trycua/cua](https://github.com/trycua/cua) (MIT). Your cursor stays put and your frontmost app keeps its place.

## Agent tools

`packages/mcp` is an MCP server over stdio. Register it with your client, for example `claude mcp add sidescreen -- bun /path/to/sidescreen/packages/mcp/src/main.ts`.

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

- **Takeover.** Your first click, scroll or key press in the Viewer pauses the agent. Its input tools fail until you press 交还控制 or collapse the Viewer.
- **Help Request.** `request_help` sends a notification and shows the reason in Peek and the Viewer. Clicking the notification opens the display in Takeover.
- **Agent Pointer.** The Viewer and Peek draw an arrow where the agent last acted. It is not on the display, so agent screenshots don't contain it.
- **Handover.** Drag one of your windows onto the Viewer, or use the pull button. When the display is dismissed, the window returns to where it was.
- **Dismissal.** Closing a display lists its windows with the agent's Disposition preselected, and you can change any of them. Quitting Sidescreen applies the same plan without asking.

## Test

```sh
bun run --filter @sidescreen/menubar test   # unit tests
bun packages/mcp/scripts/control.ts         # Takeover and Help Request, needs a debug build running
bun packages/mcp/scripts/smoke.ts           # end to end with Calculator
```

Debug builds accept `debugTakeover`, `debugShow` (viewer, peek, collapse, dismiss) and `debugHandover` on the control socket, so scripts can drive the UI without a real click. For click-level UI checks, run `codex exec` with Codex's `computer-use` server on the app `dev.sidescreen.menubar`. It clicks through the Viewer without moving your cursor. It cannot reach the menu bar item.

## Known limits

- Private APIs only. This cannot ship on the App Store and may break on a future macOS.
- The virtual display's own menu bar and status items do not take clicks from the panel.
- The first click on a window only activates it, the same as a real click on an inactive window.
- Clicks on an empty part of a display reach nothing. Passing them to Finder's desktop would trigger "click wallpaper to reveal desktop" and slide every window on every display aside.
- `launch_app` on a running single-window app (Calculator) gets no new window. Quit it first or use `place_window`.
- A window that refuses to close on dismissal, such as an Open panel or one with unsaved changes, is left to macOS, which moves it to the main display.
- Canvas apps and games that filter per-process events may ignore forwarded input.
- IOSurface-backed layers ignore ancestor transforms. The open and close morph therefore animates a still-frame proxy and swaps in the live view when it lands.
