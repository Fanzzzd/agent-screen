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

Sidescreen started as a Swift app; the Rust app replaced it ([ADR 0005](adr/0005-rust-app-and-mcp.md)), and the Swift code lives on in the history.

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

- **Virtual displays** come from the private `CGVirtualDisplay` API, the same one DeskPad and BetterDisplay use. Each display is 1280 × 800 points by default, at one pixel per point: macOS lists a two-pixel mode, but refuses to switch a virtual display to it (`CGDisplaySetDisplayMode` returns 1001), so the Viewer scales the image up. The app parks it diagonally off the top-right corner of your physical screens so the real cursor rarely wanders onto it. Not a bottom corner: AeroSpace hides other workspaces' windows there, hanging off screen, and they would show on the display.
- **Live frames** come from ScreenCaptureKit as 420f pixel buffers that GPUI draws without a copy. One stream per display runs at the highest frame rate and size any view asks for: 60 fps at full size for the Viewer, 12 fps at 416 px wide for thumbnails.
- **Input** in the panel goes to the target process through SkyLight's `SLEventPostToPid`, stamped with window-routing fields, plus a focus record that tells the target window it has focus without raising it. The recipe is ported from [trycua/cua](https://github.com/trycua/cua) (MIT), minus its defocus record to the front app: that one ends the user's input method session. Your cursor stays put, and your front app keeps its place and its key window.

## Test

```sh
cargo test --manifest-path rust/Cargo.toml   # unit tests
bun rust/tests/mcp/control.ts         # Takeover and Help Request, needs a debug build running
bun rust/tests/mcp/smoke.ts           # end to end with Calculator
bun rust/tests/e2e/run.ts             # the user's side: scrolling, the menu bar strip, focus, permission setup

# Beside your own Sidescreen:
SIDESCREEN_INSTANCE=e2e bun rust/tests/mcp/robust.ts      # what agents get back, and nothing on your screen
SIDESCREEN_INSTANCE=e2e bun rust/tests/e2e/takeover.ts    # Takeover through the Viewer
SIDESCREEN_INSTANCE=e2e bun rust/tests/e2e/timing.ts      # how long create_display and launch_app take
```

`robust.ts` and `timing.ts` launch TextEdit, whose new windows can show on your screen for a frame or two. `robust.ts` waits until you have been idle for 45 s; `E2E_NOW=1` skips the wait.

The table predates 0.1.7, which caches AeroSpace's answer instead of asking it on each `create_display`; a run on 2026-10-12 was discarded because the load average was 81. `timing.ts` on an M-series MacBook with a second monitor and AeroSpace ignoring agent displays, 2026-10-11:

| | median | p90 |
| --- | --- | --- |
| `create_display` | 772 ms | 922 ms |
| `launch_app` TextEdit, not running | 1781 ms | 1947 ms |
| `launch_app` TextEdit, running | 612 ms | 803 ms |

`bun rust/tests/mcp/flash.ts` measures how long a running app's new window shows on your screen before it moves: 29 ms median, 55 ms p90 and 138 ms at most over 30 launches of TextEdit. Pressing the app's menu item, a catch on its own thread, and making the app's window on the display its main window first didn't change it; hiding the app made it worse, 341 ms median. A file that opens after the 4 s turn, forced with `FILES=1 SIDESCREEN_RUNNING_TURN_MS=0` in a debug build, shows longer: 64 to 86 ms median and 189 ms at most over three runs of 15 on 2026-10-12, at load average 15 to 30. A file opened within the turn showed for 0 ms median and 43 ms at most.

Most of a cold launch is TextEdit starting. These are the numbers with AeroSpace ignoring agent displays, which makes Sidescreen skip AeroSpace entirely, as without AeroSpace. Under stock AeroSpace each step also waits for AeroSpace to answer: `create_display` measured 1204 ms median and `launch_app` on a running app 1244 ms before these changes.

With AeroSpace running, a window counts as placed only once AeroSpace files it under the workspace it shows on the display: it can accept a move for a window it hasn't registered yet and then lay the window out in your workspace. `robust.ts` passed 10 of 10 runs in a row after that change; loop it to check a fix for flakes:

```sh
for i in $(seq 10); do SIDESCREEN_INSTANCE=e2e bun rust/tests/mcp/robust.ts | tail -1; done
```

`SIDESCREEN_INSTANCE=e2e` runs the debug app as a second instance with its own socket (`control-e2e.sock`) and display serials, so your Sidescreen and its displays keep running. Those tests send input to app processes, never to your pointer or keyboard. `takeover.ts` also keeps the Viewer off your screen: `debugShow` with the surface `stage` makes one of the test's displays stand in for your screen, and `debugViewerInput` makes clicks and key presses inside the app, through the Viewer's own handler. `E2E_LOG=FILE` keeps the debug app's log.

`rust/tests/e2e` posts real pointer events, so it borrows your cursor. It waits until you have been idle for 45 s with no Agent Display open (`E2E_NOW=1` skips the wait), then puts the cursor, the front app, the AeroSpace workspace and the installed Sidescreen back. Name cases to run some: `scroll`, `strip`, `focus`, `permissions`. `focus` puts a stand-in app with a text field in front and checks that it stays the key window while an agent types: losing it ends the user's input method session. The permission case runs a copy under the bundle id `dev.sidescreen.e2e-ungranted`, so it starts with no grants and never touches yours; what it checks ends where only a person can flip the switch. Captures go to `rust/target/e2e`. `probe tap` in `tests/e2e/tools` records real trackpad events for `hid replay`, which is how `fixtures/trackpad.log` was made.

The scripts start `rust/target/Sidescreen.app/Contents/MacOS/Sidescreen mcp`; `SIDESCREEN_MCP="<binary> [args]"` picks another server. Quit Calculator after `smoke.ts`: it marks the window to keep, and quitting the app would hand it back to your screen.

Debug builds accept `debugTakeover`, `debugShow`, `debugViewerInput` and `debugQuit` on the control socket, so scripts can drive the UI without a real click. `debugShow` takes a `surface`: peek, click, menu, hide, select, viewer, collapse, create, dismiss, answer, primary, windows, help, focus, quit, setup, setup-close, assist-screen, assist-ax, assist-preview, assist-preview-unlisted, assist-hide, connect (runs Connect Claude Code and Codex) or stage, and a `displayId` where one applies. `debugViewer` returns the Viewer's frame and its display image in screen points, and whether the Viewer is the key window. For click-level UI checks, run `codex exec` with Codex's `computer-use` server on the app `dev.sidescreen.menubar`. It clicks through the Viewer without moving your cursor. It cannot reach the menu bar item.

## Install a local build

`rust/sidescreen/scripts/install.sh` builds a release app, quits any running Sidescreen, installs it as `/Applications/Sidescreen.app` and opens it. Debug builds in `rust/target` don't touch the installed copy. An app started from a terminal borrows the terminal's privacy grants, so a build that works from the shell can still lack its own.

## Release

CI (`.github/workflows/ci.yml`) checks formatting, runs clippy and the unit tests on every push and pull request, builds the release app on macOS 26, and starts that build on macOS 15.

Releases follow the changesets model, with [knope](https://knope.tech) (`knope.toml`):

1. With each change users will notice, add a changeset: `knope document-change` asks for the kind (patch, minor or major) and a summary, and writes `.changeset/<name>.md`. Write the summary for users. CI warns when the app changes without one; tests and refactors need none.
2. Every push to `main` with changesets runs `prepare-release.yml`. It keeps one pull request, **Release Sidescreen x.y.z**, open from the `release` branch: the version in the three `Cargo.toml` files and `Cargo.lock`, and a new `CHANGELOG.md` section from the changesets, which it deletes.
3. Merging that pull request runs `release.yml`. It runs the checks, builds and signs the DMG with `release.sh`, has knope tag `v<version>` and publish the GitHub release with the changelog section and the DMG, checks the downloaded DMG against the cask's sha256, and pushes the cask to `Fanzzzd/homebrew-sidescreen`.

Running **Release** from the Actions tab is a dry run: checks, signing and the DMG, kept as an artifact, and nothing published.

Its secrets are the Developer ID certificate (`DEVELOPER_ID_P12_BASE64`, `DEVELOPER_ID_P12_PASSWORD`), `TAP_DEPLOY_KEY`, a write deploy key of the tap, and `RELEASE_DEPLOY_KEY`, a write deploy key of this repository: prepare-release pushes the `release` branch with it, so CI runs on that branch and its checks show on the pull request. Releases aren't notarized. Commit messages stay plain sentences: knope ignores conventional commits here.
