# Developing Agent Screen

## Layout

```
apps/macos/        the app: GPUI UI, control socket, `AgentScreen mcp`
apps/website/      agentscreen.app, a Cloudflare Worker with static assets
crates/mcp/        the MCP server (rmcp), built into the app binary
crates/model/      domain rules with no OS calls: displays, windows, key chords
crates/colorsync/  the root helper that keeps ColorSync's display registry short (ADR 0009)
tests/e2e/         MCP scripts, user-side e2e tests and the README demo recorder
vendor/            Zed's gpui_apple, patched
docs/              ADRs, the AGENTS.md convention, Linux and Windows feasibility notes
```

The root holds the Cargo workspace (`target/` builds there) and a pnpm workspace run by Turborepo:

```sh
pnpm install
pnpm check                  # what CI runs: cargo fmt, clippy and tests
pnpm dev --filter website   # wrangler dev
pnpm run deploy --filter website
```

The promo video and the website films are made in a private repo.

Agent Screen started as a Swift app; the Rust app replaced it ([ADR 0005](adr/0005-rust-app-and-mcp.md)), and the Swift code lives on in the history.

## Build

```sh
apps/macos/scripts/bundle.sh [release]   # build target/Agent Screen Dev.app (release: Agent Screen.app)
apps/macos/scripts/release.sh            # the DMG and Homebrew cask for a release
```

A debug bundle is a separate app, "Agent Screen Dev" (`app.agentscreen.AgentScreen.dev`), so it runs beside an installed Agent Screen without touching it: its own privacy grants, its own control socket (instance `dev` unless `AGENTSCREEN_INSTANCE` says otherwise), and its own agent registration (`agent-screen-dev`). Its menu bar icon is purple.

The Rust toolchain is pinned in `rust-toolchain.toml`. The bundle script signs with your first Apple Development identity. macOS ties the Screen Recording and Accessibility grants to that signature, so the grants survive rebuilds. An ad-hoc signature would lose them every build.

Development switches for the Rust app:

- `AGENTSCREEN_DEBUG=1` enables the debug control methods in a release build, for measuring.
- `AGENTSCREEN_APPEARANCE=light` or `dark` forces the app's appearance in debug builds, for screenshots.

## How it works

- **Virtual displays** come from the private `CGVirtualDisplay` API, the same one DeskPad and BetterDisplay use. Each display is 1280 × 800 points by default, at one pixel per point: macOS lists a two-pixel mode, but refuses to switch a virtual display to it (`CGDisplaySetDisplayMode` returns 1001), so the Viewer scales the image up. The app parks it diagonally off the top-right corner of your physical screens so the real cursor rarely wanders onto it. Not a bottom corner: AeroSpace hides other workspaces' windows there, hanging off screen, and they would show on the display.
- **Display changes are expensive.** Adding, removing or moving a display is a reconfiguration every app on the Mac handles. Each one also makes WindowServer (SkyLight's `CAWSManager::restore_color_preferences`) ask `colorsync.displayservices` for every online display's profile, and wait 50 ms for the answer. A later answer is dropped, and WindowServer asks again 5 s later, until one comes in time; every display change starts another such loop, and no setting takes a display out of the request. For each online display, the service has colorsyncd read and parse its whole registry of displays, `/Library/Caches/ColorSync/com.apple.colorsync.devices`, five times. The registry keeps a record for every display serial macOS has ever seen, each with the display's name in 39 languages, 97% of what gets parsed. So an answer takes about online displays × records × 0.2 ms: with 84 records and four displays online, 66 to 91 ms, and no loop ever ended; they piled up to hundreds of requests a minute, and apps that ask for display information hung (issue #26). A new virtual display also kept sRGB until an answer came in time. Agent Screen gives each display the lowest serial none of its displays holds (`display::Serial`), so its records are as many as the displays it holds at once. ADR 0009's helper, set up once with the user's password and run with `sudo -n`, removes the records of displays not connected that hold only factory data and keeps each record's names in English and the Mac's languages; macOS leaves a trimmed record as it is when its display comes back. The app runs it 3 s after a display is created or released, every 30 minutes and 10 s after launch, and writes its JSON line to `displays.log`. After that, 13 displays online took 19 ms, where 4 took 23 ms with untrimmed records; neither unregistering nor the rewrite starts a request. A late answer still happens under load, at 60 to 130 ms, and its loop ends at the next try. Before a new display, the app estimates the answer's time from the displays online and the records left and refuses `create_display` over 40 ms (`Core::budget_allows`). `AgentScreen colorsync [setup|remove|reclaim]` drives the helper by hand. Agent Screen also keeps its changes few: an ended, empty display stays connected as a spare for the next display of its size, two at most for ten minutes; agents get ten new displays per ten minutes; and a new display is parked once, while a reused one already sits in place. `~/Library/Logs/Agent Screen/displays.log` (`displays-e2e.log` for the test instance) records each display created, kept, reused and released, with its unit number, frame and serial, so a slow Mac can be checked against it.
- **Live frames** come from ScreenCaptureKit as 420f pixel buffers that GPUI draws without a copy. One stream per display runs at the highest frame rate and size any view asks for: 60 fps at full size for the Viewer, 12 fps at 416 px wide for thumbnails. ScreenCaptureKit tells displays apart by unit number, and WindowServer gives a new virtual display the unit 5 plus the number of virtual displays already there, without checking that no display has it. Once virtual displays come and go out of order, a new one can get the unit of AeroSpace's closet, and every stream and screenshot of it then shows the closet, the user's hidden windows. `VirtualDisplay::create` keeps such a display until another one comes, so that one gets the next unit, then lets it go. `screenshot::filter` refuses a display that still shares its unit (`cg::unit_twin`), so neither the Viewer nor an agent gets a picture of it, and each streamed frame's screen rect is checked against the display's bounds: frames of another screen are dropped.
- **Input** in the panel goes to the target process through SkyLight's `SLEventPostToPid`, stamped with window-routing fields, plus a focus record that tells the target window it has focus without raising it. The recipe is ported from [trycua/cua](https://github.com/trycua/cua) (MIT), minus its defocus record to the front app: that one ends the user's input method session. Your cursor stays put, and your front app keeps its place and its key window. Each event carries the time it is sent and its point in the target window, as hardware input does: with no time, Chromium kills a page whose click changes its URL, and AppKit reads the window point only from an event that has a time. A drag's first move goes in steps of a point, then waits 100 ms, since AppKit starts a resize or drag only after a few points and loses a move that jumps past that.

## Test

```sh
cargo test                    # unit tests
cargo e2e control             # Takeover and Help Request, needs a debug build running
cargo e2e smoke [app]         # end to end with Calculator
cargo e2e run                 # the user's side: scrolling, the menu bar strip, focus, permission setup

# Beside your own Agent Screen:
AGENTSCREEN_INSTANCE=e2e cargo e2e robust      # what agents get back, and nothing on your screen
AGENTSCREEN_INSTANCE=e2e cargo e2e takeover    # Takeover through the Viewer
AGENTSCREEN_INSTANCE=e2e cargo e2e viewerdrag  # a title-bar drag in the Viewer stays on the display
AGENTSCREEN_INSTANCE=e2e cargo e2e viewerkey  # a click that makes the Viewer key again, and a paste after it in a Takeover, keep the app running
AGENTSCREEN_INSTANCE=e2e cargo e2e restart    # after the app aborts, the agent hears it quit and can place its windows back (needs Field.app from `ime`)
AGENTSCREEN_INSTANCE=e2e cargo e2e launchhere # an app opened from a launcher while the user works in a Viewer opens on that display (needs Field.app from `ime`)
AGENTSCREEN_INSTANCE=e2e cargo e2e requestwindow # place_window by app asks the user for a window the agent didn't launch; Keep refuses, Hand Over places it as a Handover (needs Field.app from `ime`)
AGENTSCREEN_INSTANCE=e2e cargo e2e placesize  # place_window sets a window's frame on the display and returns the one the app kept, below its minimum and past the display's edge
AGENTSCREEN_INSTANCE=e2e cargo e2e compactstate # get_window_state with max_depth and act_on_element with result changes or none send a fraction of the tree (skips while Calculator runs, quits it after)
AGENTSCREEN_INSTANCE=e2e cargo e2e claims     # the claim file lists the displays and the windows placed on them, drops a window taken back, and is gone after quit; with AeroSpace running, the display gets none of your workspaces or windows (needs Field.app from `ime`)
AGENTSCREEN_INSTANCE=e2e cargo e2e oneclick   # one real click on a field of an Electron login window, through the Viewer, focuses it; counts down on your screen first (needs Electron.app and Field.app)
AGENTSCREEN_INSTANCE=e2e cargo e2e peekhover   # Peek opened by hover closes once the pointer leaves, or at the first click on the icon
AGENTSCREEN_INSTANCE=e2e cargo e2e electron       # an Electron window, from tests/e2e/fixtures/electron: launch with arguments, an accessory app's window that opens on the display, clicks, typing, edge resize, late redraws; needs target/e2e/Electron.app
AGENTSCREEN_INSTANCE=e2e cargo e2e agentshots     # a screenshot is the whole display scaled down; a zoom over all of it works
AGENTSCREEN_INSTANCE=e2e cargo e2e spares         # an ended display comes back as the next one, with no display change
AGENTSCREEN_INSTANCE=e2e cargo e2e warmspare      # while you are idle the app makes two spares, and the next two displays take them with no display change; 3 displays
AGENTSCREEN_INSTANCE=e2e cargo e2e serials        # a new display takes the serial a released one freed
AGENTSCREEN_INSTANCE=e2e cargo e2e viewerpicture  # new displays get unit numbers of their own, and the Viewer shows each
AGENTSCREEN_INSTANCE=e2e cargo e2e shared      # an app you and an agent both use: its keys reach only its window
AGENTSCREEN_INSTANCE=e2e cargo e2e openpanel   # a file Open panel: keys, clicks, window state, picking a file by path
AGENTSCREEN_INSTANCE=e2e cargo e2e browsers    # Safari: the first click, URLs in a running browser, a select menu
AGENTSCREEN_INSTANCE=e2e E2E_NOW=1 cargo e2e settings  # the Settings window opens, fits a 13-inch screen and closes with Done; shows on your screen
AGENTSCREEN_INSTANCE=e2e cargo e2e helpcard    # the Help Card: shows, Later, comes back, steps aside for the Viewer and Takeover; the Viewer lists the request's steps
AGENTSCREEN_INSTANCE=e2e cargo e2e recording   # record_display: the video's length and size, the timeline's actions and offsets, ending the display ends it
AGENTSCREEN_INSTANCE=e2e cargo e2e asksave     # closing a display whose app asks whether to save: the question stays on the display, which goes once the window has closed, and stays after Cancel
AGENTSCREEN_INSTANCE=e2e cargo e2e alerts      # a system alert put on an agent display moves to the main display; it shows on your screen for a moment
AGENTSCREEN_INSTANCE=e2e cargo e2e ime         # type and key through Apple's pinyin give 你好; your front app, input source and workspace stay; waits for you to be idle 3 s
AGENTSCREEN_INSTANCE=e2e cargo e2e timing      # how long create_display and launch_app take
AGENTSCREEN_INSTANCE=e2e cargo e2e pointer     # records the Agent Pointer in the Viewer through moves and quick clicks, frames at 60 fps in target/e2e/pointer; needs ffmpeg
AGENTSCREEN_INSTANCE=e2e cargo e2e load        # with no displays, then LOAD_DISPLAYS (12) showing Chrome pages, or the Electron fixture's when Chrome is yours: CPU of WindowServer, replayd and Agent Screen, how late a plain AppKit app runs, and every agent typing and taking its screenshot at once; LOAD_SECONDS; AGENTSCREEN_DISPLAY_HZ and AGENTSCREEN_DISPLAY_SCALE (1 or 2) set the test build's display mode
AGENTSCREEN_INSTANCE=e2e cargo e2e perf        # what agents and you wait for, over ROUNDS rounds: a new and a reused display, launching Calculator, clicks, screenshots, window states, display changes, the test app's main-thread stalls and how late a plain AppKit app runs meanwhile; JSON in target/e2e/perf. Two agents measuring at once wrap each run in `/usr/bin/lockf /tmp/agent-screen-perf.lock env AGENTSCREEN_INSTANCE=NAME cargo e2e perf`, so runs never overlap
AGENTSCREEN_INSTANCE=e2e cargo e2e changecost     # how long every window on the Mac stops drawing when a display is added and removed, by Core Animation and by a main thread; the test build's AGENTSCREEN_NO_PARK, AGENTSCREEN_MAX_PIXELS=fit, AGENTSCREEN_DISPLAY_SCALE and AGENTSCREEN_DISPLAY_HZ compare choices; 1 display
AGENTSCREEN_INSTANCE=e2e cargo e2e peekanim       # "+" shows a waiting tile at once and grows it in at the display's rate, the new display made once it has grown; a closed display's tile shrinks out the same way; 2 displays
AGENTSCREEN_INSTANCE=e2e cargo e2e peekrapid      # three + in a row with Peek opened by a click: Peek ends as one card filling its window; GAP_MS (1500) between them; 4 displays
AGENTSCREEN_INSTANCE=e2e cargo e2e statusicon     # the menu bar icon's front screen fills while a display is in use and empties once it ends (debugIcon); the pictures come from `apps/macos/assets/status/make.py`; 1 display
AGENTSCREEN_INSTANCE=e2e cargo e2e displaychange  # a display comes and goes while Peek is open: Peek draws at its new size at once, with no draw GPUI skipped (AGENTSCREEN_LOG_ALL=1 logs GPUI's own errors), keeps drawing, and the new display gets its wallpaper; 2 displays
AGENTSCREEN_INSTANCE=e2e cargo e2e chrome      # Chrome on a throwaway profile while Firefox starts on another display: a click focuses a page's field, typing lands in it, a button runs the page's script
AGENTSCREEN_INSTANCE=e2e cargo e2e keys        # the key action in Chrome: a page sees ], }, cmd+[ and cmd+] as those characters on any keyboard type, and the brackets land in the field; 1 display
AGENTSCREEN_INSTANCE=e2e cargo e2e sound       # a recording has the sound of an app on its display (440 Hz, from the Electron fixture) and not of a process elsewhere (2000 Hz); mutes your output while it runs; needs Electron.app
AGENTSCREEN_INSTANCE=e2e cargo e2e stop        # quit a test instance a stopped script left running
```

Test scripts count the displays they make in `/tmp/agent-screen-test-displays`. `E2E_DISPLAYS_PER_HOUR=N` makes `createDisplay` fail in the script past N in an hour; there is no limit by default. About 60 displays in 11 minutes froze this Mac once, so compare builds with few rounds rather than loops.

The scripts live in `tests/e2e/src/bin`, one file each, with the helpers they share in `tests/e2e/src/lib.rs`. `cargo e2e` is an alias in `.cargo/config.toml` for `cargo run --quiet --package agent-screen-tests --bin`. The Swift helpers they call (`probe`, `hid`, `flash`, …) are in `tests/e2e/tools` and build into `target/e2e` on first use.

`robust`, `timing` and `shared` skip when TextEdit is already open, `openpanel` when Preview is, and `browsers` when Safari is, since it may be yours. `robust` and `timing` launch TextEdit, whose new windows can show on your screen for a frame or two. `robust` waits until you have been idle for 45 s; `E2E_NOW=1` skips the wait.

`timing` on an M-series MacBook with a second monitor and AeroSpace ignoring agent displays, two debug-build runs on 2026-10-12 at load average 33 to 50. This Mac rarely drops below 20, so the numbers include that contention:

|                                    | median          | p90             |
| ---------------------------------- | --------------- | --------------- |
| `create_display`                   | 884 to 913 ms   | 1040 to 1317 ms |
| `launch_app` TextEdit, not running | 1035 to 1496 ms | 1237 to 1629 ms |
| `launch_app` TextEdit, running     | 585 to 598 ms   | 673 to 1617 ms  |

`create_display` breaks down in the debug log as: macOS creates the virtual display in 330 to 510 ms; WindowServer accepts its position on the first try, in 3 to 88 ms; the AeroSpace check adds 10 to 30 ms. The table's runs also lost 100 to 400 ms to the main thread, busy with what a new display sets off. Since 0.1.12 Agent Screen makes the display on a background thread, so Peek and the Viewer keep drawing, and parks it right after: the next run, at load 27, gave a 771 ms median and 1093 ms p90, with parking done on the first try every time. macOS's creation time is the floor.

`cargo e2e flash` measures how long a running app's new window shows on your screen before it moves: 29 ms median, 55 ms p90 and 138 ms at most over 30 launches of TextEdit. Pressing the app's menu item, a catch on its own thread, and making the app's window on the display its main window first didn't change it; hiding the app made it worse, 341 ms median. A file that opens after the 4 s turn, forced with `FILES=1 AGENTSCREEN_RUNNING_TURN_MS=0` in a debug build, shows longer: 64 to 86 ms median and 189 ms at most over three runs of 15 on 2026-10-12, at load average 15 to 30. A file opened within the turn showed for 0 ms median and 43 ms at most.

Most of a cold launch is TextEdit starting. These are the numbers with AeroSpace ignoring agent displays, which makes Agent Screen skip AeroSpace entirely, as without AeroSpace. Under stock AeroSpace each step also waits for AeroSpace to answer: `create_display` measured 1204 ms median and `launch_app` on a running app 1244 ms before these changes.

With AeroSpace running, a window counts as placed only once AeroSpace files it under the workspace it shows on the display: it can accept a move for a window it hasn't registered yet and then lay the window out in your workspace. `robust` passed 10 of 10 runs in a row after that change. On 2026-10-12, at load 40 to 80, it passed 9 of 10: in the tenth TextEdit took longer than the cold-start wait and the checks after it failed. Loop it to check a fix for flakes:

```sh
for i in $(seq 10); do AGENTSCREEN_INSTANCE=e2e cargo e2e robust | tail -1; done
```

`AGENTSCREEN_INSTANCE=e2e` runs the debug app as a second instance with its own socket (`control-e2e.sock`) and display serials, so your Agent Screen and its displays keep running. Those tests send input to app processes, never to your pointer or keyboard. `takeover` also keeps the Viewer off your screen: `debugShow` with the surface `stage` makes one of the test's displays stand in for your screen, and `debugViewerInput` makes clicks, key presses and drags (`kind` click, key or drag) inside the app, through the Viewer's own handler. A click with `key` 1 takes the keyboard as the user's click does. A key takes `shift` 1 and `command` 1 for those modifiers. `E2E_LOG=FILE` keeps the debug app's log.

`cargo e2e run` posts real pointer events, so it borrows your cursor. It waits until you have been idle for 45 s with no Agent Display open (`E2E_NOW=1` skips the wait), then puts the cursor, the front app, the AeroSpace workspace and the installed Agent Screen back. Name cases to run some (`cargo e2e run focus`): `scroll`, `strip`, `focus`, `permissions`. `focus` puts a stand-in app with a text field in front and checks that it stays the key window while an agent types: losing it ends the user's input method session. The permission case runs a copy under the bundle id `app.agentscreen.e2e-ungranted`, so it starts with no grants and never touches yours; what it checks ends where only a person can flip the switch. Captures go to `target/e2e`. `probe tap` (`tests/e2e/tools/probe.swift`) records real trackpad events for `hid replay`, which is how `tests/e2e/fixtures/trackpad.log` was made.

The scripts start `target/Agent Screen Dev.app/Contents/MacOS/AgentScreen mcp`; `AGENTSCREEN_MCP="<binary> [args]"` picks another server. Quit Calculator after `smoke`: it marks the window to keep, and quitting the app would hand it back to your screen.

Debug builds accept `debugTakeover`, `debugShow`, `debugIcon`, `debugViewerInput`, `debugMoveWindow` (any window, by `pid` and `windowId`, to a global `x`, `y`), `debugPanic` (a panic on the main thread after it replies, which aborts the app), `debugFront` (macOS last named the app with `pid` the front app), `debugUserKey` (a key of the user's went to the app with `pid`, as the global key monitor reports one), `debugAnswerWindow` (answers the display's Window Request with `give` true or false once one is open, as the card's buttons do) and `debugQuit` on the control socket, so scripts can drive the UI without a real click. `debugShow` takes a `surface`: peek, click, icon-hover and card-hover (the pointer entering, `displayId` 1, or leaving, 0), menu, hide, select, viewer, collapse, create, dismiss, dismiss-confirm (the Dismiss sheet's confirm, with the plan as it stands), answer, primary, give, take-back, help, card-open, card-later, card-take-over, card-give, card-keep (a Window Request's Hand Over and Keep), focus, quit, settings (or setup), setup-close, assist-screen, assist-ax, assist-preview, assist-preview-unlisted, assist-hide, connect (runs Connect Claude Code and Codex), wallpaper (the Open panel behind Settings' Wallpaper row) or stage, and a `displayId` where one applies. `debugIcon` returns which picture the menu bar icon shows (`state`: idle, working, asking or setup). `debugViewer` returns the Viewer's frame and its display image in screen points, whether the Viewer is the key window, and whether its display's stream last showed another screen (`misrouted`). For click-level UI checks, run `codex exec` with Codex's `computer-use` server on the app `app.agentscreen.AgentScreen`. It clicks through the Viewer without moving your cursor. It cannot reach the menu bar item.

## Install a local build

`apps/macos/scripts/install.sh` builds a release app, quits any running Agent Screen, installs it as `/Applications/Agent Screen.app` and opens it. Debug builds in `target` don't touch the installed copy. An app started from a terminal borrows the terminal's privacy grants, so a build that works from the shell can still lack its own.

## Release

CI (`.github/workflows/ci.yml`) checks formatting, runs clippy and the unit tests on every push and pull request, builds the release app on macOS 26, and starts that build on macOS 15.

Releases follow the changesets model, with [knope](https://knope.tech) (`knope.toml`):

1. With each change users will notice, add a changeset: `knope document-change` asks for the kind (patch, minor or major) and a summary, and writes `.changeset/<name>.md`. Write the summary for users. CI warns when the app changes without one; tests and refactors need none.
2. Every push to `main` with changesets runs `prepare-release.yml`. It keeps one pull request, **Release Agent Screen x.y.z**, open from the `release` branch: the version in the three `Cargo.toml` files and `Cargo.lock`, and a new `CHANGELOG.md` section from the changesets, which it deletes.
3. Merging that pull request runs `release.yml`. It runs the checks, builds, signs and notarizes the DMG with `release.sh`, has knope tag `v<version>` and publish the GitHub release with the changelog section and the DMG, checks the downloaded DMG against the cask's sha256, and pushes the cask to `Fanzzzd/homebrew-agent-screen`.

Running **Release** from the Actions tab is a dry run: checks, signing and the DMG, kept as an artifact, and nothing published.

Its secrets are the Developer ID certificate (`DEVELOPER_ID_P12_BASE64`, `DEVELOPER_ID_P12_PASSWORD`), an App Store Connect API key of the same team for notarization (`APP_STORE_CONNECT_KEY_P8`, `APP_STORE_CONNECT_KEY_ID`, `APP_STORE_CONNECT_ISSUER_ID`), `TAP_DEPLOY_KEY`, a write deploy key of the tap, and `RELEASE_DEPLOY_KEY`, a write deploy key of this repository: prepare-release pushes the `release` branch with it, so CI runs on that branch and its checks show on the pull request. To build a notarized DMG locally, store the key once with `xcrun notarytool store-credentials agent-screen` (see the top of `release.sh`). Commit messages stay plain sentences: knope ignores conventional commits here.
