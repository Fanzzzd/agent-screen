# Conflict-free input

ADR 0001 says agents never take the user's pointer, keyboard or focus. Three cases broke it or came close:

- **#37, web drags.** Chromium and WebKit read `buttons` from `[NSEvent pressedMouseButtons]`, WindowServer's state for the whole session. Closed PR #89 made drags work by holding the session's left button for up to 0.6 s. The maintainer rejected it, since any chance of a conflict is too much.
- **#48 and #79, dropdowns.** An open `NSMenu` takes the session's keyboard. PR #91 shrinks that to 66-122 ms per choice, which still leaves a window.
- **#39, Electron menu items.** An AX press reaches Electron as a keyboard shortcut. PR #90 only adds a note.

This file covers each candidate, the experiment behind it, and what happened. Everything ran on macOS 26 (Darwin 25.2.0) with Electron 44.7.0 (Chromium 152), Chrome 155 and bare AppKit apps. The probes live in `/tmp/cfi` and are not in the repo. Every live run used `AGENTSCREEN_INSTANCE=e2e`, held one Agent Display under the display lock, and showed the countdown sign first.

## Summary

Two mechanisms cover most cases with no shared state at all:

1. **Choose by typing, never by opening a menu.** A closed `<select>` in Chromium and a closed `NSPopUpButton` both take type-ahead. Agent Screen focuses the control through AX and types the item's title with the per-process keys it already sends. No menu opens and the session's key focus never moves. This needs no new channel and works in apps the agent didn't start. An `NSComboBox` takes `AXValue` plus `AXConfirm`.
2. **A private CDP pipe for the Chromium and Electron apps `launch_app` starts.** `--remote-debugging-pipe` puts DevTools on fds 3 and 4, which only Agent Screen holds. `Input.dispatchMouseEvent` with `buttons`, `Input.dispatchDragEvent` and `Input.insertText` run inside the browser and never touch WindowServer's button state or key focus.

What remains open: drags in WebKit apps and in Chromium apps the agent didn't start, context menus, pull-down buttons, and #39's fidelity issue (which involves no conflict with the user).

## How a menu takes the keyboard

`-[NSMenuTrackingSession _setupMenuTracking]` (AppKit, disassembled) does two things:

- It calls `CPSStealKeyFocusReturningID(CGSMainConnectionID(), 0xc80, &id)` unless its process already holds key focus. Session key focus is a stack in WindowServer. The menu's process goes on top, and the user's keys go there wherever they work.
- It calls `CGSSetBackgroundEventMask(cid, 0x24000fe)`, so the process also gets mouse downs, ups, moves, drags and scrolls from anywhere. That is how the user's click elsewhere closes the menu.

`_tearDownMenuTracking` calls `CPSReleaseKeyFocusWithID` and puts the old mask back.

Tested with a probe (`stealback`) on a bare AppKit pop-up:

| Step | Result |
| --- | --- |
| AX press on the pop-up | the menu's process held key focus 8 ms later |
| A third process steals key focus for the user's app's connection | `err 1003`, refused |
| A third process steals key focus for itself | allowed, and the menu closed at once (`menuDidClose`, no item chosen) |
| That process releases its id | `err 1010`. Focus stayed on it until it exited, then went back to the user's app |

No process outside the menu's own can keep a menu open without the keyboard. Taking focus back closes the menu, and the release API misbehaves. This rules out "open the menu, give the keyboard back". No SkyLight call fits the bill either: `SLSSetEventCapture*` is never called (traced with lldb), and the focus stack has no per-connection switch.

So the only conflict-free menu is one that never opens.

## Candidates and results

### 1. Type-ahead on a closed control

AppKit's `-[NSPopUpButtonCell keyDown:inRect:ofView:]` gathers typed characters into a string with a hysteresis timer, which is type-select. `moveUp:` and `moveDown:` call `performClickWithFrame:inView:`, which opens the menu. So letters choose and arrows open. Blink's menulist works the same way on the Mac.

The probe (`cfiprobe` with `kfwatch` watching key focus and the target's windows every 3 ms):

- A native `NSPopUpButton` in a bundled app on an Agent Display, launched by `launchApp`. `AXFocused=true` returned success and the button reported focused. Agent Screen's own `type "ch"` gave `pop action Cherry`, and `type "b"` gave `pop action Banana`. The app showed only its layer-0 window, never a layer-101 menu. Key focus stayed with the user's app the whole time.
- A `<select>` in Electron, launched by `launchApp`, with no CDP involved. `AXManualAccessibility`, then `AXFocused=true` on its `AXPopUpButton`, gave `select focus`. `type "ch"` and `type "b"` made the page fire `input` and `change` with Cherry, then Banana. No menu window appeared and key focus didn't move.
- A Chrome `<select>` reports a settable `AXValue`, but setting it does nothing (#48 notes). An `NSPopUpButton`'s `AXValue` isn't settable, and a closed button has no children.
- An `NSComboBox` takes `AXValue = "Blue"` (returns `ok`, reads back `Blue`). `AXConfirm` then fires the action (`combo action Blue`). No menu opens.

Limits: type-ahead matches a prefix of the title, so Agent Screen types the whole title and checks `AXValue` afterwards. Items with identical titles can't be told apart. Disabled items are skipped. WebKit's menulist has the same type-ahead, but Safari wasn't tested (it may be the user's). Pull-down buttons (`AXMenuButton`) run actions, not a value, and weren't tested.

### 2. CDP over a private pipe (Chromium and Electron launched by the agent)

`/tmp/cfi/tools/cdp.py` spawns the binary with a pipe on fds 3 and 4 and speaks null-terminated JSON over it.

- **Electron 44 honors `--remote-debugging-pipe`.** It returned `Browser.getVersion` as `Chrome/152.0.7977.130`. Chrome 155 does too, on a throwaway `--user-data-dir`. Since Chrome 136, Chrome ignores the switch on its default profile.
- **Splitter drag.** `mousePressed` with `buttons:1`, 19 moves, then a release took the splitter from 200 to 390 px. The page saw `pointerdown buttons=1`, and all 19 moves carried the button.
- **HTML5 drag.** `Input.setInterceptDrags`, then moves, then `Input.dragIntercepted` with `text/plain: row`. After that, `dispatchDragEvent` sent `dragEnter`, `dragOver` and `drop`. The page got `drop data=row` and `dragend dropEffect=copy`.
- **Typing.** `Input.insertText "héllo 你好"` reached the field whole. `dispatchKeyEvent` typed characters.
- **Select.** Setting `value` through `Runtime.evaluate` and dispatching `input` and `change` worked.
- **Session state.** A poller read `NSEvent.pressedMouseButtons` and the cursor every 2 ms throughout: 9892 samples, 0 nonzero, and the cursor never moved.
- **A click on a `<select>` still opens a native menu.** Over CDP on an Agent Display, the app held key focus at the first sample after the press (polled every 3 ms) and showed a layer-101 window 36 ms later. Mobile emulation and touch emulation didn't change that. After injecting `select, ::picker(select) { appearance: base-select }`, the picker opened in the page (`:open` true) and no menu window appeared. A CDP click on the option set Cherry, and key focus never moved. So a CDP channel must turn select clicks into a choice, or inject base-select.
- **When Agent Screen exits.** The pipe closes and the app keeps running (Chromium logs "Connection terminated while reading from pipe"). An app adopted after a restart has no channel.

#### Which Electron apps accept the flag

There are 31 Electron apps in /Applications. Their fuses (`EnableNodeCliInspectArguments` and the rest) never cover `--remote-debugging-*`, so the fuses don't decide it. App code does:

- Claude.app's `app.asar` lists `remote-debugging-port` and `remote-debugging-pipe` among the switches it filters.
- Figma removes `remote-debugging-port` at startup unless `FIGMA_TEST` is set. Whether it also drops the pipe wasn't tested.
- Antigravity's `app.asar` mentions the switches twice. Those mentions weren't read.
- The other 28 don't mention the switches. None of the 31 was launched, since they hold user data.

Cost: `open` can't hand fds to the app. `launch_app` would spawn the bundle's executable itself (`posix_spawn` with fds 3 and 4), and lose `open -g -j`. Window catching already handles where the windows go. The parent-pid check from #35 changes accordingly.

### 3. Safari through safaridriver

Not run: Safari may be the user's. `safaridriver` (Safari 26.2) is installed. WebDriver needs `safaridriver --enable` with an admin password and Develop > Allow Remote Automation. It listens on a localhost TCP port, so any local process can reach it, although only one session runs at a time. Its windows are separate and ephemeral, without the user's cookies, and a glass pane blocks the user from them. It doesn't drive WKWebView apps.

Verdict: not worth it. An agent's Safari would have none of the user's logins, and a throwaway Chrome over a pipe does the same job more privately.

### 4. A separate input session

There is one WindowServer process (pid 407) and one console session. The button scoreboard and the key focus stack are per session. macOS gives one user one GUI session, and `CGSCreateLoginSession` is gone. The only second session available is another macOS user's: Screen Sharing's virtual session, or a VM. Apps there run with another home folder and keychain, so they have none of the user's logins and data. Agent Screen would stream that session over VNC instead of using virtual displays.

Verdict: feasible only as a separate sandbox product, not for "operate my apps".

### 5. SkyLight switches

There is no setter for the button scoreboard (#37 notes). Menu tracking uses `CPSStealKeyFocusReturningID` and a background event mask, not event capture (see above). Nothing per connection keeps a menu open without the keyboard.

### 6. #39: the AX press reads as a shortcut

Electron's `-[ElectronMenuController itemSelected:]` builds its flags from `[NSApp currentEvent]`. `triggeredByAccelerator` comes out true unless that event carries a mouse button. Tests:

- A plain AX press on Go > Previous view logged `triggeredByAccelerator=true` (6 of 6 runs).
- A per-process key first, a left mouse up first, or a left mouse down first, sent to a point in no window with delays from 0 to 150 ms: true every time (10 of 10).

The press takes nothing from the user. Only its fidelity is wrong. For apps that ignore shortcuts while a field has focus, one idea is to blur `document.activeElement` over CDP before the press. That was not tested against such an app.

## Coverage

✓ means conflict-free and tested. (✓) means expected but not tested. ✗ means there is no conflict-free path.

| App family | Drag (pointer follows the button) | HTML5 or native drag and drop | Choose in a dropdown | Menu bar item | Context menu | Typing |
| --- | --- | --- | --- | --- | --- | --- |
| Chromium or Electron started by `launch_app` with the pipe | ✓ CDP | ✓ CDP | ✓ type-ahead, or CDP DOM | ✓ AX, with #39 fidelity gap | ✗ (CDP can fire `contextmenu` in the page, but the native menu opens) | ✓ keys today; ✓ `insertText` |
| Chromium or Electron without the pipe (placed windows, the user's running Chrome, Claude.app, Chrome's default profile) | ✗ | ✗ | ✓ type-ahead | ✓ AX, with #39 fidelity gap | ✗ | ✓ |
| Native AppKit | ✓ for views that follow posted events; ✗ for code that polls the button | in one process ✓ (#37 notes); across processes untested | ✓ `NSPopUpButton` type-ahead; ✓ `NSComboBox` AXValue; pull-down untested | ✓ AX (today) | ✗ | ✓ |
| WebKit (Safari, WKWebView apps) | ✗ | ✗ | (✓) type-ahead | ✓ AX | ✗ | ✓ |

## Recommendation

1. **Replace #91's open-and-pick with focus plus type-ahead.** For an `AXPopUpButton`, `set_value` sets `AXFocused`, types the full title through the per-process key path, then checks `AXValue`. For an `AXComboBox`, it sets `AXValue` and sends `AXConfirm`. This closes #48 and #79 with no window of conflict, in every app family, with no new channel. Close PR #91, or rework it this way.
2. **Never open a native menu for an agent.** A `click` or `press` that lands on an `AXPopUpButton`, `AXMenuButton` or `AXComboBox` gets its item list back with a pointer to `set_value`, the way a menu bar title already does. A right-click doesn't open a context menu: it returns an error that names the alternatives. This removes the source of #48.
3. **Add the CDP pipe for the Chromium and Electron apps `launch_app` starts.** Agent Screen spawns them itself with `--remote-debugging-pipe`, a throwaway profile for browsers, and base-select injected. It sends drags through `Input.dispatchMouseEvent` and `dispatchDragEvent`. This closes #37 for those apps. PR #89 stays closed.
4. **Where there is no channel, a drag fails and says why.** That covers WebKit, Chromium apps the agent didn't start, and apps that filter the flag. The error suggests `launch_app` for a Chromium app, or `request_help`. No latch, and no borrowing the user's mouse.
5. **#39 keeps PR #90's note.** It isn't a conflict. The blur-before-press idea for CDP apps can follow later.

## For the user to decide

- Whether Agent Screen may start Chromium and Electron apps itself (`posix_spawn`, not `open`) with a DevTools pipe. Nothing outside Agent Screen can connect to it, but it is a debugging channel into the app.
- Whether drags without a channel should fail (recommended), or ask to borrow the user's mouse for about a second.
- Whether clicks on dropdown and menu buttons, and right-clicks, should be refused in favour of item lists. A few agents' flows change.
- Whether context menus stay unsupported. No conflict-free path exists for them.

## Round 2: keeping menus without a conflict

The user wants agents to keep menu clicks, since many apps work only through them. This round tests input routing while an agent's menu is open, and the alternatives.

### Setup

The setup runs on one Agent Display with two scratch apps, both started by `launchApp`:

- **PopApp** is the agent's app, with an `NSPopUpButton`.
- **UserApp** stands in for the user's front app, with a text field. The probe activates it, so it holds key focus as the user's app would.

Then a probe (`/tmp/cfi/router/router.m`) does this:

1. It opens PopApp's menu through AX.
2. It types as "the user": HID-level events marked with `kCGEventSourceUserData`, so a tap can tell them apart from real hardware events.
3. Optionally, it switches to Apple Pinyin or turns on secure input in UserApp.

The router is an active event tap at the head of `kCGHIDEventTap`, and it acts only on the marked events. Both apps log key and mouse events with wall-clock times.

### Results

| Mode | The agent's menu | UserApp |
| --- | --- | --- |
| No router | took every key: type-ahead moved the highlight | nothing |
| **Route**: drop the key at the HID tap, post it to UserApp's pid | saw nothing and stayed open | `abc` in order, 0.3 to 47 ms after each key |
| Route, Apple Pinyin on | saw nothing | `nihao␣` composed `你好` |
| **Hold**: drop and queue the keys, `AXCancel` the menu, replay them at the HID tap once key focus has left the agent | closed; saw nothing | `abc` in order; key focus came back 18 to 57 ms after the first key |
| Hold, Apple Pinyin on | closed; saw nothing | `你好` |
| Route, with secure input on in UserApp | took every key | nothing: the tap never saw the keys |
| User's move, scroll and click on UserApp's window, no router | the scroll went to **PopApp**; the click went nowhere visible, and the menu stayed open | nothing |

How long a menu holds key focus inside a single call, measured three times on an AppKit pop-up: AX press, find the menu window, press the item, until key focus comes back.

- Key focus is taken 2 to 8 ms after the AX press.
- The menu window appears 115 to 128 ms later.
- The item press returns, and key focus comes back, after 458 to 472 ms in total.

PR #91 measured 66 to 122 ms for Chrome.

What the results mean:

- **Pid-posted keys go through the input method when the app they reach is the active app.** ADR 0008's limit applies to background apps. So routing doesn't break Pinyin for the user's front app.
- **The mouse is captured too.** While an agent's menu tracks, the user's scroll over their own window went to the agent's app, and their click didn't reach their window. These were synthetic HID events, and real hardware wasn't tested. If real input behaves the same, the conflict covers the user's mouse as well as their keyboard.
- **Secure input blinds every tap.** With a password field focused anywhere, the user's keys go to the agent's menu, and no router can stop that.

### Route compared with hold

**Route** keeps the agent's menu open, but it is the wrong tool for the session:

- Keys dropped at the HID tap never reach WindowServer. So system shortcuts (Command-Tab, Spotlight, screenshots) stop working, and the session's modifier state goes stale.
- A launcher panel that takes keys without coming to the front gets nothing.
- Routing clicks to a pid skips WindowServer, so window activation, title-bar drags, resizing and Mission Control are lost.

**Hold** keeps the user's input on its normal path, delayed. The input method, shortcuts, modifiers, clicks and drags all work after the replay. The cost falls on the agent, whose menu closes. The delay is how long the menu stays open, or the time to cancel it.

### The inverse: stop the menu taking focus

Not possible from outside the app:

- AppKit steals key focus whenever its process doesn't already hold it, popped up from a non-activating accessory app included.
- A third process can't steal for another app's connection (`err 1003`).
- Stealing for itself closes the menu (round 1).

`SLSSetMenuBarOwner` concerns the menu bar, not pop-up tracking. The only way in is code injection, which hardened apps refuse.

### Other routes

- **Snapshot the menu, close it, pick later in one call.** This works for any menu an agent's click opens: pop-ups, pull-downs and context menus.
  - Agent Screen sees the app's layer-101 window appear after the agent's click. It reads the items through AX, cancels the menu, and returns the items, the way it returns a menu bar title's items.
  - When the agent picks an item, Agent Screen repeats the opening click and presses the item through AX in the same call. Key focus is held under 0.5 s (above), with no agent think time inside it.
  - It breaks for menus whose items change between opens, and for menus that open on hover.
- **A macOS VM** (Virtualization.framework):
  - Isolation is complete, so there is zero conflict.
  - It runs on Apple silicon only, and the macOS licence allows two extra copies.
  - Each VM needs a 15 to 18 GB restore image, 30 GB or more of disk and 4 to 8 GB of memory, and takes about a minute to boot.
  - The guest has none of the user's apps, logins or keychain. The user signs in again inside it.
  - It would be a separate product mode, many weeks of work. Not run.
- **A second GUI session** needs another macOS user (round 1). Same verdict as the VM, minus the image.

### Recommendation for menus

Combine the snapshot route with hold:

1. **Agents never leave a menu open between calls.** A click that opens a menu returns its items, and the menu closes. A pick reopens the menu and presses the item inside one call.
2. **For that window (under 0.5 s), an HID tap holds the user's events.** It holds keys, flags, button downs and ups, and scrolls, but not pointer moves, so the cursor keeps moving. The tap releases the events as soon as key focus leaves the agent's app. If the window runs past a deadline (say 300 ms), it cancels the menu first.
3. **Before it opens any menu, Agent Screen refuses while secure input is on** (`IsSecureEventInputEnabled`, as the ime path already checks).

The user then sees at most a few hundred milliseconds of lag once in a while. None of their input reaches the agent, and none is lost.

Remaining risks:

- Real hardware events weren't tested, only marked HID events.
- Secure input that turns on mid-window can't be held.
- A crash between drop and replay loses the held events. A watchdog that replays them, and macOS's own tap timeout, bound that.

Effort: medium, about the size of PR #89's tap plus the menu snapshot.

### Incidents during this round

Fixed in the scratch apps:

- In two runs, UserApp was started with `open`, not `launchApp`. AeroSpace moved its window to the user's screen and tiled it there for about 10 s. One of those runs posted a marked move, scroll and click at that window's spot, (150, 1001) on the main screen, and put the cursor back afterwards.
- In one hold run, Agent Screen gave the front back to the user's Dia (#67), and the router replayed `abc` there.

The scratch apps now:

- quit within 20 ms if their window leaves the Agent Display;
- quit after 60 s;
- start through `launchApp`.

The router now types and replays only while key focus is one of the scratch apps.

## Round 3: survey

The maintainer asked for every feasible approach, from other projects and from lower in macOS, combined into the best design. This round surveyed both, then ran three experiments on the leads nobody had tested here. The probes live in `~/.asq/cfi3`, outside the repo. Every live run used `AGENTSCREEN_INSTANCE=e2e`, one Agent Display under the display lock, and a scratch app (`PopApp3`, one `NSPopUpButton`) started through `launchApp`.

### What other projects do

| Project | Menus | Drags | Zero conflict? |
| --- | --- | --- | --- |
| trycua/cua `cua-driver` | Key-focus guard ([#4943](https://github.com/trycua/cua/pull/4943), [#5129](https://github.com/trycua/cua/pull/5129)): when a background menu steals key focus, release the theft by id and keep the menu open; menu bar items through AX while closed ([#5380](https://github.com/trycua/cua/pull/5380)); pop-ups open and close inside one call ([#4740](https://github.com/trycua/cua/pull/4740)) | Per-pid posts; refuses AppKit drag and drop when a drag image appears ([#5309](https://github.com/trycua/cua/pull/5309)); Electron only through CDP from the main process's Node inspector ([#5351](https://github.com/trycua/cua/pull/5351)); opt-in brief foreground drag that moves the real pointer ([#5481](https://github.com/trycua/cua/pull/5481)) | Close. They report 0 lost keys over 12 trials, down from 3 to 65 per trial |
| OpenAI Codex computer use (`SkyComputerUseService`) | Not published. cua #4943 says Codex handles the theft the same way | Coordinates only | Unknown |
| Anthropic background computer use (Sept 2026) | Not published | Not published | Unknown |
| Microsoft UFO² | UIA patterns (`Invoke`, `ExpandCollapse`, `SelectionItem`) | Real cursor; isolation comes from an RDP loopback session ([paper](https://arxiv.org/html/2504.14603v1)) | Only inside the separate session. macOS has no equivalent short of a second user or a VM |
| Agent S, Hammerspoon, Keyboard Maestro, AltTab | Real cursor, or the app comes to the front for a moment. Hammerspoon's `selectMenuItem` is an AX press | Real cursor | No |
| yabai scripting addition | Code in Dock.app reorders, moves and layers windows. It has no event posting and no menu code ([payload.m](https://github.com/koekeishiya/yabai/blob/master/src/osax/payload.m)) | none | Not relevant, and it needs SIP partly off |
| XCUITest, Appium mac2 | Takes the HID devices exclusively ([mac2 README](https://unpkg.com/appium-mac2-driver@0.11.0/README.md)) | Real cursor | No |
| safaridriver, WebKit automation | `WebAutomationSessionMac` replaces `+[NSEvent pressedMouseButtons]` inside the web process while it delivers synthetic events ([source](https://github.com/WebKit/WebKit/blob/main/Source/WebKit/UIProcess/Automation/mac/WebAutomationSessionMac.mm)) | Same | Yes, but for Safari automation windows only (round 1, candidate 3) |

Nothing found on macOS gives a second key focus, a second cursor or a second button scoreboard in one login session. Karabiner's virtual HID feeds the one cursor. Universal Control and Sidecar move that same cursor.

### Experiment 1: out-of-process menus

On macOS 26.2, `-[NSMenuTrackingSession _setupMenuTracking]` skips `CPSStealKeyFocusReturningID` when `NSMenuIsOutOfProcess()` is true. AppKit turns that on for the os_feature `CoreFoundation/CFPasteboardVerification`, the app default `NSMenuEnableOutOfProcessMenus`, or the pasteboard privacy developer preview. Agent Screen could pass the default as a launch argument.

`PopApp3` launched with `-NSMenuEnableOutOfProcessMenus YES` read the default as set. The AX press opened the menu in a new process, `TrustedUIService (PopApp3)`, which owned the layer-101 window and took key focus 58 ms after the press. Without the argument, PopApp3 itself took key focus 4 ms after the press. So the flag moves the theft to another process and doesn't remove it. Apple also dropped the related `EnablePasteboardPrivacyDeveloperPreview` default in 27.2 beta 2 ([thread](https://mastodon.social/@_inside/116285733111262102)). Dead end.

### Experiment 2: release the menu's theft from outside

Round 1 concluded that no outside process can keep a menu open without the keyboard. That was wrong. Round 1 called `CPSReleaseKeyFocusWithID(cid, id)`, which returns 1010 for every id. The SkyLight function cua uses takes the id alone: `SLPSReleaseKeyFocusWithID(id)`. Theft ids come from one counter for the whole session.

The probe (`guard.m`) does this once the menu holds key focus and its window is on screen:

1. It steals key focus on its own connection and gets id N, then releases N.
2. It releases N−1, the menu's theft. The return is 0.
3. Its event tap on the scratch app (`CGEventTapCreateForPid`, event type 21) drops the "lost key focus" notice (field 64 = `0x4000`, field 73 = the probe's pid) that step 1 sends the menu's app.

Results. The user's front app was Dia, and nothing was posted to it.

| Mode | Key focus | The menu | Pick through AX |
| --- | --- | --- | --- |
| No release | the menu's app until the menu closed | open | Cherry chosen |
| Release, no tap | back to Dia 0.6 ms after the release | closed 24 ms later, nothing chosen | none |
| Release with the tap | back to Dia 0.6 ms after the release | still open 300 ms later | Cherry chosen (`pop action Cherry`); key focus stayed with Dia |

Earlier, without any release, the probe stole key focus for itself with options 0, 0x80, 0x400, 0x800 and 0xc80, and each time the menu was still open 150 ms later.

What this means:

- An agent's menu can stay open and be driven through AX while WindowServer sends the user's keys to the user's app again. The menu holds the keyboard only from the press until its window is on screen. That took 47 to 83 ms here.
- The menu's app keeps its background event mask. `CGSSetBackgroundEventMask` on another process's connection returns 1000. So while the menu is open, the user's clicks and scrolls still reach the menu's app (round 2), and a click closes the menu.
- WindowServer also sends the user's front app a "lost key focus" notice. cua drops it with a tap on that app, so its window never resigns key. This round tapped only its own scratch app. Whether Dia's window resigned key and came back wasn't measured: AX reported `AXFocused=0` for Dia's window before, during and after, in both modes.
- Not tested here: Chromium and Electron `<select>` menus and context menus. cua reports the guard works for a Chrome `<select>` and for `AXShowMenu` context menus.

### Experiment 3: the AX surface of closed controls

On a closed AppKit `NSPopUpButton` and on a pull-down button, AX lists no children, `AXShowMenu` and `AXPress` as the actions, and only `AXFocused` as settable. `NSComboBox` has `AXValue` settable (round 1 uses it). This matches cua #4740. A closed menu can't be read through AX, so a list of items needs an open menu.

### Other candidates, ruled out

| Candidate | Why not |
| --- | --- |
| Code injection (`DYLD_INSERT_LIBRARIES`) to stub the theft or `pressedMouseButtons` | The hardened runtime ignores the variable, and nearly every notarized app has it |
| yabai-style scripting addition in Dock | SIP partly off. It also offers nothing for key focus or button state |
| testmanagerd and XCTest events | Session-level HID, the real cursor |
| A DriverKit virtual HID device | The same session cursor and the same key focus |
| A second session through Screen Sharing, or a VM | Zero conflict, but none of the user's apps or logins (rounds 1 and 2) |
| Option bits on `CPSStealKeyFocusReturningID` | Every value behaved the same |
| The Node inspector in Electron's main process (cua #5351) | Covers Electron apps that drop `--remote-debugging-pipe`, but it opens a TCP port any local process can reach, and apps can fuse it off |

### Ranked solutions

**1. In-call menus with the focus guard (all apps, no new channel).** This is the round 2 recommendation with the key hold mostly replaced:

- Choose without opening wherever possible: type-ahead on a focused pop-up or `<select>`, `AXValue` plus `AXConfirm` on a combo box, and AX presses on menu bar items (round 1).
- Any other menu opens only inside one call. A click that opens a menu gets the items back and the menu closes. A pick reopens the menu and presses the item in the same call (round 2).
- During that call, Agent Screen taps the target app and drops its own "lost key focus" notice. Once the menu window is on screen, it releases the menu's theft. The user's keys then go to their own app while the menu stays open.
- For the 50 to 80 ms before the release, the HID tap holds the user's keys and replays them after (round 2's hold). For the whole call, under 0.5 s, it holds the user's button presses, releases and scrolls, since the menu's app keeps its mouse mask. Pointer moves pass.
- Agent Screen refuses to open a menu while secure input is on.

What the user notices: keys typed in the first 80 ms come through up to 80 ms late. A click or scroll during a menu call comes through up to half a second late. Nothing reaches the agent and nothing is lost. Effort: medium. It is round 2's tap plus a SkyLight release and one per-pid tap.

**2. Add the CDP pipe for Chromium and Electron apps `launch_app` starts** (rounds 1 and 2, in progress). It covers drags, HTML5 drag and drop, `insertText` and `<select>`, with base-select injected so no native menu opens. Nothing else found gives drags in a web page without the session's button state. WebKit apps, the user's own Chrome and apps that filter the flag still get a refusal that explains why.

**3. An isolated mode for full zero conflict:** a second macOS user or a VM, as a separate product mode. Only this covers canvas apps, drags in WebKit and drags between processes. The user signs in again inside it. Effort: weeks.

Solutions 1 and 2 together are the recommendation. Solution 3 is optional and later.

### For the user to decide

- Whether Agent Screen may release another app's key-focus theft through private SkyLight (`SLPSStealKeyFocusReturningID`, `SLPSReleaseKeyFocusWithID`), as cua and reportedly Codex do. For a few microseconds during the id lookup, Agent Screen itself holds key focus. A user key in that instant would reach Agent Screen, which can replay it.
- Whether Agent Screen may also tap the user's front app to drop the one "lost key focus" notice, as cua does. That stops the user's window from resigning key, but it intercepts an event meant for a process the agent didn't start. Without it, whether the user's window stays key needs a test with the user present.
- Whether mouse input during a menu call should be held (recommended) or allowed to close the agent's menu.

### Incident during this round

After a reboot, the rebuilt scratch app made its window with `NSWindow(contentRect:…)` at Agent Display coordinates. In six launches the window appeared at (1028, 361) on the user's main screen. The app's own check quit it 21 to 25 ms later each time. The window is now created with `screen:` set to the Agent Display and the app refuses to show it unless its frame is on that display. Every later run placed it at (4388, 2857) on the Agent Display.
