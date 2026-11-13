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
skills/            the agent-screen skill, docs/agents/AGENTS.md with frontmatter
.claude-plugin/    the Claude Code marketplace that serves the skill as a plugin
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
- `AGENTSCREEN_TEST_PARK_REFUSED=1`, only in a debug build with `AGENTSCREEN_INSTANCE=e2e`, refuses parking verification after applying the real configuration. `parksafe` uses it to test failed creation without deliberately leaving a display beside the user's. Release builds ignore it.

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

- **Virtual displays** come from the private `CGVirtualDisplay` API, the same one DeskPad and BetterDisplay use. Each display is 1280 × 800 points by default, at one pixel per point: macOS lists a two-pixel mode, but refuses to switch a virtual display to it (`CGDisplaySetDisplayMode` returns 1001), so the Viewer scales the image up. The app parks its displays in a compact grid off the top-right corner of your rightmost physical screen (`agent_screen_model::arrangement`): the first touches it only at that corner, and the rest fill rows no wider than your screens, in serial order. macOS won't leave a display floating, so the grid starts at a corner a screen has, not at the corner of all your screens together, which is empty when their tops don't line up (#155). A display is accepted wherever macOS puts it if it overlaps none of your screens, touches each at most at a corner point and never along an edge, so the real cursor can't slide onto it, and sits right of your screens, no lower than the rightmost one's top; anywhere else, creation fails and the display is released. Your screens stay where you put them, and System Settings shows them at a usable size. Not a bottom corner: AeroSpace hides other workspaces' windows there, hanging off screen, and they would show on the display.
- **Display changes are expensive.** Adding, removing or moving a display is a reconfiguration every app on the Mac handles. Each one also makes WindowServer (SkyLight's `CAWSManager::restore_color_preferences`) ask `colorsync.displayservices` for every online display's profile, and wait 50 ms for the answer. A later answer is dropped, and WindowServer asks again 5 s later, until one comes in time; every display change starts another such loop, and no setting takes a display out of the request. For each online display, the service has colorsyncd read and parse its whole registry of displays, `/Library/Caches/ColorSync/com.apple.colorsync.devices`, five times. The registry keeps a record for every display serial macOS has ever seen, each with the display's name in 39 languages, 97% of what gets parsed. So an answer takes about online displays × records × 0.2 ms: with 84 records and four displays online, 66 to 91 ms, and no loop ever ended; they piled up to hundreds of requests a minute, and apps that ask for display information hung (issue #26). A new virtual display also kept sRGB until an answer came in time. Agent Screen gives each display the lowest serial none of its displays holds (`display::Serial`), so its records are as many as the displays it holds at once. ADR 0009's helper, set up once with the user's password and run with `sudo -n`, removes the records of displays not connected that hold only factory data and keeps each record's names in English and the Mac's languages; macOS leaves a trimmed record as it is when its display comes back. The app runs it 3 s after a display is created or released, every 30 minutes and 10 s after launch, and writes its JSON line to `displays.log`. After that, 13 displays online took 19 ms, where 4 took 23 ms with untrimmed records; neither unregistering nor the rewrite starts a request. A late answer still happens under load, at 60 to 130 ms, and its loop ends at the next try. Before a new display, the app estimates the answer's time from the displays online and the records left and refuses `create_display` over 40 ms (`Core::budget_allows`). `AgentScreen colorsync [setup|remove|reclaim]` drives the helper by hand. Agent Screen also keeps its changes few: an ended, empty display stays connected as a spare for the next display of its size, two at most for ten minutes; agents get ten new displays per ten minutes; and a new display is parked once, while a reused one already sits in place. `~/Library/Logs/Agent Screen/displays.log` (`displays-e2e.log` for the test instance) records each display created, kept, reused and released, with its unit number, frame and serial, so a slow Mac can be checked against it.
- **Live frames** come from ScreenCaptureKit as 420f pixel buffers that GPUI draws without a copy. One stream per display runs at the highest frame rate and size any view asks for: 60 fps at full size for the Viewer, 12 fps at 416 px wide for thumbnails. ScreenCaptureKit tells displays apart by unit number, and WindowServer gives a new virtual display the unit 5 plus the number of virtual displays already there, without checking that no display has it. Once virtual displays come and go out of order, a new one can get the unit of AeroSpace's closet, and every stream and screenshot of it then shows the closet, the user's hidden windows. `VirtualDisplay::create` keeps such a display until another one comes, so that one gets the next unit, then lets it go. `screenshot::filter` refuses a display that still shares its unit (`cg::unit_twin`), so neither the Viewer nor an agent gets a picture of it, and each streamed frame's screen rect is checked against the display's bounds: frames of another screen are dropped.
- **Input** in the panel goes to the target process through SkyLight's `SLEventPostToPid`, stamped with window-routing fields, plus a focus record that tells the target window it has focus without raising it. The recipe is ported from [trycua/cua](https://github.com/trycua/cua) (MIT), minus its defocus record to the front app: that one ends the user's input method session. Your cursor stays put, and your front app keeps its place and its key window. Each event carries the time it is sent and its point in the target window, as hardware input does: with no time, Chromium kills a page whose click changes its URL, and AppKit reads the window point only from an event that has a time. A drag's first move goes in steps of a point, then waits 100 ms, since AppKit starts a resize or drag only after a few points and loses a move that jumps past that.

## Test

The frame-placement regression injects the actual AX writer callback to check reserved-strip constraints without touching a window. The scroll regression constructs synthetic CGEvents through the production constructor and reads their fields. It does not post them or inspect a browser. These tests do not verify Dia's real zoom, focus or virtualized-list behavior.

```sh
cargo test                    # unit and isolated integration tests
cargo test -p agent-screen-mcp --test diagnostics # CLI/MCP over an isolated endpoint; no app, display or input
cargo test -p agent-screen-mcp --test element_steps # step actions and subtree schema, socket forwarding, validation and scoped changes; no app or input
cargo test -p agent-screen-mcp --test queue_adapter # actual app coordinator, compiled without GPUI; no app or input
cargo test -p agent-screen-mcp --test input_queue_protocol # queue/drag MCP statuses and no fallback after input_pending; mock socket only
cargo test -p agent-screen-model input_queue # lifecycle, FIFO, withdrawal, approval generations and expiry
cargo test -p agent-screen-model substitute # slider direction inference and numeric guards
cargo test -p agent-screen-model tree # production traversal: shallow discovery beyond a long list, subtree budgets and ancestry guards
cargo test -p agent-screen-model numbers # full versus partial reads preserve stable element numbers
cargo test -p agent-screen-model watch # wait_for_change: conditions, counting appearances by content, timeout default and cap, poll gap
cargo test -p agent-screen-model screen_edge # the Input Grant edge's outline: smoothed corners, notch, panels with measured shapes, and the outline against points of Apple's bezel art (crates/model/testdata)
```

End-to-end scripts drive the real app. Each is one file in `tests/e2e/src/bin`, run with `cargo e2e NAME`. `tests/e2e/src/manifest.rs` lists them all: the areas each covers, its tier, how it drives the app, whether you must keep your hands off the Mac while it runs, and the most displays it makes. A unit test fails when a script has no entry there, or when its line below sits under another area than its first.

### Run the scripts that cover a change

```sh
AGENTSCREEN_INSTANCE=e2e cargo suite --changed            # the scripts that cover what differs from origin/main; --base REF names another commit
AGENTSCREEN_INSTANCE=e2e cargo suite --area viewer-input  # one area's scripts, or several: --area peek,takeover
AGENTSCREEN_INSTANCE=e2e cargo suite --tier core          # 17 scripts that reach every area
AGENTSCREEN_INSTANCE=e2e cargo suite --tier full          # every script the suite runs
cargo suite --list [--area A] [--tier T]                  # the scripts with their tier, drive, displays and last run; runs nothing
cargo suite --changed --dry-run                           # the plan and its estimated time; runs nothing
```

`suite` maps the changed files to areas (`PATHS` in `tests/e2e/src/select.rs`) and runs the scripts the manifest lists under those areas. A file that cuts across areas, such as `ui/shell.rs` or `control/agent.rs`, or one the map doesn't list, adds the core tier, and `suite` says so. A change outside the app and the test code runs nothing, and `suite` says that too. `--changed`, `--area` and `--tier` narrow each other: `--changed --tier core` runs the core scripts of the changed areas.

`suite` prints the plan and an estimate, then runs the scripts one at a time: the ones that leave you alone first, then the hands-off ones together, so you step away once. A failing script doesn't stop the rest. The run ends with a table of each script's result (pass, fail, skip or not run) and seconds, and exits non-zero when a script failed or was left out. Each script's output goes to `target/e2e/suite/NAME.log`. `target/e2e/suite/durations.json` keeps how long each script took, for the next estimate; a script never timed counts as 60 s.

A script has one of three tiers. `core` scripts are the small set that reaches every area. `full` is every script `suite` runs, the core ones included. `manual` scripts are never run by `suite`: tools, measurements that check nothing, and scripts with needs of their own, such as `run`, which quits the installed Agent Screen and moves your pointer. `suite` names the manual scripts that cover the chosen areas, and `--list` says why each is manual.

The manifest also says how a script drives the path it checks:

- `real`: real HID events, or a real input method. This is what the user's hands and the system do.
- `hook`: a debug control method plays the user's part. The code between the user's hand and that method never runs, so a pass says nothing about it. `viewerime` passed while the input method was broken for users (#117), because the script made the input method's calls itself.
- `agent`: only what an agent sends, over the control socket or MCP. For an agent's tools this is the real path.

Before each script, `suite` counts the `created display=` lines the test instance wrote to `displays-e2e.log` in the last 10 minutes, whoever ran the scripts. It waits until there is room for the displays the script makes, and says what it waits for. The budget is 12 displays per 10 minutes (`--display-budget`). A run that has made 60 displays stops and lists the scripts it left out (`--display-cap`). A script that makes more displays than the budget is left out.

### Scripts by area

Each script is listed under its first area. The lines with `AGENTSCREEN_INSTANCE=e2e` run beside your own Agent Screen.

```sh
# viewer-input: the user's clicks, keys, drags and scrolls in the Viewer
cargo e2e run                 # the user's side: scrolling, the menu bar strip, focus, permission setup
AGENTSCREEN_INSTANCE=e2e cargo e2e viewerdrag  # a title-bar drag in the Viewer stays on the display
AGENTSCREEN_INSTANCE=e2e cargo e2e viewerkey  # a click that makes the Viewer key again, and a paste after it in a Takeover, keep the app running
AGENTSCREEN_INSTANCE=e2e cargo e2e nativeclick # one click through the Viewer's handler on a field of an inactive native login window focuses it, after a click in another app, in the app's other window, and with the app in front
AGENTSCREEN_INSTANCE=e2e cargo e2e oneclick   # one real click on a field of an Electron login window, through the Viewer, focuses it; its sign shows each of the three steps (needs Electron.app and Field.app)
AGENTSCREEN_INSTANCE=e2e cargo e2e viewerpopup    # one real click in the Viewer on an item of a list that closes on any press in another app, as Qt pop-ups such as WeCom's search results do, picks the item
AGENTSCREEN_INSTANCE=e2e cargo e2e viewerqt       # real clicks in the Viewer on results of a real Qt app (PySide6, downloaded once into target/e2e/qtenv): in Qt's pop-up, a borderless window that takes the keys, as WeCom's, and one that never does, with your app or the list's app in front; each picks the result, not the row under it

# input-method: composing with an input method in the Viewer, and the agent's Keyboard Borrow
AGENTSCREEN_INSTANCE=e2e cargo e2e viewerime  # in a Takeover, the input method composes in the Viewer at Field's caret and Field gets only the finished 你好; keys with a plain layout, keys with no text and Command shortcuts still go to Field as keys; a hand-back on the agent's request card drops a composition, and text the input method commits later never comes back, even after the next click; a click commits one; a letter typed at once after a click waits for the focus lookup, then composes in a text field and goes as a key, never composed, in Field's password field; the password field skips the input method, also when reached by a Tab Field follows 60 ms late, as Chrome does, and typed into at once, and when Field focuses it by itself a composition is dropped and its letters reach the field as keys; the run prints the lookup and wait times; the next word's first letter typed at once after a commit composes without waiting; Return after a Korean syllable sends the syllable then Return; keys it passes through reach a key view as keys; a Return held down as a hand-back drops its repeat comes up in the key view; after a commit Field follows by moving to a password field that turns no secure input on, the next letter goes as a key, never composed; Shift typed at once after a click waits with the letter and arrives in order; a click with a keyboard layout looks up nothing and no key looks anything up on the main thread; with target/e2e/Electron.app, a letter typed into its page's password field before the page is published goes as a key, never composed, and the first word in a field on its page composes; your input source is untouched; 2 displays (needs Field.app from `ime`)
AGENTSCREEN_INSTANCE=e2e cargo e2e viewerwetype # real keys through the pinyin input method you have selected (WeType or Pinyin), after a real click on the Viewer's picture: "nihao" picked with 1 after a click and after a second click, with Space straight after a commit, and with 1 typed at once after a Return; each time the Viewer is the key window, every letter goes to the input method and none to Field as a key, Field gets the candidate's Chinese text and never composes by itself, and the candidate window stays up at the caret the Viewer gives it, on your screen, never on the Agent Display, without jumping; skips with another input source and never changes it; stops and presses Escape when you touch the keyboard or pointer; takes your pointer and keyboard for about 10 s; hands off; 1 display (needs Field.app from `ime`)
AGENTSCREEN_INSTANCE=e2e cargo e2e ime         # type and key through Apple's pinyin give 你好; your front app, input source and workspace stay; waits for you to be idle 3 s

# agent-input: the agent's clicks, keys, typing and scrolls, sent in the background
cargo e2e smoke [app]         # end to end with Calculator
AGENTSCREEN_INSTANCE=e2e cargo e2e robust      # what agents get back, and nothing on your screen
AGENTSCREEN_INSTANCE=e2e cargo e2e electron       # an Electron window, from tests/e2e/fixtures/electron: launch with arguments, an accessory app's window that opens on the display, clicks, typing, edge resize, late redraws, a window larger than the display, and one its app grows past the display or moves to the main screen after it shows; needs target/e2e/Electron.app
AGENTSCREEN_INSTANCE=e2e cargo e2e shared      # an app you and an agent both use: its keys reach only its window
AGENTSCREEN_INSTANCE=e2e cargo e2e sharedbrowser # one Electron app, two windows (#110): its user window on your screen, key and in front, its agent window on the display; a click, type, Return and a button in the agent's window reach its page through accessibility or fail naming the user's window; in a login form, Tab and shift+Tab move focus between its fields, type goes into the field Tab reached, and Return names the form's submit button, whose click submits it once; a click, act_on_element press and Return on its select open no menu and name set_value, set_value there says nothing was chosen, a click on a link whose page cancels it succeeds with changed false, and clicks with a real effect say changed true; on a page grown to about 5000 elements it prints five clicks' median time with the check and without it (debugPressCheck); with the focused window forced unknown (debugFocusUnknown), a click presses through accessibility, a click on blank page, act_on_element press on the select and show_menu are refused with no menu, and neither window's focus changes; show_menu beside the user's window opens no context menu; File > Close Window pressed from the agent's window state is refused and the user's window stays open; the user's window never loses key focus or sees an event; with your front app back, posted keys reach the agent's window as before; needs target/e2e/Electron.app; 1 display
AGENTSCREEN_INSTANCE=e2e cargo e2e openpanel   # a file Open panel: keys, clicks, window state, picking a file by path
AGENTSCREEN_INSTANCE=e2e cargo e2e typedrops   # typing into address fields in Chrome on a throwaway profile: a busy page and a controlled field get every character; a page that drops fast keys makes the type action fail and say what the field holds; 1 display
AGENTSCREEN_INSTANCE=e2e cargo e2e scrolljump  # 3, 60, 200 and 800 wheel clicks move a long virtualized list in Chrome 40 px each, not one page at most (#104); 1 display
AGENTSCREEN_INSTANCE=e2e cargo e2e keys        # the key action in Chrome: a page sees ], }, cmd+[ and cmd+] as those characters on any keyboard type, and the brackets land in the field; 1 display

# input-queue: Input Requests, the Input Queue, Input Grants and Input Loans, which use the user's real pointer
AGENTSCREEN_INSTANCE=e2e cargo e2e inputqueue  # the Input Queue's card with a preview intent: it asks, shows the grant, and leaves when the grant idles out; the agent then sees approval_lapsed, and a wait or same-token retry returns at once; the card fits its content and the Input Bar shows at the bottom, clear of the card, during the grant, and its Stop ends it; a menu item's request (`input-queue-menu`) asks on the same card and names the item, and so does a double click (`input-queue-click`); three preview drags (`input-queue-drags`) ask on one card, and allowed with one shown running (`input-queue-running`) the card fits Stop; captures the cards, the bar, and the red edge at the main screen's top-left corner and top middle (a built-in display's corner and notch); no real input; about 45 s; 1 display
AGENTSCREEN_INSTANCE=e2e cargo e2e inputqueue -- --buttons-only # real HID clicks on the painted Allow and Not Now buttons: a preview-only request opens a grant that survives the next queue tick, then a new request is declined; no queued action executes; restores pointer and front app; hands off; 1 display
AGENTSCREEN_INSTANCE=e2e cargo e2e realdrag    # the Input Queue's executor moves your pointer (ADR 0013): an approved drag on the Electron harness's splitter, opened without a pipe, runs with route real_input and the splitter follows; a red edge shows meanwhile; a pointer move of yours ends the approval and leaves the pointer; after done the pointer goes back; needs target/e2e/Electron.app; hands off; 1 display
AGENTSCREEN_INSTANCE=e2e cargo e2e realclick   # a click with your pointer (ADR 0013, Clicks): the HIDClick fixture's button takes a click only while the HID system reads the button down, so a background click leaves it red; the same click with realInput asks first, names the click and the app, and an actual HID click on the painted Allow button turns it green with route real_input; the reply says how long the request waited; with no other agent using the approval, it ends at once and your pointer and front app come back; an allowed double click arrives as one; a right click, and a Control-held left click, with realInput are refused at once, naming show_menu, and send nothing; a declined click sends nothing; after the fixture repaints while the request waits (SIGUSR1), an allowed click clicks nothing and tells the agent to take a new screenshot; moves your pointer for a second, twice; hands off; 1 display
AGENTSCREEN_INSTANCE=e2e cargo e2e filedrag    # a file dragged between two apps' windows on one display (#166, ADR 0013, Drags between windows): FileSource from tools/filedrag.swift starts a system drag session with a file URL, FileDrop takes it; the background drag asks for your pointer instead of reporting route posted, and declined sends nothing; allowed, it runs with route real_input and FileDrop gets the file after at least 3 dragover updates; with E2E_ELECTRON or target/e2e/Electron.app, the same file dropped onto the Electron harness's drop zone reaches the page, which prints its dragover count and time; the app log's `input queue: drag of` line gives the drag's phases in ms; a drop window moved while the request waited gets nothing and nothing is pressed; moves your pointer for a second; hands off; 1 display
AGENTSCREEN_INSTANCE=e2e cargo e2e realclick -- --allow-only --observer-timeout # disable the real input observer during a preview grant: the grant ends without executing, the observer recovers without restoring approval, and a fresh actual Allow executes one fixture click and restores pointer/front app; hands off; 1 display
AGENTSCREEN_INSTANCE=e2e cargo e2e loanfocus   # an Input Loan's focus step alone, 20 rounds (E2E_LOOPS) on one display (#121): a fresh background Electron harness, which AeroSpace has never seen in front, handed over and made main and frontmost; AeroSpace never moves it off the Agent Display, nor the user's windows, and nothing shows on your screen; needs target/e2e/Electron.app; hands off; about 2 min; 1 display

# displays: an Agent Display's life, from creation and parking to spares and review
AGENTSCREEN_INSTANCE=e2e cargo e2e restart    # after the app aborts, the agent hears it quit and can place its windows back (needs Field.app from `ime`)
AGENTSCREEN_INSTANCE=e2e cargo e2e lastapp    # the only app on a display quits, and the display stays: a screenshot and the next launch_app on it work (needs Field.app from `ime`)
AGENTSCREEN_INSTANCE=e2e cargo e2e holdmode       # a display macOS switches to 800×600 by itself is back at the size it was made with, and the display log says so; 1 display
AGENTSCREEN_INSTANCE=e2e cargo e2e spares         # an ended display comes back as the next one, with no display change, and the display log records the app start and each display released at quit
AGENTSCREEN_INSTANCE=e2e cargo e2e warmspare      # while you are idle the app makes two spares, and the next two displays take them with no display change; 3 displays
AGENTSCREEN_INSTANCE=e2e cargo e2e serials        # a new display takes the serial a released one freed
AGENTSCREEN_INSTANCE=e2e cargo e2e arrangement    # new displays park in a grid off the top-right corner of your rightmost screen, beside each other while the row fits, sharing no edge with your screens; nothing already there moves; 2 displays
AGENTSCREEN_INSTANCE=e2e cargo e2e parksafe       # refused parking verification fails create_display and releases the unassigned display; existing display frames stay unchanged; one empty display, no app launches or pointer input; requires a stopped e2e instance
AGENTSCREEN_INSTANCE=e2e cargo e2e sparesize      # a 1280x800 spare grows to 1280x1600, parks again and is reused by the next session; one empty display, no app launches or pointer input; requires a stopped test instance
AGENTSCREEN_INSTANCE=e2e cargo e2e asksave     # closing a display whose app asks whether to save: the question stays on the display, which goes once the window has closed, and stays after Cancel

# windows: Placement, launch_app, and whose a new window is
AGENTSCREEN_INSTANCE=e2e cargo e2e launchhere # an app opened from a launcher while the user works in a Viewer opens on that display (needs Field.app from `ime`)
AGENTSCREEN_INSTANCE=e2e cargo e2e latewindow # an app launch_app started opens its main window on your main screen 4 s after the loading window it answered with, once idle, once with its main thread busy for 1.5 s, as WeCom's is, and once busy for 1.5 s after the window shows: the main window lands on the display and in the claim file, not on your screen; then the app, hidden after its window showed, stays hidden; 1 display
AGENTSCREEN_INSTANCE=e2e cargo e2e reopened   # a window an app the agent started opens by itself, long after the agent's last action, while its other windows are on the display: it opens there too; 40 s (needs Field.app from `ime`)
AGENTSCREEN_INSTANCE=e2e cargo e2e placesize  # place_window sets a window's frame on the display and returns the one the app kept, below its minimum and past the display's edge
AGENTSCREEN_INSTANCE=e2e cargo e2e placebrowser  # Chrome on a throwaway profile: place_window returns the frame the window took, and launch_app with a file path in the running browser opens a new window on the display showing the file (#168); skips while Chrome runs; 1 display
AGENTSCREEN_INSTANCE=e2e cargo e2e placeheld  # Field moves to a user stand-in stage; place_window takes it as a stray, or fails when it keeps moving back; 2 displays, nothing on your screen
AGENTSCREEN_INSTANCE=e2e cargo e2e claims     # the claim file lists the displays and the windows placed on them, drops a window taken back, and is gone after quit; with AeroSpace running, the display gets none of your workspaces or windows (needs Field.app from `ime`)
AGENTSCREEN_INSTANCE=e2e cargo e2e launchagain    # launch_app with arguments on an Electron app that allows one instance and already runs on the display answers with its window, not "did not start"; needs target/e2e/Electron.app
AGENTSCREEN_INSTANCE=e2e cargo e2e secondwindow   # an Electron app on the display that brings itself to the front to show a window, at launch and for a window.open: your front app is back within 300 ms; shows the sign first; needs target/e2e/Electron.app
AGENTSCREEN_INSTANCE=e2e cargo e2e samebundle     # launch_app with arguments takes the instance it started, not a copy of the same Electron.app started from a shell meanwhile; needs target/e2e/Electron.app and a quit e2e app
AGENTSCREEN_INSTANCE=e2e cargo e2e alerts      # a system alert put on an agent display moves to the main display: a UserNotificationCenter alert, then a stand-in for FollowUpUI's Apple Account sign-in alert; counts down on your screen first, where each shows for a moment
AGENTSCREEN_INSTANCE=e2e cargo e2e coldflash   # Chrome started by launch_app RUNS times (20) on one display, each on a new throwaway profile: its first window never shows on your screen, which the run's screen watch checks at every frame (#123); 1 display

# window-access: Window Requests, Handovers and Window Authorization
AGENTSCREEN_INSTANCE=e2e cargo e2e requestwindow # place_window by app asks for a window started by a separate stage session; Keep leaves it on the stage, Hand Over places it as a Handover; 2 displays, nothing on your screen
AGENTSCREEN_INSTANCE=e2e cargo e2e windowaccess # Window Authorization on a real Field window on a user stand-in stage: an app grant takes it without a card except during a Takeover, the user's Revoke leaves it, return all sends it home at its frame and leaves the agent's own window, a regrant of its app or all apps still asks with a card, the user's words for that window take it and clear the exclusion, and Take Back excludes it again; captures the Viewer and Peek in each state (AGENTSCREEN_LANG=zh for Chinese); 2 displays, nothing on your screen
AGENTSCREEN_INSTANCE=e2e cargo e2e closetwindow # a window of yours in another AeroSpace workspace, on its closet display: place_window by bundle id and by .app path asks for it, Keep leaves it in its workspace, Hand Over by id brings it to the display, where it stays, and a return sends it back to that workspace; a name nothing matches gets a not_found saying what was compared; needs AeroSpace; Field shows on your screen for a moment; 1 display
AGENTSCREEN_INSTANCE=e2e cargo e2e handoverfront # Login is the user's front app (debugFront) with one window handed over: with its other window focused on the user stand-in stage, a double click on the Handover is refused and names that window (#110); once that window is handed over too, the double click and a type reach the Handover (#162); 2 displays, nothing on your screen
AGENTSCREEN_INSTANCE=e2e cargo e2e endhandover # a window the user handed over goes back to the user stand-in stage when the agent ends its display, while the agent's own Login keeps the display in review; the same session's next display takes it again, and place_window for Login says it waits for review and blames no other session (#161); 3 displays, nothing on your screen

# takeover: Takeover, Help Requests, Control Requests and conversation control
cargo e2e control             # Takeover and Help Request, needs a debug build running
AGENTSCREEN_INSTANCE=e2e cargo e2e takeover    # Takeover through the Viewer
AGENTSCREEN_INSTANCE=e2e cargo e2e requestcontrol # request_control during a Takeover, from an agent with a long name: the card shows with both buttons inside it (debugCard), Keep Control tells the waiting agent and leaves the Takeover, Let … Continue hands the display back, a withdrawal leaves the Takeover, and without a Takeover it answers at once
AGENTSCREEN_INSTANCE=e2e cargo e2e conversation # chat pause and hand-back, session window authorization, Takeover suspension, revocation and cross-session rejection through MCP; one empty display, no app launches or user-window movement
AGENTSCREEN_INSTANCE=e2e cargo e2e helpanswers # each answer to a Help Request, as the agent's request_help hears it: Continue and Reply from the card and the notification, the user's Handover of a Field window from a user stand-in stage, Take Over and its hand-back, a withdrawal, an answer after the call timed out, and Later then a new request with the same reason; captures the card and its reply field per AGENTSCREEN_LANG; 2 displays, nothing on your screen but the card
AGENTSCREEN_INSTANCE=e2e cargo e2e helpcard    # the Help Card: shows, Later, comes back, steps aside for the Viewer and Takeover; the Viewer lists the request's steps; captures a Window Request's card for a short and a long app name, per AGENTSCREEN_LANG

# peek: Peek and the menu bar icon
AGENTSCREEN_INSTANCE=e2e cargo e2e appwhere   # an app brought to the front with its windows only on an Agent Display brings Peek down at that display's tile with its window outlined, once a minute for automatic activation; every explicit Dock click reminds again, including while it is already frontmost, and Peek goes by itself; one with a window on your screen, or one the agent started that brings itself forward, gets none (needs Field.app from `ime`)
AGENTSCREEN_INSTANCE=e2e cargo e2e peekhover   # hover opens a preview; the first icon click pins it, a later click closes it; pointer departure closes only an unpinned preview
AGENTSCREEN_INSTANCE=e2e cargo e2e peekanim       # "+" shows a waiting tile at once and grows it in at the display's rate, the new display made once it has grown; a closed display's tile shrinks out the same way; neither tile's animation starts over when the card stops sliding, and the stage clip of Peek is kept; 2 displays
AGENTSCREEN_INSTANCE=e2e cargo e2e peekmisroute  # a stream that shows another screen after a display change (AGENTSCREEN_TEST_MISROUTE, after a real mode switch) starts again by itself and shows the display within 5 s, no Viewer opened, and the display log notes the misroute (debugStreams); 1 display
AGENTSCREEN_INSTANCE=e2e cargo e2e peekrapid      # three + in a row with Peek opened by a click: Peek ends as one card filling its window; GAP_MS (1500) between them; 4 displays
AGENTSCREEN_INSTANCE=e2e cargo e2e statusicon     # the menu bar icon's front screen fills while a display is in use and empties once it ends (debugIcon); the pictures come from `apps/macos/assets/status/make.py`; 1 display
AGENTSCREEN_INSTANCE=e2e cargo e2e displaychange  # a display comes and goes while Peek is open: Peek draws at its new size at once, with no draw GPUI skipped (AGENTSCREEN_LOG_ALL=1 logs GPUI's own errors), keeps drawing, and the new display gets its wallpaper; 2 displays

# viewer: the Viewer's picture, header and Agent Pointer
AGENTSCREEN_INSTANCE=e2e cargo e2e viewerpicture  # new displays get unit numbers of their own, and the Viewer shows each
AGENTSCREEN_INSTANCE=e2e cargo e2e viewerheader  # the Viewer at its narrowest, on a 640×1600 display: the header's buttons fit for an agent, a Help Request and a Takeover; captures each, per AGENTSCREEN_LANG; 1 display
AGENTSCREEN_INSTANCE=e2e cargo e2e pointer     # records the Agent Pointer in the Viewer through moves and quick clicks, frames at 60 fps in target/e2e/pointer; needs ffmpeg

# elements: get_window_state and act_on_element on menus, selects and controls
AGENTSCREEN_INSTANCE=e2e cargo e2e compactstate # get_window_state with max_depth and act_on_element with result changes or none send a fraction of the tree (skips while Calculator runs, quits it after)
AGENTSCREEN_INSTANCE=e2e cargo e2e waitchange   # wait_for_change on the Watch fixture, whose buttons, pressed through act_on_element, change it seconds later: an idle window times out at timeout_seconds; appears fires on "Delivered" with + Delivered, - Sending… and ~ the status; a subtree wait ignores a change outside its root; a change that landed before the call fires at once against the last tree; disappears fires with the old line; an unmet condition times out; the window closing ends the wait; 1 display, no real input, about 30 s
AGENTSCREEN_INSTANCE=e2e cargo e2e subtreestate # native Electron AX: an 800-row earlier branch truncates the default read; shallow reads discover the later pane, root reads reach its card, scoped actions report changes and partial reads preserve unrelated IDs; one display, no private apps or real input
AGENTSCREEN_INSTANCE=e2e cargo e2e cursormenu     # Cursor, a VS Code fork, on a throwaway profile: Edit > Find pressed through accessibility with the terminal focused runs as ⌘F there, and the press answers with a note naming the Command Palette (#39); skips when Cursor runs; 1 display
AGENTSCREEN_INSTANCE=e2e cargo e2e menupress      # act_on_element press on an Electron menu item: with no field focused, through accessibility as Electron's shortcut with a note; with the field focused after typing, an Input Request that, allowed, chooses it once as a click on the display's menu bar, keeps the field's text and focus, sends the page nothing and puts the pointer and front app back; File > Open Recent > notes.txt, one submenu down, the same; declined, nothing; real_input false, the shortcut again. Moves your pointer for a second; shows the sign; 1 display; needs target/e2e/Electron.app
AGENTSCREEN_INSTANCE=e2e cargo e2e menuchoice  # a native pop-up button chosen by set_value through type-ahead, a repeated key and CJK included, and a combo box set and confirmed, with no menu opened; a title with a space, a pop-up that takes no focus and a WKWebView select are refused before any key; focus moving to a text field mid-title stops typing and leaves the field empty; the combo box beside it keeps its value; a title an earlier item starts with, a wrong-case title, a rejected commit and unsafe titles fail and say so; a Takeover stops either choice; 1 display
AGENTSCREEN_INSTANCE=e2e cargo e2e selectvalue # a closed select in Chrome, or Brave while any Chrome runs, on a throwaway profile, chosen by set_value through its DevTools pipe with one change event per choice and no menu opened; a title it shows sends nothing, and a missing title fails; selects in another site's frame are refused before any key, and the field one would focus stays empty; any open menu is closed at once (#79); 1 display
AGENTSCREEN_INSTANCE=e2e cargo e2e menuclosed  # a select menu in Chrome on a throwaway profile: typing into the open menu chooses its item; once the menu has closed, as at your click or key anywhere, the next click or key fails and says so; 1 display
AGENTSCREEN_INSTANCE=e2e cargo e2e signupcontrols # a Google-style signup page in Chrome: a custom combobox's option chosen by press, click and arrow keys, and radios by press and click (#79); 1 display

# browser: web pages in Chrome, Safari and Electron, and the DevTools pipe
AGENTSCREEN_INSTANCE=e2e cargo e2e browsers    # Safari: the first click, URLs in a running browser, a select menu
AGENTSCREEN_INSTANCE=e2e cargo e2e chrome      # Chrome on a throwaway profile while Firefox starts on another display: a click focuses a page's field, typing lands in it, a button runs the page's script
AGENTSCREEN_INSTANCE=e2e cargo e2e firstclick  # a page opened in a running Chrome by launch_app runs its button's script on one click (#79); 1 display
AGENTSCREEN_INSTANCE=e2e cargo e2e webdrag     # drags in web pages go through the DevTools pipe launch_app gives Chromium apps (ADR 0010): the Electron harness's splitter, which follows pointermove only while buttons holds the left button, and an HTML5 drop; the same harness started with open sets its range input through accessibility, and declines a queued drag or press as Not Now does, with nothing sent; Chrome on a throwaway profile (left out while Chrome runs); the session's left button never reads down; needs target/e2e/Electron.app; 1 display

# screenshots: the agent's screenshots
AGENTSCREEN_INSTANCE=e2e cargo e2e agentshots     # a screenshot is the whole display scaled down; a zoom over all of it works
AGENTSCREEN_INSTANCE=e2e cargo e2e barleak        # screenshots right after the Electron harness quits never show your menu bar's status items; needs target/e2e/Electron.app; 1 display

# recording: a Recording's video, sound and timeline
AGENTSCREEN_INSTANCE=e2e cargo e2e recording   # record_display: the video's length and size, the timeline's actions and offsets, ending the display ends it
AGENTSCREEN_INSTANCE=e2e cargo e2e sound       # a recording has the sound of an app on its display (440 Hz, from the Electron fixture) and not of a process elsewhere (2000 Hz); mutes your output while it runs; needs Electron.app

# settings: the Settings window and permission setup
AGENTSCREEN_INSTANCE=e2e E2E_NOW=1 cargo e2e settings  # the Settings window opens, fits a 13-inch screen and closes with Done; shows on your screen

# release: self-update and the release bundle
SPARKLE_ED_KEY_FILE=<key file> cargo e2e selfupdate   # a release build updates itself through Sparkle (ADR 0012): 0.0.1 under a test bundle id, with a feed on 127.0.0.1 naming a signed 0.0.2 DMG, downloads it, holds it while an Agent Display is open and installs it in place once the display ends; builds the release bundle; 1 display

# performance: what displays, launches and input cost, in numbers
AGENTSCREEN_INSTANCE=e2e cargo e2e timing      # how long create_display and launch_app take
AGENTSCREEN_INSTANCE=e2e cargo e2e load        # with no displays, then LOAD_DISPLAYS (12) showing Chrome pages, or the Electron fixture's when Chrome is yours: CPU of WindowServer, replayd and Agent Screen, how late a plain AppKit app runs, and every agent typing and taking its screenshot at once; LOAD_SECONDS; AGENTSCREEN_DISPLAY_HZ and AGENTSCREEN_DISPLAY_SCALE (1 or 2) set the test build's display mode
AGENTSCREEN_INSTANCE=e2e cargo e2e perf        # what agents and you wait for, over ROUNDS rounds: a new and a reused display, launching Calculator, clicks, screenshots, window states, display changes, the test app's main-thread stalls and how late a plain AppKit app runs meanwhile; JSON in target/e2e/perf. Two agents measuring at once wrap each run in `/usr/bin/lockf /tmp/agent-screen-perf.lock env AGENTSCREEN_INSTANCE=NAME cargo e2e perf`, so runs never overlap
AGENTSCREEN_INSTANCE=e2e cargo e2e changecost     # how long every window on the Mac stops drawing when a display is added and removed, by Core Animation and by a main thread; the test build's AGENTSCREEN_NO_PARK, AGENTSCREEN_MAX_PIXELS=fit, AGENTSCREEN_DISPLAY_SCALE and AGENTSCREEN_DISPLAY_HZ compare choices; 1 display
AGENTSCREEN_INSTANCE=e2e E2E_NOW=1 cargo e2e flash  # TextEdit launched again RUNS times (15): how long each new window shows on your screen before it moves, with the distribution; TextEdit must not be in use

# tools, which cover no area
AGENTSCREEN_INSTANCE=e2e cargo e2e stop        # quit a test instance a stopped script left running
cargo e2e demo        # records the README's demo beside your Agent Screen, as the instance `demo`, and writes docs/media/demo.gif; needs ffmpeg
AGENTSCREEN_INSTANCE=e2e cargo suite --changed  # runs the scripts that cover a change; see above
```

Test scripts count the displays they make in `/tmp/agent-screen-test-displays`. `E2E_DISPLAYS_PER_HOUR=N` makes `createDisplay` fail in the script past N in an hour; there is no limit by default. About 60 displays in 11 minutes froze this Mac once, so compare builds with few rounds rather than loops.

The scripts live in `tests/e2e/src/bin`, one file each, with the helpers they share in `tests/e2e/src/lib.rs`. `cargo e2e` is an alias in `.cargo/config.toml` for `cargo run --quiet --package agent-screen-tests --bin`. The Swift helpers they call (`probe`, `hid`, `flash`, …) are in `tests/e2e/tools` and build into `target/e2e` on first use.

Every script shows a sign on your main screen (`tests/e2e/tools/overlay.swift`, started by `run` in `lib.rs`). A script first builds what it needs, the test app and any Swift helper or fixture app in `tests/e2e/tools` whose source is newer than its binary, with the badge saying so, while you keep using the Mac; the countdown starts after the build. It counts down from 3 in the middle with the script's name: in orange with "don't touch the mouse or keyboard / 请勿操作鼠标和键盘" for scripts that move your pointer, take your keyboard or input source, mute your sound or put windows, alerts, cards or focus on your screen (the `HANDS_OFF` list in `lib.rs`), in blue with "Agent Screen e2e running / 测试运行中" for the rest. Then it shrinks to a badge in the bottom-right corner that shows the script's name and each `check` label as it passes (✓) or fails (✗), and any step a script names with `sign_step`. At the end it shows the result: green "全部通过 / all passed" with the count of checks, red "N 项失败 / N failed" with the first failing label, or grey "跳过 / skipped" with the reason, and "可以操作了 / you can use the Mac" below. It goes 3 s later, 6 s after a failure. An error, a panic and `skip()` end it the same way. It is a non-activating panel of an accessory process that ignores the mouse, so it never takes focus or a click, and it is one per process however many threads a script runs. `capture` and `hid hover` take it off the screen while they capture, so pictures of the main screen never show it. `E2E_NO_SIGN=1` turns it off, for batch runs.

`run` also watches your screen for the whole script (`tests/e2e/tools/sighting.swift`). Every 10 ms it lists the windows on your displays, every display but Agent Displays and AeroSpace's closet, and keeps those of the test's processes: the script's descendants, anything whose executable or command line is in this checkout (`target/e2e` tools and apps, fixtures, throwaway browser profiles), and any app that started after the script did, outside `/System/Library` and `/usr` and other checkouts. The sign and the test app's own windows (Peek, cards, the Viewer, Settings) don't count. The script ends with the check "nothing the test started showed on your screen", which lists each window with its owner, title, frame, when it showed and for how long, and fails for any of them. `target/e2e/sightings-<script>.jsonl` keeps the list. A script may allow an owner's windows for some milliseconds with `allow_on_your_screen(owner, ms, why)`, only for a window macOS shows before Agent Screen can move it or one the script puts there on purpose, and says why in the call. `E2E_NO_SIGHTING=1` turns the watch off.

`robust`, `timing` and `shared` skip when TextEdit is already open, `openpanel` when Preview is, and `browsers` when Safari is, since it may be yours. `robust` and `timing` launch TextEdit, whose new windows can show on your screen for a frame or two. `robust` starts 3 s after you stop using the mouse and keyboard; `E2E_NOW=1` skips the wait.

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

`AGENTSCREEN_INSTANCE=e2e` runs the debug app as a second instance with its own socket (`control-e2e.sock`) and display serials, so your Agent Screen and its displays keep running. Those tests send input to app processes, never to your pointer or keyboard. `takeover` also keeps the Viewer off your screen: `debugShow` with the surface `stage` makes one of the test's displays stand in for your screen, and `debugViewerInput` makes clicks, key presses and drags (`kind` click, key or drag) inside the app, through the Viewer's own handler. A click with `key` 1 takes the keyboard as the user's click does. A key takes `shift` 1 and `command` 1 for those modifiers, `phase` `down` or `up` for one half of the press, and `repeat` 1 for a held key's repeat. `kind` shift with `down` 1 or 0 presses or releases Shift. A key or click takes `source` `layout` or `inputMethod` to stand in for the user's input source. A key takes `characters`, what it types, and `ime`, what the input method does with it: `pass`, `nothing`, `mark:TEXT`, `insert:TEXT` or `insertcommand:TEXT` (inserts, then asks for Return as a command), made through the view's own NSTextInputClient calls; its reply's `route` says what the key came to: the key itself (`raw`), composed text (`text`), composed text then the key (`textThenKey`), nothing yet (`inputMethod`), or waiting for the focus lookup (`held`), and `secrecy` what the Viewer knew of the focus when it routed the key. `kind` mark and commit, with `text`, make the input method's setMarkedText and insertText calls on the Viewer's view. `debugViewer` returns the marked text, the kind of input source the Viewer types with now (`source`: `layout` or `inputMethod`), whether the Viewer's input context is AppKit's current one (`inputContext`), the target's caret in the Viewer, whether a password field has the keyboard (`secure`), what the Viewer knows of the focus (`secrecy`: `unknown` until a lookup answers after the focus may have moved, then `secure` or `open`), how long the latest focus lookups took and how long held keys waited for them, in milliseconds (`lookupMs`, `heldMs`), each key press's final route by key code (`routes`), how many compositions a lookup dropped for a password field (`secureFound`), which a lookup in time leaves at none, the number of focus lookups asked for (`inspections`), and the lookups made on the main thread (`mainThreadLookups`), which should be none. `E2E_LOG=FILE` keeps the debug app's log.

`cargo e2e run` posts real pointer events, so it borrows your cursor. It starts 3 s after you stop using the mouse and keyboard, with no Agent Display open (`E2E_IDLE_SECS` changes the 3 s, `E2E_NOW=1` skips the wait). The sign shows what it waits for from the first moment, so you know it is about to begin. A script that sees your mouse or keyboard while it runs says so on the sign and in its result, since a failure may come from that. It puts the cursor, the front app, the AeroSpace workspace and the installed Agent Screen back. Name cases to run some (`cargo e2e run focus`): `scroll`, `strip`, `focus`, `permissions`. `focus` puts a stand-in app with a text field in front and checks that it stays the key window while an agent types: losing it ends the user's input method session. The permission case runs a copy under the bundle id `app.agentscreen.e2e-ungranted`, so it starts with no grants and never touches yours; what it checks ends where only a person can flip the switch. Captures go to `target/e2e`. `probe tap` (`tests/e2e/tools/probe.swift`) records real trackpad events for `hid replay`, which is how `tests/e2e/fixtures/trackpad.log` was made.

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

### Conversation control and window authorization

```sh
cargo test -p agent-screen-model access
cargo test -p agent-screen-model model
cargo test -p agent-screen-mcp tools::tests
AGENTSCREEN_INSTANCE=e2e cargo e2e conversation
AGENTSCREEN_INSTANCE=e2e cargo e2e windowaccess
```

The offline tests exercise authorization scope, Takeover suspension, explicit
hand-back, revocation, exclusions and MCP argument validation. The `conversation`
script uses one empty Agent Display and separate MCP sessions to check chat
pause/continue, authorization status, direct user revocation and session isolation.
It does not launch apps or move user windows. Run it against this checkout's
build; an already running test instance may be older than the bundle just built.

Those checks do not prove real-window Handover, return-location restoration or
Viewer/Peek layout. `windowaccess` covers them with owned Field windows: a
stage session launches the user's window on a user stand-in stage and gives it up
with `debugUserWindow`, and the target session launches its own. It drives the user's Revoke and Take Back through the
`access-revoke` surface and `debugTakeBack`, and captures the Viewer (`access`)
and pinned Peek under `target/e2e/windowaccess-<lang>-*.png`. Each language is a
separate run with its own two displays; the app picks its language at launch.
Peek's hover hint and the Viewer's hover text with the relayed words need a real
pointer and are not captured. Do not use the user's working windows as fixtures.

### Input Queue control protocol

`inputQueue` is scoped to the connection that created the display lifetime. It takes `displayId` and `action`: `request`, `wait`, `status`, `withdraw` or `done`. `request` needs a stable `operation` token and a `reason`; repeating the token returns the newest request with it, a declined or completed one included. Only a token whose newest request was replaced or withdrawn starts a fresh request, and the old `requestId` keeps its stopped result. A connection holds at most 64 requests across the displays it owns, finished ones included, of the queue's 256; past that `request` fails and asks for `done`. `done` clears one display's requests, so a control connection with several displays calls it on each. An Agent Session has one display, so for it the two are the same. `wait` and `withdraw` need `requestId`; `done` refuses one, since it clears that owner's results. `timeout` is 0 to 60 seconds, default 60 for request/wait; timeout leaves the request queued. Responses expose only that owner's request details, plus the total waiting count. `done` withdraws that owner's pending requests and declares that participant done; it never completes another session's operation.

Complete queued drags have `executorAvailable: true`. A request moves through `queued`, `approved`, `running` and `completed`, or ends `declined` or `stopped`. Only `completed` means its events were sent and the loan released its input. Generic `input_queue request` calls only ask; they have `runnable: false`, even after approval. A split `mouseDown` also only asks: no press is held across calls. A whole drag without a background route waits up to 60 seconds for approval; once running, it waits through cleanup even if that deadline passes. A call that times out detaches its queued drag; `wait` or a same-token drag retry attaches it again. When a grant ends before a request ran, it reports `approval_lapsed` with a `next` hint instead of waiting for a card that will not return. A new request or a revocation asks again. Successful drags report `route`: `devtools`, `accessibility`, `placement`, `posted` or `real_input`. A `click` with `realInput: true` queues the whole click the same way, sends nothing in the background meanwhile, and reports `route: real_input` once it ran; after its call reads the result, the session is done unless it has more waiting, so the grant ends. Background routes remain first; only `realInput: true` bypasses them to ask for a loan.

`apps/macos/src/input_queue.rs` holds admission, scheduling and projection; `queue_adapter` compiles the same coordinator without GPUI. `Core` forwards owner lifetime, Takeover and disconnect changes. The shell shows a click-through red edge and the Input Bar (`ui/input_bar.rs`), runs one loan at a time, and acknowledges grant cleanup after restoration. `mac/real_input.rs` posts tagged HID events and observes untagged input through a listen-only HID tap; an unavailable or disabled tap stops execution. Each drag or click rechecks its saved window/display geometry and the window under each point; secure input or a foreground change stops it. Real input and Keyboard Borrow share a session mutex. Per-process gates serialize posted events, DevTools input and AX mutations with a real-input loan; unrelated processes remain independent. Real-keyboard operations are not routed through the queue. Native runtime checks are separate from the offline coordinator tests.

The red edge is the outline of the screen's glass (`crates/model/src/screen_edge.rs`), stroked twice as wide and masked by itself so its outer side lies on the glass's edge. Under it, another stroke of the outline casts a soft red shadow in a layer masked by the glass, so only its inner half shows as a faint glow. The window never changes while it shows, so Core Animation draws it once. The rounding of the 14- and 16-inch MacBook Pro panels was fitted to Apple's bezel images from [Apple Design Resources](https://developer.apple.com/design/resources/), whose screen opening is the glass. `tests/e2e/tools/edgefit.swift` checks an outline against one: it reports the largest gap between the line and the glass's edge and any overhang past it, overall and for each top corner and the notch, and the line's width, and writes an 8x overlay of the corners and the notch.

```sh
swiftc -O tests/e2e/tools/edgefit.swift -o target/e2e/edgefit
cargo run -q -p agent-screen-model --example screen_edge -- 3456 2234 1 3456 2234 1542 1912 64 > target/e2e/edge16.txt
target/e2e/edgefit "MacBook Pro 16.png" 402 303 target/e2e/edge16.txt 4 target/e2e/edgefit.png
```

The example's arguments are the screen's size, panel pixels per point, the panel's native size and the notch as NSScreen reports it; here the 16-inch panel at one point per pixel, whose bezel image has the screen opening at (402, 303).

### Debug control methods

The control socket accepts these methods in debug builds. `AGENTSCREEN_DEBUG=1` enables them in release builds too, except for `userStage`. Scripts can drive the UI without a real click.

| Method | Arguments and result |
| --- | --- |
| `debugTakeover` | `displayId` and `active` begin or end a Takeover. |
| `debugShow` | `surface` selects an action from the list below. Supply `displayId` where the action needs one. |
| `debugViewerInput` | Makes a click, key press, Shift change or drag through the Viewer's own handler, or an input method's marked or committed text. See the input parameters above. |
| `debugMoveWindow` | Moves the window named by `pid` and `windowId` to global `x`, `y`. |
| `debugHandOver` | Hands `windowId` over to `displayId`, as from the Windows menu. |
| `debugFocusUnknown` | `AGENTSCREEN_INSTANCE=e2e` only. `on` true makes the front app's focused window read as unknown to the input routes, as when the app doesn't answer in time; false ends it. |
| `debugPressCheck` | `on` false stops checking a press beside the user's window for its effect, true starts again; `sharedbrowser` times a press both ways. |
| `debugTakeBack` | Takes `windowId` back from `displayId`, as from the Windows menu. |
| `debugUserWindow` | Debug builds only. On this session's user stand-in stage (`userStage`), makes `windowId`, a window it launched there, the user's own: the stage forgets it, and the claim keeps AeroSpace off it. |
| `debugAnswerWindow` | Answers the display's Window Request with `give` true or false once it opens. Returns `answered`. |
| `debugAnswerHelp` | Answers the display's Help Request as the Help Card does: `answer` `continue`, `reply` with `text` (typed into the card's field, then Send), `later` or `takeOver`. With `via` `notification`, takes the notification's action path instead: `continue`, `reply`, `takeOver` or `open`. Returns `answered`. |
| `debugLoanFocus` | Runs only an Input Loan's focus step on `windowId`: the window becomes main and its app frontmost, then the app that was in front comes back. Watches the window for `watchMs` (2000) and returns `before` and `after` frames, `came` (why the step failed, or null), `readyAtMs`, `movedAtMs` and `trips` (when and how often the window left its frame), `movedAfterReadyMs`, and `othersChanged`, the ids of other windows whose frames changed. Takes the front app. |
| `debugFront` | Records that macOS last named `pid` the front app. It does not activate the app. |
| `debugUserKey` | Records a user key press to `pid`, as the global key monitor does. |
| `debugActivated` | Simulates the user bringing `pid` to the front for the window-location reminder. Returns the displays it named as `reminded`. |
| `debugAgentReady` | Returns the agent the setup window says loaded the tools, as `agent`. |
| `debugCard` | Returns the Help Card's `width` and each button's `[x, y, w, h]` from the card's corner at its last paint, only those the last render drew. `buttons` maps button ids to bounds, and holds the queue card's first request row as `input-request`. `queueHeight` is the queue card's content height at its last paint. |
| `debugInputObserver` | `AGENTSCREEN_INSTANCE=e2e` only. `action: status` returns `watching` and the actual event tap's `enabled` state. `action: timeout` disables the retained tap and invokes the same handler as a macOS event-tap timeout. It never approves a request. |
| `debugIcon` | Returns the menu bar icon's `state`: idle, working, asking or setup. |
| `debugStreams` | Returns each display's capture stream: `running`, `starting`, `misrouted` and `broken` (it showed another screen or macOS stopped it), `delivered` frames, `hasFrame` for whether Peek and the Viewer have a picture, `breaks` (restarts in a row since its last good frame), `repairs` and `fps`. |
| `debugViewer` | Returns `frame` and `image` in screen points, `key` for whether the Viewer is the key window, `inputContext` for whether its input context is AppKit's current one, through which the input method composes, `takeBack` for the header button's bounds in points from the Viewer's corner, and `misrouted` for whether its stream showed another screen. |
| `debugPanic` | Panics on the main thread after replying. This aborts the test app. |
| `debugQuit` | Quits the test app after replying. |

`debugShow` accepts these surfaces:

| Surfaces | Action |
| --- | --- |
| `peek`, `click`, `menu`, `hide` | Open Peek held, click the menu bar icon, open its menu, or hide Peek. |
| `icon-hover`, `card-hover` | Simulate pointer entry with `displayId` 1, or exit with 0. |
| `select`, `viewer`, `collapse`, `focus` | Select a display, open or close its Viewer, or take the keyboard. |
| `marked` | Open the display's Viewer with "nǐ hǎo" marked by the input method, drawn at the target's caret, or at the last press in the Viewer. |
| `create`, `dismiss`, `dismiss-confirm` | Create a display, open Dismissal, or confirm the Dismiss sheet's current plan. |
| `primary`, `give`, `take-back`, `help` | Invoke the display's primary action, open its Give or Take Back menu, or open it for help. A menu opens at the pointer, or at `x` and `y` in global top-left points. |
| `answer` | Confirm the Viewer's Dismiss sheet with `displayId` nonzero, or cancel it with 0. |
| `card-open`, `card-later`, `card-take-over` | Open the Help Card's display, put the card off, or take over its display. |
| `card-give`, `card-keep` | Answer a Window Request with Hand Over or Keep. |
| `card-continue`, `card-keep-control` | Answer a Control Request with Let It Continue or Keep Control. |
| `card-help-continue` | Answer the display's Help Request with the card's Continue. |
| `card-reply`, `card-reply-cancel` | Open the Help Card's reply field or close it. Unlike the user's click on Reply, it leaves the keyboard with the user's app. `debugAnswerHelp` types and sends a reply. |
| `card-give-window` | Open the Give menu from the Help Card's Give a Window button. A window picked there answers the Help Request. |
| `access`, `access-revoke` | Open the display's Viewer to inspect its window authorization, or revoke it through the user's action. Supply `displayId`. |
| `window-request` | Open a Window Request for no real window, with `app` and `title` for its card. `card-keep` ends it. |
| `input-queue`, `input-queue-approved` | On this connection's live `displayId`, enqueue a preview intent, which only asks, and show the queue card, optionally approved as Allow approves it. Only available with `AGENTSCREEN_INSTANCE=e2e`. Approval includes the shared test queue, so run only under the display lock. |
| `input-queue-menu` | As `input-queue`, for a menu item of an Electron app that needs a real click: the card names it, `app` (default Slack) and Go > Back. The request never runs. `AGENTSCREEN_INSTANCE=e2e` only. |
| `input-queue-drags` | On this connection's live `displayId`, enqueue `count` (1 to 3, default 3) preview drags across the display, then a click with `presses` (1 to 3) when given, whose calls never wait, so none runs, and show the card with their paths and the click's point. `AGENTSCREEN_INSTANCE=e2e` only. |
| `input-queue-running` | As `input-queue-drags`, then Allow, and show the second step as the running one on the card and the Input Bar until the grant ends. Nothing runs. `AGENTSCREEN_INSTANCE=e2e` only. |
| `input-queue-click` | On this connection's live `displayId`, enqueue a preview click with `presses` (1 to 3, default 1) in `app`'s window `title` (default WeCom, Chats), whose call never waits, so it never runs, and show the card with its point. `AGENTSCREEN_INSTANCE=e2e` only. |
| `input-queue-allow` | Allow for what waits now, without adding a request, as the user's click does: an approved drag then runs with the real pointer. `AGENTSCREEN_INSTANCE=e2e` only. |
| `input-queue-decline` | Not Now for the current batch, without adding a request. `AGENTSCREEN_INSTANCE=e2e` only. |
| `input-bar-stop` | Press the Input Bar's Stop, which ends the grant as the user's click does. `AGENTSCREEN_INSTANCE=e2e` only. |
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

1. With each change users will notice, add a changeset: `knope document-change` asks for the kind (patch, minor or major) and a summary, and writes `.changeset/<name>.md`. The single `[package]` in `knope.toml` uses the changeset key `default`, not the Rust package name `agent-screen`, and CI fails a changeset with any other key, since knope would leave it out of the release silently. Write the summary for users. CI warns when the app changes without one; tests and refactors need none.
2. Every push to `main` with changesets runs `prepare-release.yml`. It keeps one pull request, **chore(release): Agent Screen x.y.z**, open from the `release` branch: the version in the three `Cargo.toml` files and `Cargo.lock`, and a new `CHANGELOG.md` section from the changesets, which it deletes.
3. Merging that pull request runs `release.yml`. It runs the checks, builds, signs and notarizes the DMG with `release.sh`, has knope tag `v<version>` and publish the GitHub release with the changelog section and the DMG, checks the downloaded DMG against the cask's sha256, and pushes the cask to `Fanzzzd/homebrew-agent-screen`.

Commits follow Conventional Commits, and pull requests merge by squash with the title as the commit's subject (`CONTRIBUTING.md`, Commits and pull requests). `pr-title.yml` checks the title with `.github/scripts/check-pr-title.sh`. knope reads no commit messages (`ignore_conventional_commits`): versions and the changelog come from changesets only. prepare-release skips a push whose commit starts with `chore(release): Agent Screen `, the squashed release pull request.

Running **Release** from the Actions tab is a dry run: checks, signing and the DMG, kept as an artifact, and nothing published.

Its secrets are the Developer ID certificate (`DEVELOPER_ID_P12_BASE64`, `DEVELOPER_ID_P12_PASSWORD`), an App Store Connect API key of the same team for notarization (`APP_STORE_CONNECT_KEY_P8`, `APP_STORE_CONNECT_KEY_ID`, `APP_STORE_CONNECT_ISSUER_ID`), `TAP_DEPLOY_KEY`, a write deploy key of the tap, `SPARKLE_ED_PRIVATE_KEY`, the EdDSA key that signs the update feed's DMG (ADR 0012; its public half is `SUPublicEDKey` in `Info.plist`), and `RELEASE_DEPLOY_KEY`, a write deploy key of this repository: prepare-release pushes the `release` branch with it, so CI runs on that branch and its checks show on the pull request. To build a notarized DMG locally, store the key once with `xcrun notarytool store-credentials agent-screen` (see the top of `release.sh`).
