# Web drags see no button held (#37)

An agent's drag in a Chromium or Electron page reached the page as `pointermove` with `buttons=0`. Code that follows a pointer only while the button is down took the moves for hover: react-resizable-panels splitters, sortable lists, sliders. HTML5 drag and drop failed too: `dragstart` fired, then Chromium ended the drag at once with `dropEffect=none`.

This file records what we tried, on macOS 26 (Darwin 25.2.0), and what Agent Screen now does.

## Where `buttons` comes from

Agent Screen already posts `LeftMouseDragged` to the target process with `SLEventPostToPid`. The event type is not what Chromium reads.

- `components/input/web_input_event_builders_mac.mm`, `ModifiersFromEvent`: every mouse event's modifiers take the button bits from `[NSEvent pressedMouseButtons]`. `WebMouseEventBuilder::Build` sets `button = kLeft` for `LeftMouseDragged`, but `buttons` comes from the modifiers.
- `third_party/blink/renderer/core/events/pointer_event_factory.cc`, `ConvertIdTypeButtonsEvent`: for a hovering pointer, `buttons = WebInputEventModifiersToButtons(modifiers)`. `web_pointer_event.cc` makes every mouse event hovering (`hovering(true)`). Only touch events get `buttons` from their state. `WebTouchEventBuilder` exists for the Mac, but `render_widget_host_view_cocoa.mm` never calls it. A tablet subtype makes the event a pen, which still takes the hovering path.
- AppKit, disassembled with lldb on this Mac: `+[NSEvent pressedMouseButtons]` loops over 32 buttons and calls `CGSInputButtonState(1, n)`. `SLSInputButtonState` reads a byte from `CGSScoreboard()`, memory WindowServer shares with every process. Nothing in it is per process or per connection.
- SkyLight exports no setter for that state. The symbols near it are `SLSGetCurrentMouseButtonState`, `SLSPostMouseEventsVButtons` (the old `CGPostMouseEvent`, which moves the cursor) and `CGSMouseStatePostMouseButtonsChangedEvents`.

WebKit's `WebEventFactory` on the Mac reads `[NSEvent pressedMouseButtons]` the same way, so Safari pages had the same problem. Safari is not tested here: it may be the user's.

## What changes the state, by experiment

Tools in `/tmp/w37` (Swift, not in the repo). Each posts a press and reads `NSEvent.pressedMouseButtons`, `CGEventSourceButtonState` for HID and combined state, and the cursor. Button 30 was used first: no app acts on it.

| Posted | `pressedMouseButtons` while down | Cursor |
| --- | --- | --- |
| `SLEventPostToPid` (Agent Screen today) | 0 | stays |
| `CGEventPost` at the session tap, any source state (HID, combined, private) | set | moves to the event's location |
| `CGEventPost` at the HID tap | set | moves to the event's location |
| `CGEventPost` at the annotated session tap | 0 | stays |
| A tablet pointer event with `tabletEventPointButtons=1` at the session tap | 0 | moves |
| Session tap, dropped by our tap at the head of the session tap | 0 | |
| Session tap, dropped by our tap at the head of the **annotated** session tap | **set** | stays when the event's location is the cursor's |

WindowServer updates the button state between the session tap and the annotated session tap. A tap at the head of the annotated session tap still drops the event before any app gets it. Checked with a listen-only tap for the pid under the cursor (Orca): without the drop, it received both marked events; with the drop, none. A zero-distance HID move posted while the button was held this way reached apps as `MouseMoved` (type 5), not as a drag, so the user's own mouse moves stay moves.

The press moves the cursor to its location even when dropped: a press at the cursor plus 1 pt moved the cursor 1 pt. So the press goes at the cursor's own location, read just before posting.

## The fix: hold the session's left button for an agent's drag

`apps/macos/native/ButtonLatch.m`, used by `Latch` in `apps/macos/src/mac/input.rs`.

- On the first use, a thread puts an active event tap at the head of the annotated session tap. It drops events carrying the marker `0x4153424c` in `kCGEventSourceUserData`, and notes the user's own left presses and releases: those whose `kCGEventSourceUnixProcessID` is 0. A 15-minute log on this Mac showed hardware presses with pid 0, and every Agent Screen instance's per-process events passing the same tap with that instance's pid.
- `SSLatchHold` posts a marked left press at the session tap, at the cursor, and waits up to 250 ms until `pressedMouseButtons` reads it. It posts nothing when the tap isn't running, since the press would then reach the app under the user's cursor, and nothing when the user already holds the button.
- `SSLatchRelease` posts the marked release and waits until the button reads up. It skips the release when the user pressed their own button meanwhile, so their release lets go and their own drag isn't cut.
- Holds nest; the last release lets go.

Where Agent Screen holds it:

- `left_click_drag` (`Forwarder::drag`): from before the press until just before the release, about 0.6 s. A Takeover mid-drag lets go first.
- `left_mouse_down`, `mouse_move` while the agent holds the button: for that step's events plus 60 ms. Between the agent's tool calls the button reads up, so the user's pointer doesn't read held while the agent thinks.
- Not the Viewer: the user's own button is down there, and apps already read it.

### Results

`AGENTSCREEN_INSTANCE=e2e cargo e2e webdrag`, before:

```
FAIL a drag's pointermoves have the left button held, and the splitter follows it: harness w=640 down=1 up=1 click=1 dbl=0 drag=0 nav=0 ts=2276 free=4
ok   the session's left button reads up after the drag, for every app
FAIL moves between mouse_down and mouse_up have the left button held, and the splitter follows: harness w=640 down=1 up=1 click=1 dbl=0 drag=0 nav=0 ts=1133 free=5
ok   the session's left button reads up between the agent's steps and after them: held after each step: [false, false, false, false]
FAIL an HTML5 drag of a row drops it on the page's drop target: harness w=640 down=1 up=1 click=0 dbl=0 drag=1 nav=0 ts=-1
ok   the session's left button reads up after the HTML5 drag
3 failed
```

After:

```
ok   a drag's pointermoves have the left button held, and the splitter follows it: harness w=640 down=1 up=1 click=1 dbl=0 drag=0 nav=0 ts=2134 held=23 split=80
ok   the session's left button reads up after the drag, for every app
ok   moves between mouse_down and mouse_up have the left button held, and the splitter follows: harness w=640 down=1 up=1 click=1 dbl=0 drag=0 nav=0 ts=1615 held=10 split=60
ok   the session's left button reads up between the agent's steps and after them: held after each step: [false, false, false, false]
ok   an HTML5 drag of a row drops it on the page's drop target: harness w=640 down=1 up=0 click=0 dbl=0 drag=1 nav=0 ts=-1 dropped=1
ok   the session's left button reads up after the HTML5 drag
all passed
```

HTML5 drag and drop works because AppKit's drag session in the source process tracks the events posted to that process, at the agent's points. In a separate harness (`/tmp/w37/dnd`), `dragend` reported the drop at the agent's point on the Agent Display, the drop target took it, and the user's cursor stayed at (24.7, 1006.5) through the whole drag. Before the latch, the same drag ended at once with `dropEffect=none` and no `drag` events.

In the last `webdrag` run the user's cursor read (1316.5, 526.0) before and after. `cargo e2e electron` (clicks and double clicks on draggable rows start no drag, edge resize by `left_mouse_down` and moves) passes with the latch. In `cargo e2e robust`, a Takeover still stops a drag short of its target. Its one failure, "place_window refuses the window taken back", times out on the Window Request card from main and has nothing to do with drags.

### A bug the test caught

The first version took any press with HID source state for the user's. Agent Screen's own per-process press passes the annotated tap with HID source state, so the latch thought the user held the button and never released: the session's left button read held for every app until the next real click. `webdrag` now checks that the button reads up after each drag and between the agent's steps. The user's presses are now those with no posting process.

## What the user may notice

- For up to about 0.6 s per drag, apps that ask read the left button as held. A Chromium page in the user's front app sees `buttons=1` on `pointermove` if the user moves the mouse over it then. A page acts on that only after its own `pointerdown`, which the user didn't make, so we expect no effect; it is not tested.
- If the user moves the mouse between the moment Agent Screen reads the cursor and the moment WindowServer takes the press, the cursor goes back by that distance, a few points at most at fast speed. Not measured.
- Another app's event tap at the session tap sees the marked press and release before ours drops them. A utility that acts on any click, such as a gesture tool, could react.
- If Agent Screen dies while holding, the button reads held until the user's next click.
- The tap needs Accessibility, which Agent Screen already has. Without it the latch does nothing and drags behave as before.

## Other directions, ranked

1. **The latch (done).** Works for every app that reads `pressedMouseButtons`: Chromium, Electron, WebKit, and AppKit code that polls the button. No cursor move, no activation, no window raised, no app receives the press. Public API only (`CGEventPost`, `CGEventTapCreate`).
2. **Chrome DevTools Protocol for apps the agent launched.** `launch_app` could add `--remote-debugging-pipe` or `--remote-debugging-port` to a Chromium or Electron app and send `Input.dispatchMouseEvent` with `buttons`, or `Input.dispatchDragEvent`. Issue #20 already showed CDP clicks working. Costs: only apps the agent starts, with Chromium's debugging switches allowed (Chrome 136 refuses them on the default profile); a port is open to every local process for the app's lifetime; coordinates must map from the screen to each frame and webview. Not needed now.
3. **Direction A, the user lends the mouse.** Queue actions that need the real pointer, ask the user once, then warp the cursor to the Agent Display, press, drag, release and put it back, within about 1 s. Needed only for what the latch can't do: apps that filter per-process events entirely (ADR 0001 names canvas apps and games), and native drags into another process that track the real cursor. It breaks ADR 0001 for the call, like ADR 0008 does for keys. Not built; the latch covers #37.
4. **Injection into the target** (`DYLD_INSERT_LIBRARIES` to swizzle `+[NSEvent pressedMouseButtons]`). Hardened-runtime apps, which is nearly every notarized app, ignore the variable. Ruled out.
5. **A virtual HID pointing device.** Its button would be the user's button, pressed where the user's cursor is. Ruled out.

## Native drags between processes

A drag from a web page or an AppKit view inside one app now works: the source process's drag session follows the events posted to it. A drag whose drop target is another process, such as a file from Finder into an app on the Agent Display, was not tested. The destination learns of a drag from WindowServer, which places it at the drag session's location; whether that location is the agent's point or the user's cursor needs a fixture with two apps on the display. If it is the user's cursor, that case needs direction A.
