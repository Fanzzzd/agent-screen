# Developing Sidescreen

## Layout

```
rust/sidescreen/        the app: GPUI UI, control socket, `Sidescreen mcp`
rust/sidescreen-mcp/    the MCP server (rmcp), built into the app binary
rust/sidescreen-model/  domain rules with no OS calls: displays, windows, key chords
rust/tests/             MCP scripts, user-side e2e tests and the demo recorder
rust/docs/              Linux and Windows feasibility notes
docs/                   ADRs and the AGENTS.md convention
```

Sidescreen started as a Swift app; the Rust app replaced it ([ADR 0005](docs/adr/0005-rust-app-and-mcp.md)), and the Swift code lives on in the history.

## Build

```sh
rust/sidescreen/scripts/bundle.sh [release]   # build rust/target/Sidescreen.app
rust/sidescreen/scripts/release.sh            # the DMG and Homebrew cask for a release
```

The Rust toolchain is pinned in `rust/rust-toolchain.toml`. The bundle script signs with your first Apple Development identity. macOS ties the Screen Recording and Accessibility grants to that signature, so the grants survive rebuilds. An ad-hoc signature would lose them every build.

Development switches for the Rust app:

- `SIDESCREEN_DEBUG=1` enables the debug control methods in a release build, for measuring.
- `SIDESCREEN_APPEARANCE=light` or `dark` forces the app's appearance in debug builds, for screenshots.

## How it works

- **Virtual displays** come from the private `CGVirtualDisplay` API, the same one DeskPad and BetterDisplay use. Each display is HiDPI, 1280 × 800 points by default. The app parks it diagonally off the top-right corner of your physical screens so the real cursor rarely wanders onto it. Not a bottom corner: AeroSpace hides other workspaces' windows there, hanging off screen, and they would show on the display.
- **Live frames** come from ScreenCaptureKit as 420f pixel buffers that GPUI draws without a copy. One stream per display runs at the highest frame rate and size any view asks for: 60 fps at full size for the Viewer, 12 fps at 416 px wide for thumbnails.
- **Input** in the panel goes to the target process through SkyLight's `SLEventPostToPid`, stamped with window-routing fields, plus a focus record that tells the target window it has focus without raising it. The recipe is ported from [trycua/cua](https://github.com/trycua/cua) (MIT), minus its defocus record to the front app: that one ends the user's input method session. Your cursor stays put, and your front app keeps its place and its key window.

## Test

```sh
cd rust && cargo test                 # unit tests
bun rust/tests/mcp/control.ts         # Takeover and Help Request, needs a debug build running
bun rust/tests/mcp/smoke.ts           # end to end with Calculator
bun rust/tests/e2e/run.ts             # the user's side: scrolling, the menu bar strip, focus, permission setup
```

`rust/tests/e2e` posts real pointer events, so it borrows your cursor. It waits until you have been idle for 45 s with no Agent Display open (`E2E_NOW=1` skips the wait), then puts the cursor, the front app, the AeroSpace workspace and the installed Sidescreen back. Name cases to run some: `scroll`, `strip`, `focus`, `permissions`. `focus` puts a stand-in app with a text field in front and checks that it stays the key window while an agent types: losing it ends the user's input method session. The permission case runs a copy under the bundle id `dev.sidescreen.e2e-ungranted`, so it starts with no grants and never touches yours; what it checks ends where only a person can flip the switch. Captures go to `rust/target/e2e`. `probe tap` in `tests/e2e/tools` records real trackpad events for `hid replay`, which is how `fixtures/trackpad.log` was made.

The scripts start `rust/target/Sidescreen.app/Contents/MacOS/Sidescreen mcp`; `SIDESCREEN_MCP="<binary> [args]"` picks another server. Quit Calculator after `smoke.ts`: it marks the window to keep, and quitting the app would hand it back to your screen.

Debug builds accept `debugTakeover` and `debugShow` on the control socket, so scripts can drive the UI without a real click. `debugShow` takes a `surface`: peek, click, menu, hide, select, viewer, collapse, create, dismiss, answer, primary, windows, help, focus, quit, setup, setup-close, assist-screen, assist-ax, assist-preview, assist-preview-unlisted, assist-hide or connect (runs Connect Claude Code and Codex), and a `displayId` where one applies. `debugViewer` returns the Viewer's frame and its display image in screen points. For click-level UI checks, run `codex exec` with Codex's `computer-use` server on the app `dev.sidescreen.menubar`. It clicks through the Viewer without moving your cursor. It cannot reach the menu bar item.

## Install a local build

`rust/sidescreen/scripts/install.sh` builds a release app, quits any running Sidescreen, installs it as `/Applications/Sidescreen.app` and opens it. Debug builds in `rust/target` don't touch the installed copy. An app started from a terminal borrows the terminal's privacy grants, so a build that works from the shell can still lack its own.
