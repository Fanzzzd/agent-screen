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

For build identities, stale MCP clients and evidence to attach to a bug, follow [Reporting bugs](REPORTING_BUGS.md). The build embeds a full source revision when git is available, dirty state, Cargo profile and target. Cargo watches the worktree gitfile, git HEAD/index/refs, tracked files and source directories. Source archives have unknown git metadata. A dirty build never counts as a verified metadata match.
## Viewer click diagnostics

`AGENTSCREEN_VIEWER_CLICK_DIAGNOSTICS=1` enables metadata for mouse down/up
through the Viewer. It is off by default, including in debug builds. The
first stderr line says that it is enabled. The release Logger allows only
this diagnostic target at Info; other release log levels stay unchanged.
The switch is read once at startup. Setting it in a shell cannot enable it
in an app that is already running.

Each JSON record has a `click` id, `phase` (`down` or `up`) and `unix_ms`.
The id follows the click from the Viewer to its input worker. It is unique
within that app run. Stages record these fields:

- `mapping`: the Viewer window frame, content-view height, aspect-fit image
  rect, frame size in pixels, display bounds, event point and mapped global
  point. `window_frame` and `point` use AppKit's bottom-left coordinates;
  `content` uses top-left window points. Display bounds and `global` use
  top-left global CG points. A point outside the image still records
  `accepted=false`, with `global=null` and `display_bounds=null`. Rejection
  keeps the existing behavior and does not query display bounds.
- `hit`: the chosen CG target and host PID/window id, layer and bounds.
  A left press can have two hit records: the title-bar grab check and the
  actual press each use their existing fresh query. No extra query is added.
- `focus`: the result of the existing `AXFocusedWindow` query as a window id.
  `null` means the query or window-id lookup was unavailable. It accepts
  sheets as well as windows. `focus_record` records whether SkyLight accepted
  the existing focus handshake.
- `grab`: whether the press held a window by its title bar.
- `stamp`: the down/up event type, target, global point, target bounds and
  window-local point written into the event. Wake clicks generated during
  the job also appear. `phase` identifies the Viewer job, while `event_type`
  identifies the generated event. Missing bounds retain the existing
  zero-origin fallback.

These records contain no text, AX values, titles, key contents or screenshots.
There is no new tap or global input monitor. Moves, drags, scrolls and keys
have no diagnostic scope. With the switch off, the diagnostic closures do
not run and add no AX or CG queries. Enabled logging adds stderr I/O and
can affect timing. A stamp records preparation for posting; it does not
prove that the target app received the event or executed a button action.
This diagnostic does not fix or reproduce a button action failure.

To retain stderr, start a build containing this change directly from a
terminal. Finder launches normally discard stderr. For a release build
beside the installed app, use a separate instance:

```sh
# Build only. This does not install or start the app.
apps/macos/scripts/bundle.sh release
mkdir -p "$HOME/Library/Logs/Agent Screen"
umask 077
AGENTSCREEN_INSTANCE=e2e AGENTSCREEN_VIEWER_CLICK_DIAGNOSTICS=1 \
  "target/Agent Screen.app/Contents/MacOS/AgentScreen" \
  2> "$HOME/Library/Logs/Agent Screen/viewer-click-diagnostics.log"
```

Do not run a second process for an instance that is already running. The
second process exits without enabling diagnostics in the first one. For a
production capture, first ask the user before quitting the installed app
and starting the new build with the switch and stderr redirection. Quitting
ends its existing Agent Displays. Replacing or restarting the installed app
requires separate user confirmation. Remove the switch at the next launch
to disable diagnostics. stderr also contains ordinary warnings, so review
the file before sharing it. The log file has no automatic rotation.

The offline checks do not create displays, post events or read live AX:

```sh
cargo test -p agent-screen --bin AgentScreen click_diagnostics
cargo test -p agent-screen --bin AgentScreen logger_tests
```

They exercise the real mapping arithmetic, window-local coordinate helper,
hit observer, scoped emission and Logger filter with metadata fixtures.
They do not exercise Xcode or prove that an app runs a button's action.

### Remote-panel coordinate replay

A visible app window can host a panel drawn by another process. Input goes
to the remote window. macOS can omit that off-screen remote window from
`OptionIncludingWindow` while `OptionAll` still reports its geometry. The
input stamper first uses the targeted query. When it misses, it reads the
full list and matches both the target PID and window id. It uses the
current remote frame, never the host frame or a cached frame. Ordinary
`window_bounds` callers retain their targeted-only lookup.

Each stamp with a missing targeted result adds one full-list query. The
fallback filters descriptions to the target process. Runtime cost has not
been measured. If both queries miss, the stamper still has its old
zero-origin behavior. This change does not make that vanished-target path
safe; refusing such input needs a separate change that handles held-button
release too.

The metadata fixture in `apps/macos/tests/fixtures/remote-panel-click.json`
replays a captured Viewer hit and stamp without reading live windows or
posting input:

```sh
cargo test -p agent-screen --bin AgentScreen captured_remote_panel_keeps_window_local_coordinates
cargo test -p agent-screen --bin AgentScreen routing_bounds
```

The first test went red with missing bounds before the fallback, then green
with window-local coordinates. The tests use the actual hit-list matcher,
lookup resolver and stamping writer seam. Other cases cover current frames
after movement, PID mismatch, disappearance and unchanged ordinary routes.
They do not verify private CGEvent setters or an AppKit button action.

Live validation needs an owned Host/Remote fixture with a verified canonical
hardware claim before the Host's first show. Creating a window at virtual
display coordinates alone does not prevent AeroSpace reclaiming it. No live
fixture is included or run until that pre-show containment is available.

## How it works

- **Virtual displays** come from the private `CGVirtualDisplay` API, the same one DeskPad and BetterDisplay use. Each display is 1280 × 800 points by default, at one pixel per point: macOS lists a two-pixel mode, but refuses to switch a virtual display to it (`CGDisplaySetDisplayMode` returns 1001), so the Viewer scales the image up. The app parks its displays in a compact grid off the top-right corner of your physical screens (`agent_screen_model::arrangement`): the first touches them only at that corner, so the real cursor rarely wanders onto it, and the rest fill rows no wider than your screens, in serial order. Your screens stay where you put them, and System Settings shows them at a usable size. Not a bottom corner: AeroSpace hides other workspaces' windows there, hanging off screen, and they would show on the display.
- **Display changes are expensive.** Adding, removing or moving a display is a reconfiguration every app on the Mac handles. Each one also makes WindowServer (SkyLight's `CAWSManager::restore_color_preferences`) ask `colorsync.displayservices` for every online display's profile, and wait 50 ms for the answer. A later answer is dropped, and WindowServer asks again 5 s later, until one comes in time; every display change starts another such loop, and no setting takes a display out of the request. For each online display, the service has colorsyncd read and parse its whole registry of displays, `/Library/Caches/ColorSync/com.apple.colorsync.devices`, five times. The registry keeps a record for every display serial macOS has ever seen, each with the display's name in 39 languages, 97% of what gets parsed. So an answer takes about online displays × records × 0.2 ms: with 84 records and four displays online, 66 to 91 ms, and no loop ever ended; they piled up to hundreds of requests a minute, and apps that ask for display information hung (issue #26). A new virtual display also kept sRGB until an answer came in time. Agent Screen gives each display the lowest serial none of its displays holds (`display::Serial`), so its records are as many as the displays it holds at once. ADR 0009's helper, set up once with the user's password and run with `sudo -n`, removes the records of displays not connected that hold only factory data and keeps each record's names in English and the Mac's languages; macOS leaves a trimmed record as it is when its display comes back. The app runs it 3 s after a display is created or released, every 30 minutes and 10 s after launch, and writes its JSON line to `displays.log`. After that, 13 displays online took 19 ms, where 4 took 23 ms with untrimmed records; neither unregistering nor the rewrite starts a request. A late answer still happens under load, at 60 to 130 ms, and its loop ends at the next try. Before a new display, the app estimates the answer's time from the displays online and the records left and refuses `create_display` over 40 ms (`Core::budget_allows`). `AgentScreen colorsync [setup|remove|reclaim]` drives the helper by hand. Agent Screen also keeps its changes few: an ended, empty display stays connected as a spare for the next display of its size, two at most for ten minutes; agents get ten new displays per ten minutes; and a new display is parked once, while a reused one already sits in place. `~/Library/Logs/Agent Screen/displays.log` (`displays-e2e.log` for the test instance) records each display created, kept, reused and released, with its unit number, frame and serial, so a slow Mac can be checked against it.
- **Live frames** come from ScreenCaptureKit as 420f pixel buffers that GPUI draws without a copy. One stream per display runs at the highest frame rate and size any view asks for: 60 fps at full size for the Viewer, 12 fps at 416 px wide for thumbnails. ScreenCaptureKit tells displays apart by unit number, and WindowServer gives a new virtual display the unit 5 plus the number of virtual displays already there, without checking that no display has it. Once virtual displays come and go out of order, a new one can get the unit of AeroSpace's closet, and every stream and screenshot of it then shows the closet, the user's hidden windows. `VirtualDisplay::create` keeps such a display until another one comes, so that one gets the next unit, then lets it go. `screenshot::filter` refuses a display that still shares its unit (`cg::unit_twin`), so neither the Viewer nor an agent gets a picture of it, and each streamed frame's screen rect is checked against the display's bounds: frames of another screen are dropped.
- **Input** in the panel goes to the target process through SkyLight's `SLEventPostToPid`, stamped with window-routing fields, plus a focus record that tells the target window it has focus without raising it. The recipe is ported from [trycua/cua](https://github.com/trycua/cua) (MIT), minus its defocus record to the front app: that one ends the user's input method session. Your cursor stays put, and your front app keeps its place and its key window. Each event carries the time it is sent and its point in the target window, as hardware input does: with no time, Chromium kills a page whose click changes its URL, and AppKit reads the window point only from an event that has a time. A drag's first move goes in steps of a point, then waits 100 ms, since AppKit starts a resize or drag only after a few points and loses a move that jumps past that.

## Test

The frame-placement regression injects the actual AX writer callback to check reserved-strip constraints without touching a window. The scroll regression constructs synthetic CGEvents through the production constructor and reads their fields. It does not post them or inspect a browser. These tests do not verify Dia's real zoom, focus or virtualized-list behavior.

```sh
cargo test                    # unit and isolated integration tests
cargo test -p agent-screen-mcp --test diagnostics # CLI/MCP over an isolated endpoint; no app, display or input
cargo e2e control             # Takeover and Help Request, needs a debug build running
cargo e2e smoke [app]         # end to end with Calculator
cargo e2e run                 # the user's side: scrolling, the menu bar strip, focus, permission setup

# Beside your own Agent Screen:
AGENTSCREEN_INSTANCE=e2e cargo e2e robust      # what agents get back, and nothing on your screen
AGENTSCREEN_INSTANCE=e2e cargo e2e takeover    # Takeover through the Viewer
AGENTSCREEN_INSTANCE=e2e cargo e2e viewerdrag  # a title-bar drag in the Viewer stays on the display
AGENTSCREEN_INSTANCE=e2e cargo e2e viewerkey  # a click that makes the Viewer key again, and a paste after it in a Takeover, keep the app running
AGENTSCREEN_INSTANCE=e2e cargo e2e restart    # after the app aborts, the agent hears it quit and can place its windows back (needs Field.app from `ime`)
AGENTSCREEN_INSTANCE=e2e cargo e2e lastapp    # the only app on a display quits, and the display stays: a screenshot and the next launch_app on it work (needs Field.app from `ime`)
AGENTSCREEN_INSTANCE=e2e cargo e2e launchhere # an app opened from a launcher while the user works in a Viewer opens on that display (needs Field.app from `ime`)
AGENTSCREEN_INSTANCE=e2e cargo e2e reopened   # a window an app the agent started opens by itself, long after the agent's last action, while its other windows are on the display: it opens there too; 40 s (needs Field.app from `ime`)
AGENTSCREEN_INSTANCE=e2e cargo e2e requestwindow # place_window by app asks for a window started by a separate stage session; Keep leaves it on the stage, Hand Over places it as a Handover; 2 displays, nothing on your screen
AGENTSCREEN_INSTANCE=e2e cargo e2e appwhere   # an app brought to the front with its windows only on an Agent Display gets a notification naming it, once a minute; one with a window on your screen, or one the agent started that brings itself forward, gets none (needs Field.app from `ime`)
AGENTSCREEN_INSTANCE=e2e cargo e2e requestcontrol # request_control during a Takeover, from an agent with a long name: the card shows with both buttons inside it (debugCard), Keep Control tells the waiting agent and leaves the Takeover, Let … Continue hands the display back, a withdrawal leaves the Takeover, and without a Takeover it answers at once
AGENTSCREEN_INSTANCE=e2e cargo e2e placesize  # place_window sets a window's frame on the display and returns the one the app kept, below its minimum and past the display's edge
AGENTSCREEN_INSTANCE=e2e cargo e2e placeheld  # Field moves to a user stand-in stage; place_window takes it as a stray, or fails when it keeps moving back; 2 displays, nothing on your screen
AGENTSCREEN_INSTANCE=e2e cargo e2e compactstate # get_window_state with max_depth and act_on_element with result changes or none send a fraction of the tree (skips while Calculator runs, quits it after)
AGENTSCREEN_INSTANCE=e2e cargo e2e claims     # the claim file lists the displays and the windows placed on them, drops a window taken back, and is gone after quit; with AeroSpace running, the display gets none of your workspaces or windows (needs Field.app from `ime`)
AGENTSCREEN_INSTANCE=e2e cargo e2e nativeclick # one click through the Viewer's handler on a field of an inactive native login window focuses it, after a click in another app, in the app's other window, and with the app in front
AGENTSCREEN_INSTANCE=e2e cargo e2e oneclick   # one real click on a field of an Electron login window, through the Viewer, focuses it; its sign shows each of the three steps (needs Electron.app and Field.app)
AGENTSCREEN_INSTANCE=e2e cargo e2e peekhover   # hover opens a preview; the first icon click pins it, a later click closes it; pointer departure closes only an unpinned preview
AGENTSCREEN_INSTANCE=e2e cargo e2e electron       # an Electron window, from tests/e2e/fixtures/electron: launch with arguments, an accessory app's window that opens on the display, clicks, typing, edge resize, late redraws, a window larger than the display, and one its app grows past the display or moves to the main screen after it shows; needs target/e2e/Electron.app
AGENTSCREEN_INSTANCE=e2e cargo e2e launchagain    # launch_app with arguments on an Electron app that allows one instance and already runs on the display answers with its window, not "did not start"; needs target/e2e/Electron.app
AGENTSCREEN_INSTANCE=e2e cargo e2e menupress      # act_on_element press on an Electron menu item reaches it, as Electron's shortcut, and answers with a note saying so; needs target/e2e/Electron.app
AGENTSCREEN_INSTANCE=e2e cargo e2e holdmode       # a display macOS switches to 800×600 by itself is back at the size it was made with, and the display log says so; 1 display
AGENTSCREEN_INSTANCE=e2e cargo e2e secondwindow   # an Electron app on the display that brings itself to the front to show a window, at launch and for a window.open: your front app is back within 300 ms; shows the sign first; needs target/e2e/Electron.app
AGENTSCREEN_INSTANCE=e2e cargo e2e samebundle     # launch_app with arguments takes the instance it started, not a copy of the same Electron.app started from a shell meanwhile; needs target/e2e/Electron.app and a quit e2e app
AGENTSCREEN_INSTANCE=e2e cargo e2e agentshots     # a screenshot is the whole display scaled down; a zoom over all of it works
AGENTSCREEN_INSTANCE=e2e cargo e2e spares         # an ended display comes back as the next one, with no display change, and the display log records the app start and each display released at quit
AGENTSCREEN_INSTANCE=e2e cargo e2e barleak        # screenshots right after the Electron harness quits never show your menu bar's status items; needs target/e2e/Electron.app; 1 display
AGENTSCREEN_INSTANCE=e2e cargo e2e spares         # an ended display comes back as the next one, with no display change
AGENTSCREEN_INSTANCE=e2e cargo e2e warmspare      # while you are idle the app makes two spares, and the next two displays take them with no display change; 3 displays
AGENTSCREEN_INSTANCE=e2e cargo e2e serials        # a new display takes the serial a released one freed
AGENTSCREEN_INSTANCE=e2e cargo e2e arrangement    # new displays park in a grid off the top-right corner of your screens, beside each other while the row fits; nothing already there moves; 2 displays
AGENTSCREEN_INSTANCE=e2e cargo e2e viewerpicture  # new displays get unit numbers of their own, and the Viewer shows each
AGENTSCREEN_INSTANCE=e2e cargo e2e shared      # an app you and an agent both use: its keys reach only its window
AGENTSCREEN_INSTANCE=e2e cargo e2e openpanel   # a file Open panel: keys, clicks, window state, picking a file by path
AGENTSCREEN_INSTANCE=e2e cargo e2e browsers    # Safari: the first click, URLs in a running browser, a select menu
AGENTSCREEN_INSTANCE=e2e E2E_NOW=1 cargo e2e settings  # the Settings window opens, fits a 13-inch screen and closes with Done; shows on your screen
AGENTSCREEN_INSTANCE=e2e cargo e2e helpcard    # the Help Card: shows, Later, comes back, steps aside for the Viewer and Takeover; the Viewer lists the request's steps; captures a Window Request's card for a short and a long app name, per AGENTSCREEN_LANG
AGENTSCREEN_INSTANCE=e2e cargo e2e viewerheader  # the Viewer at its narrowest, on a 640×1600 display: the header's buttons fit for an agent, a Help Request and a Takeover; captures each, per AGENTSCREEN_LANG; 1 display
AGENTSCREEN_INSTANCE=e2e cargo e2e recording   # record_display: the video's length and size, the timeline's actions and offsets, ending the display ends it
AGENTSCREEN_INSTANCE=e2e cargo e2e asksave     # closing a display whose app asks whether to save: the question stays on the display, which goes once the window has closed, and stays after Cancel
AGENTSCREEN_INSTANCE=e2e cargo e2e alerts      # a system alert put on an agent display moves to the main display: a UserNotificationCenter alert, then a stand-in for FollowUpUI's Apple Account sign-in alert; counts down on your screen first, where each shows for a moment
AGENTSCREEN_INSTANCE=e2e cargo e2e ime         # type and key through Apple's pinyin give 你好; your front app, input source and workspace stay; waits for you to be idle 3 s
AGENTSCREEN_INSTANCE=e2e cargo e2e timing      # how long create_display and launch_app take
AGENTSCREEN_INSTANCE=e2e cargo e2e pointer     # records the Agent Pointer in the Viewer through moves and quick clicks, frames at 60 fps in target/e2e/pointer; needs ffmpeg
AGENTSCREEN_INSTANCE=e2e cargo e2e load        # with no displays, then LOAD_DISPLAYS (12) showing Chrome pages, or the Electron fixture's when Chrome is yours: CPU of WindowServer, replayd and Agent Screen, how late a plain AppKit app runs, and every agent typing and taking its screenshot at once; LOAD_SECONDS; AGENTSCREEN_DISPLAY_HZ and AGENTSCREEN_DISPLAY_SCALE (1 or 2) set the test build's display mode
AGENTSCREEN_INSTANCE=e2e cargo e2e perf        # what agents and you wait for, over ROUNDS rounds: a new and a reused display, launching Calculator, clicks, screenshots, window states, display changes, the test app's main-thread stalls and how late a plain AppKit app runs meanwhile; JSON in target/e2e/perf. Two agents measuring at once wrap each run in `/usr/bin/lockf /tmp/agent-screen-perf.lock env AGENTSCREEN_INSTANCE=NAME cargo e2e perf`, so runs never overlap
AGENTSCREEN_INSTANCE=e2e cargo e2e changecost     # how long every window on the Mac stops drawing when a display is added and removed, by Core Animation and by a main thread; the test build's AGENTSCREEN_NO_PARK, AGENTSCREEN_MAX_PIXELS=fit, AGENTSCREEN_DISPLAY_SCALE and AGENTSCREEN_DISPLAY_HZ compare choices; 1 display
AGENTSCREEN_INSTANCE=e2e cargo e2e peekanim       # "+" shows a waiting tile at once and grows it in at the display's rate, the new display made once it has grown; a closed display's tile shrinks out the same way; neither tile's animation starts over when the card stops sliding, and the stage clip of Peek is kept; 2 displays
AGENTSCREEN_INSTANCE=e2e cargo e2e peekrapid      # three + in a row with Peek opened by a click: Peek ends as one card filling its window; GAP_MS (1500) between them; 4 displays
AGENTSCREEN_INSTANCE=e2e cargo e2e statusicon     # the menu bar icon's front screen fills while a display is in use and empties once it ends (debugIcon); the pictures come from `apps/macos/assets/status/make.py`; 1 display
AGENTSCREEN_INSTANCE=e2e cargo e2e displaychange  # a display comes and goes while Peek is open: Peek draws at its new size at once, with no draw GPUI skipped (AGENTSCREEN_LOG_ALL=1 logs GPUI's own errors), keeps drawing, and the new display gets its wallpaper; 2 displays
AGENTSCREEN_INSTANCE=e2e cargo e2e chrome      # Chrome on a throwaway profile while Firefox starts on another display: a click focuses a page's field, typing lands in it, a button runs the page's script
AGENTSCREEN_INSTANCE=e2e cargo e2e menuclosed  # a select menu in Chrome on a throwaway profile: typing into the open menu chooses its item; once the menu has closed, as at your click or key anywhere, the next click or key fails and says so; 1 display
AGENTSCREEN_INSTANCE=e2e cargo e2e typedrops   # typing into address fields in Chrome on a throwaway profile: a busy page and a controlled field get every character; a page that drops fast keys makes the type action fail and say what the field holds; 1 display
AGENTSCREEN_INSTANCE=e2e cargo e2e keys        # the key action in Chrome: a page sees ], }, cmd+[ and cmd+] as those characters on any keyboard type, and the brackets land in the field; 1 display
AGENTSCREEN_INSTANCE=e2e cargo e2e webdrag     # drags in web pages go through the DevTools pipe launch_app gives Chromium apps (ADR 0010): the Electron harness's splitter, which follows pointermove only while buttons holds the left button, and an HTML5 drop; the same harness started with open fails with a message and sends nothing; Chrome on a throwaway profile (left out while Chrome runs); the session's left button never reads down; needs target/e2e/Electron.app; 1 display
AGENTSCREEN_INSTANCE=e2e cargo e2e sound       # a recording has the sound of an app on its display (440 Hz, from the Electron fixture) and not of a process elsewhere (2000 Hz); mutes your output while it runs; needs Electron.app
AGENTSCREEN_INSTANCE=e2e cargo e2e stop        # quit a test instance a stopped script left running
```

Test scripts count the displays they make in `/tmp/agent-screen-test-displays`. `E2E_DISPLAYS_PER_HOUR=N` makes `createDisplay` fail in the script past N in an hour; there is no limit by default. About 60 displays in 11 minutes froze this Mac once, so compare builds with few rounds rather than loops.

The scripts live in `tests/e2e/src/bin`, one file each, with the helpers they share in `tests/e2e/src/lib.rs`. `cargo e2e` is an alias in `.cargo/config.toml` for `cargo run --quiet --package agent-screen-tests --bin`. The Swift helpers they call (`probe`, `hid`, `flash`, …) are in `tests/e2e/tools` and build into `target/e2e` on first use.

Every script shows a sign on your main screen (`tests/e2e/tools/overlay.swift`, started by `run` in `lib.rs`). It counts down from 3 in the middle with the script's name: in orange with "don't touch the mouse or keyboard / 请勿操作鼠标和键盘" for scripts that move your pointer, take your keyboard or input source, mute your sound or put windows, alerts or focus on your screen (the `HANDS_OFF` list in `lib.rs`), in blue with "Agent Screen e2e running / 测试运行中" for the rest. Then it shrinks to a badge in the bottom-right corner that shows the script's name and each `check` label as it passes (✓) or fails (✗), and any step a script names with `sign_step`. At the end it shows the result: green "全部通过 / all passed" with the count of checks, red "N 项失败 / N failed" with the first failing label, or grey "跳过 / skipped" with the reason, and "可以操作了 / you can use the Mac" below. It goes 3 s later, 6 s after a failure. An error, a panic and `skip()` end it the same way. It is a non-activating panel of an accessory process that ignores the mouse, so it never takes focus or a click, and it is one per process however many threads a script runs. `capture` and `hid hover` take it off the screen while they capture, so pictures of the main screen never show it. `E2E_NO_SIGN=1` turns it off, for batch runs.

`run` also watches your screen for the whole script (`tests/e2e/tools/sighting.swift`). Every 10 ms it lists the windows on your displays, every display but Agent Displays and AeroSpace's closet, and keeps those of the test's processes: the script's descendants, anything whose executable or command line is in this checkout (`target/e2e` tools and apps, fixtures, throwaway browser profiles), and any app that started after the script did, outside `/System/Library` and `/usr` and other checkouts. The sign and the test app's own windows (Peek, cards, the Viewer, Settings) don't count. The script ends with the check "nothing the test started showed on your screen", which lists each window with its owner, title, frame, when it showed and for how long, and fails for any of them. `target/e2e/sightings-<script>.jsonl` keeps the list. A script may allow an owner's windows for some milliseconds with `allow_on_your_screen(owner, ms, why)`, only for a window macOS shows before Agent Screen can move it or one the script puts there on purpose, and says why in the call. `E2E_NO_SIGHTING=1` turns the watch off.

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

### Recording playback and pointer timelines

The menu bar icon's Recordings… entry opens a normal GPUI window. It lists
local MP4 files in `~/Movies/Agent Screen`; Refresh reloads the list.
Directory discovery and sidecar parsing run off the UI thread. Selection
and refresh generations discard stale asynchronous results. The list is
limited to 2,000 videos. Hidden files, directories and symlinks are excluded.
No sidecar can choose a media URL or execute an action.

`mac::playback::Player` owns one AVPlayer and AVPlayerItemVideoOutput. It
returns IOSurface-backed 420f CVPixelBuffers for GPUI's `surface` element.
AVPlayer supplies the media time for the image and pointer sample. It also
owns audio playback. Pause, forward/backward seek and repeated seeks use
that same player. Seek completions carry a generation; an old completion
cannot release a newer seek. No frame is exposed while a seek is pending.
Show Pointer only changes overlay visibility. It does not recreate the
player, change rate, seek, encode video or send input to any app.

`ui::pointer::playback_layer` reuses the current arrow drawing. Its position,
click press and ripple are pure samples of media time. It has no live
DisplayId, shared live-pointer state or Instant-based animation. This keeps
pause, reverse seeks and toggles deterministic and leaves the style
replaceable. No system cursor replacement is involved.

New recordings keep the MP4 and the same-stem `.timeline.jsonl` file.
The sidecar begins with an `event: recording` header at schema version 1:

- `logicalSize`, `videoSize`, `videoScale` and `shotScale` describe points,
  video pixels and the screenshot-to-points conversion.
- `coordinateSpace: logical` and `pointerClock: recorded_duration` identify
  the pointer units and clock. Resolved points are recorded from the
  existing agent pointer path on that display, before gesture forwarding.
  They annotate the agent's requested point, not proof of input delivery or
  a button action. Their `t` comes from SCRecordingOutput's recorded media
  duration, not the tool call's completion or a playback wall clock.
- Pointer records contain `event`, `actor`, `visible`, `t`, `x`, `y` and
  `click`. `x` and `y` are recording-relative logical points. A current
  display-size change maps through normalized coordinates. The additional
  pointer records contain no text, keys or account fields. Existing action
  records retain their prior fields; they never drive pointer playback.
- `userPointerRecorded: false` means Takeover hides the unknown user pointer.
  A hidden pointer event marks its start. The producer checks the current
  Core display/Takeover state at each point, not a startup snapshot. Handing
  back control does not
  resurrect the previous agent pointer; the next agent point makes it visible.
- `completeAtEnd: true` requires a final `recordingEnd` with
  `pointerTimelineComplete: true`. Missing or failed completion disables the
  pointer, including a truncated file that ends on a valid JSON boundary.

Pointer/action writes use one buffered file per recording. No pointer write
occurs before the recording clock starts, after removal from the active
recordings map, or for an unrecorded display. New sidecars are owner-only and
use a unique recording stem. The reader caps each sidecar at 16 MiB and
100,000 event rows plus its header. Sidecars must be regular files; no-follow,
nonblocking open plus descriptor validation rejects links, FIFOs and devices.
It parses lines independently; any
malformed row, invalid coordinate, incompatible schema or limit violation
makes the pointer unavailable. Video playback remains independent. Old
unversioned JSONL stays readable as legacy data, but lacks reliable pointer
units and timing, so no coordinates are guessed.

The recorder asks ScreenCaptureKit for available codecs and MP4 support.
It requests HEVC when advertised, otherwise H.264 when available. Unsupported
configuration/start failures remain errors; it does not silently restart an
active recording or fall back on a permission/disk error. `recordStart` and
`recordStop` report `selectedCodec` (`hvc1` or `avc1`) and
`hardwareEncoderVerified: false`. The header records the same selection.
This is a requested codec, not proof of the encoder implementation. The
frame interval remains explicitly 1/30. Audio filtering and the passthrough
join remain unchanged. See [the codec research](research/efficient-screen-recording.md)
for hardware API limits and the bounded synthetic capability experiment.
No same-quality space or CPU improvement is claimed for ScreenCaptureKit.

Offline checks use synthetic sidecars and temporary files, not the user's
recordings:

```sh
cargo test -p agent-screen-model recording_playback
cargo test -p agent-screen --bin AgentScreen mac::playback
```

`apps/macos/tests/PlaybackProbe.m` is a headless test of the production
`native/Playback.m` bridge. It creates no NSApplication, window, display,
capture or input event. It checks a paused poster frame, 420f/IOSurface
buffers, native media-time advance, pause, forward/backward and rapid seeks,
end seeking and replay. For a generated silent H.264 fixture, from the repo
root:

```sh
media=$(mktemp -d)
helper=$(mktemp -d)
ffmpeg -hide_banner -loglevel error -f lavfi \
  -i testsrc2=size=640x400:rate=30:duration=2 -an \
  -c:v libx264 -pix_fmt yuv420p -movflags +faststart "$media/synthetic.mp4"
clang -fobjc-arc -framework Foundation -framework AVFoundation \
  -framework CoreMedia -framework CoreVideo \
  apps/macos/native/Playback.m apps/macos/tests/PlaybackProbe.m \
  -o "$helper/playback-probe"
"$helper/playback-probe" "$media/synthetic.mp4" 2
rm -rf "$media" "$helper"
```

The optional final argument is the fixture's expected duration in seconds.
Only use owned, silent synthetic fixtures for this probe. The H.264 and HEVC
10-second fixtures from the codec capability experiment also exercise this
bridge. Their decode/seek results do not prove ScreenCaptureKit encoding,
audio continuity or the GPUI controls' behavior.

GUI acceptance still needs separate permission. Check menu/open/close,
empty and populated lists, asynchronous selection, small/portrait video,
light/dark appearance, play/pause and dragging seek in both directions.
Toggle the pointer during video with sound and confirm no playhead or audio
interruption. Check old, missing, damaged and oversized sidecars, then a new
recording's media alignment and Takeover boundary. No GUI test, capture,
installation or restart is part of the offline commands above.
### Peek intent checks without UI

```sh
CARGO_INCREMENTAL=0 cargo test -p agent-screen-model peek::tests
CARGO_INCREMENTAL=0 cargo test -p agent-screen peek_shell
CARGO_INCREMENTAL=0 cargo test -p agent-screen status_button_down_and_up
```

The model uses a virtual monotonic clock. Native event values drive the same shell policy seams used by the status button, mouse monitor and hover callbacks. These checks do not construct a window or install an input monitor. Left-button down captures the current open/pin/close intent and invalidates hover timers; only a valid up commits it once. Keyboard, accessibility and `debugShow click` use logical activation without synthesizing a press.

The initial dwell and leave constants are 300 ms and 500 ms. The [research notes](research/hover-click-intent.md) distinguish NN/g's web-content recommendations from Apple's APIs and macOS app source. These are candidate values, not Apple thresholds or measured Peek latency. Pointer sampling runs every 100 ms to repair missed exit notifications, so actual dismissal can add detection, scheduling and animation time. Native event ordering, visual transitions and user preference still need separately authorized GUI checks.

### Debug control methods

The control socket accepts these methods in debug builds. `AGENTSCREEN_DEBUG=1` enables them in release builds too, except for `userStage`. Scripts can drive the UI without a real click.

| Method | Arguments and result |
| --- | --- |
| `debugTakeover` | `displayId` and `active` begin or end a Takeover. |
| `debugShow` | `surface` selects an action from the list below. Supply `displayId` where the action needs one. |
| `debugViewerInput` | Makes a click, key press or drag through the Viewer's own handler. See the input parameters above. |
| `debugMoveWindow` | Moves the window named by `pid` and `windowId` to global `x`, `y`. |
| `debugHandOver` | Hands `windowId` over to `displayId`, as from the Windows menu. |
| `debugTakeBack` | Takes `windowId` back from `displayId`, as from the Windows menu. |
| `debugAnswerWindow` | Answers the display's Window Request with `give` true or false once it opens. Returns `answered`. |
| `debugFront` | Records that macOS last named `pid` the front app. It does not activate the app. |
| `debugUserKey` | Records a user key press to `pid`, as the global key monitor does. |
| `debugActivated` | Simulates the user bringing `pid` to the front for the window-location reminder. Returns the displays it named as `reminded`. |
| `debugAgentReady` | Returns the agent the setup window says loaded the tools, as `agent`. |
| `debugCard` | Returns the Help Card's `width` and each button's `[x, y, w, h]` from the card's corner at its last paint. `buttons` maps button ids to bounds. |
| `debugIcon` | Returns the menu bar icon's `state`: idle, working, asking or setup. |
| `debugViewer` | Returns `frame` and `image` in screen points, `key` for whether the Viewer is the key window, `takeBack` for the header button's bounds in points from the Viewer's corner, and `misrouted` for whether its stream showed another screen. |
| `debugPanic` | Panics on the main thread after replying. This aborts the test app. |
| `debugQuit` | Quits the test app after replying. |

`debugShow` accepts these surfaces:

| Surfaces | Action |
| --- | --- |
| `peek`, `click`, `menu`, `hide` | Open Peek held, click the menu bar icon, open its menu, or hide Peek. |
| `icon-hover`, `card-hover` | Simulate pointer entry with `displayId` 1, or exit with 0. |
| `select`, `viewer`, `collapse`, `focus` | Select a display, open or close its Viewer, or take the keyboard. |
| `create`, `dismiss`, `dismiss-confirm` | Create a display, open Dismissal, or confirm the Dismiss sheet's current plan. |
| `primary`, `give`, `take-back`, `help` | Invoke the display's primary action, open its Give or Take Back menu, or open it for help. |
| `answer` | Confirm the Viewer's Dismiss sheet with `displayId` nonzero, or cancel it with 0. |
| `card-open`, `card-later`, `card-take-over` | Open the Help Card's display, put the card off, or take over its display. |
| `card-give`, `card-keep` | Answer a Window Request with Hand Over or Keep. |
| `card-continue`, `card-keep-control` | Answer a Control Request with Let It Continue or Keep Control. |
| `window-request` | Open a Window Request for no real window, with `app` and `title` for its card. `card-keep` ends it. |
| `recordings`, `recordings-close` | Open or close the independent recording library/player. No display is required. A debug build can use `AGENTSCREEN_PLAYBACK_DIR` for owned synthetic MP4/sidecar fixtures instead of the user's recording directory. |
| `settings`, `setup`, `setup-close`, `setup-waiting` | Open Settings, close it, or preview its waiting state with both permissions missing. `settings` and `setup` are aliases. |
| `assist-screen`, `assist-ax` | Open the Screen Recording or Accessibility pane and its helper. |
| `assist-preview`, `assist-preview-unlisted`, `assist-hide` | Show the helper preview, preview an app absent from the permission list, or hide the helper. |
| `connect`, `wallpaper`, `quit` | Run Connect Claude Code and Codex, open Settings' wallpaper picker, or quit. |
| `stage` | Select the test display that stands in for the user's screen. `displayId` 0 clears the selection. |
| `userStage` | Supply the selected stage's user-display role to window ownership rules. Only debug-assertion builds compile this surface. |

`userStage` requires a selected, live stage owned by the same connection. Layout, capture and the independent sighting watcher keep the real hardware identity. Ending or releasing the stage, closing its connection, or changing the selected stage clears the role. Release builds contain no role override, even with `AGENTSCREEN_DEBUG=1`.

For click-level UI checks, run `codex exec` with Codex's `computer-use` server on the app `app.agentscreen.AgentScreen`. It clicks through the Viewer without moving your cursor. It cannot reach the menu bar item.

## Agent Pointer geometry preview

`ui/pointer.rs` remains the shared drawing entry for the Viewer and Peek. Its original cubic controls, stroke sizes, hotspot transform and aspect-fit geometry live in `crates/model/src/pointer_geometry.rs`. They add no clock or per-display state. The existing pointer animation remains in the shared drawing entry.

```sh
python3 -I apps/macos/scripts/preview-pointer.py --output /tmp/agent-screen-rounded-pointer-preview
cargo test -p agent-screen-model pointer_geometry::tests
```

The preview generator parses the production Rust control points and stroke constants directly. It writes `index.html` and `preview.svg` locally, records the source hash, and includes 18/24/32 logical-pixel canvases, light/dark content, clipping examples and preview-only hotspot guides. It imports no third-party cursor paths. Its sample accent and SVG paint are not a GPUI screenshot or an anti-aliasing test. Opening it in Orca needs no Agent Display or app replacement.

## Install a local build

`apps/macos/scripts/install.sh` builds a release app, quits any running Agent Screen, installs it as `/Applications/Agent Screen.app` and opens it. Debug builds in `target` don't touch the installed copy. An app started from a terminal borrows the terminal's privacy grants, so a build that works from the shell can still lack its own.

## Release

CI (`.github/workflows/ci.yml`) checks formatting, runs clippy and the unit tests on every push and pull request, builds the release app on macOS 26, and starts that build on macOS 15.

Releases follow the changesets model, with [knope](https://knope.tech) (`knope.toml`):

1. With each change users will notice, add a changeset: `knope document-change` asks for the kind (patch, minor or major) and a summary, and writes `.changeset/<name>.md`. Write the summary for users. CI warns when the app changes without one; tests and refactors need none.
2. Every push to `main` with changesets runs `prepare-release.yml`. It keeps one pull request, **chore(release): Agent Screen x.y.z**, open from the `release` branch: the version in the three `Cargo.toml` files and `Cargo.lock`, and a new `CHANGELOG.md` section from the changesets, which it deletes.
3. Merging that pull request runs `release.yml`. It runs the checks, builds, signs and notarizes the DMG with `release.sh`, has knope tag `v<version>` and publish the GitHub release with the changelog section and the DMG, checks the downloaded DMG against the cask's sha256, and pushes the cask to `Fanzzzd/homebrew-agent-screen`.

Commits follow Conventional Commits, and pull requests merge by squash with the title as the commit's subject (`CONTRIBUTING.md`, Commits and pull requests). `pr-title.yml` checks the title with `.github/scripts/check-pr-title.sh`. knope reads no commit messages (`ignore_conventional_commits`): versions and the changelog come from changesets only. prepare-release skips a push whose commit starts with `chore(release): Agent Screen `, the squashed release pull request.

Running **Release** from the Actions tab is a dry run: checks, signing and the DMG, kept as an artifact, and nothing published.

Its secrets are the Developer ID certificate (`DEVELOPER_ID_P12_BASE64`, `DEVELOPER_ID_P12_PASSWORD`), an App Store Connect API key of the same team for notarization (`APP_STORE_CONNECT_KEY_P8`, `APP_STORE_CONNECT_KEY_ID`, `APP_STORE_CONNECT_ISSUER_ID`), `TAP_DEPLOY_KEY`, a write deploy key of the tap, and `RELEASE_DEPLOY_KEY`, a write deploy key of this repository: prepare-release pushes the `release` branch with it, so CI runs on that branch and its checks show on the pull request. To build a notarized DMG locally, store the key once with `xcrun notarytool store-credentials agent-screen` (see the top of `release.sh`).
