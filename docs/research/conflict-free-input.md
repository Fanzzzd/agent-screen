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
