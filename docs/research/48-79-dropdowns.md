# Dropdowns on an Agent Display (#48, #79)

Experiments from 2026-10-26, macOS 26, Chrome on a throwaway profile, and a bare AppKit app (`menuprobe`) that pops up an `NSMenu` on an Agent Display and logs every event, notification and the stack when the menu closes.

## Why an open menu closes at the user's input

An open `NSMenu`, which is what a web page's `<select>` shows in Chrome and Safari, runs `-[NSMenuTrackingSession startRunningMenuEventLoop:]` in its app. While that loop runs, WindowServer sends the session's keyboard input to the app that owns the menu, wherever the user is working:

- The user's F13, pressed at the HID level while `menuprobe`'s menu was open on an Agent Display, arrived in `menuprobe` as a keyDown with window 0. The user's front app was Dia.
- Their Escape closed the menu. The stack at `menuDidClose` is `startRunningMenuEventLoop:` → `_performPostTrackingDismissalActions` → `_sendMenuClosed`, with the Escape keyUp as the current event. Nothing in Agent Screen took part, and no activation change happened.
- In Chrome, the menu of a select stayed open for 20 s and more while the user was idle. It closed within 0.6 s of the user's next keyDown or left mouse down (`CGEventSourceSecondsSinceLastEventType`), 2 of 2 trials.
- A key posted to the app's pid with `CGEventPostToPid` neither reaches nor closes the menu. The loop takes HID input only.

So the close comes from the menu's own tracking, which owns the session's keyboard and watches for mouse-downs outside it. SkyLight has no public or known private switch that lets a process track a menu without that. The bigger problem is the other direction: a menu an agent leaves open takes the user's keystrokes. A letter goes to the menu's type-ahead, Return picks an item, Escape closes it, and the user's app never gets the key. That breaks ADR 0001.

Not measured, because it needs a real click: whether the user's click that closes the menu also reaches the window they clicked.

## What accessibility offers for a select in Chrome

- The closed select is an `AXPopUpButton` whose only child is the selected `AXMenuItem`. Its actions are `AXShowMenu` and `AXScrollToVisible`.
- `AXValue` reports as settable, and setting it returns success, but the select keeps its value.
- `AXShowMenu` opens the page's context menu (Back, Forward, Reload…), not the select's.
- Once a click opens the select's menu, the menu is not in the window's tree. `AXUIElementCopyElementAtPosition` at its window (layer 101) finds it, and `AXPress` on an item chooses it: the page gets `input` and `change`.

## What Agent Screen does

- `act_on_element set_value` on an `AXPopUpButton` opens the menu with a posted click, finds it by its window, presses the item the value names (`agent_screen_model::choice::choose`: exact title, then case-insensitive, then the only one starting with or containing it) and waits for the button to show it. The menu is open 66 to 122 ms. A name that matches none or several fails with the item list and closes the menu (`cargo e2e selectvalue`).
- A click or key meant for a menu that closed since the agent's last call fails and says so (#86, `cargo e2e menuclosed`). The error now points to `set_value`.
- `act_on_element`'s description and `docs/agents/AGENTS.md` tell agents to choose with `set_value`.

## Open

- A menu the agent opens with a click and leaves open still takes the user's keys until it closes. Agent Screen could cancel such a menu (`AXCancel` on the `AXMenu`) once the agent's call that opened it has returned its screenshot, or after a short time. That breaks agents that click items in their next call. It needs a decision.
- Custom dropdowns, such as Google signup's Month and Gender in #79, are in-page listboxes with no `NSMenu`. In Chrome they work by coordinate click, `act_on_element click` and keys.
- Dia: the binary checks for a single instance ("Failed to initialize ADKApplication. A common cause is when another instance of the app is running simultaneously", `singleInstanceAcquired`, `runningApplicationsWithBundleIdentifier:`). Its own data, apart from Chromium's `--user-data-dir`, is under `~/Library/Application Support/Dia`. A second instance beside the user's could refuse to start, hand the launch to the user's instance (a window on their screen), or open their data. It was not started.
- Safari was in use and was not tested. Its select uses the same `NSMenu` tracking.
