---
status: accepted
---

# Agent menus close within one call and protect the user's input

An AppKit menu steals the session's key focus and captures clicks and scrolls outside its window. A menu left open between tool calls can take the user's typing or close before the agent chooses. The approved design keeps menus inside one call and protects input while they track. It uses private SkyLight calls and a tap on the user's front app because per-process event posting alone cannot prevent a menu's session-wide capture.

**Implementation status.** Closed pop-up type-ahead and combo confirmation are implemented separately. `crates/model/src/menu_guard.rs` defines queue ordering and cancellation rules. The runtime focus guard, menu snapshots and reopen-and-pick are not implemented yet. This ADR records their requirements, not a claim that current menu calls meet them.

## Avoid opening a menu

For a pop-up button or HTML select, focus the closed control through AX, type the full item title through per-process events, and verify its value. Type-ahead cannot distinguish equal titles and may change the choice before verification fails. Let an earlier search expire before typing another title. For a combo box, set `AXValue` and perform `AXConfirm`. Menu bar items keep the existing AX press path unless the Electron mouse-event path is verified.

## One call owns an open menu

A click that opens any other menu returns a numbered item list and closes the menu before returning. A later item action reopens it and chooses within that same call. Store the opener, owning window and item title path. Revalidate all of them when reopening. Fail rather than press a different or ambiguous item when the menu changes. Pull-downs, context menus and menus opened by an ordinary click follow the same rule.

The call has a 500 ms input-protection budget. Start an independent watchdog before any action that might open a menu. Reserve time for cancellation instead of waiting for the budget to expire. An AX action can block, so the watchdog must not share its executor or wait behind it. Close the menu before replaying mouse input, including on errors, timeout, cancellation or a dropped request.

## Release the menu's focus theft

Before opening, refuse if secure input is enabled, key focus cannot be identified, a required private symbol is missing, or any required tap cannot run. Serialize menu calls across Agent Displays.

1. Install an active HID tap before posting the opening action. Hold hardware keys and modifier changes until key focus returns. Hold hardware button presses, releases and scrolls for the whole menu call. Pointer moves pass.
2. Tap the user's front app to suppress the lost-key-focus notice caused by this menu transaction. Tap the menu app to suppress the notice caused by Agent Screen's temporary steal, which would otherwise close the menu. Match event type 21, subtype field 64 equal to `0x4000`, and thief pid field 73. Pass unrelated focus notices.
3. Once the menu window is on screen and its process demonstrably holds key focus, call `SLPSStealKeyFocusReturningID` on Agent Screen's own connection to obtain N. Call `SLPSReleaseKeyFocusWithID(N)`, then `SLPSReleaseKeyFocusWithID(N-1)`. The release function takes one argument. Verify that key focus returned to the app recorded before opening. Never scan older theft IDs after a failure.
4. Replay held keys in order after focus returns. Keep buttons and scrolls until menu closure. A later key waits behind an earlier held mouse event so input cannot reorder across event kinds.
5. Close the menu and verify focus return. Drain the remaining queue through the normal input path, preserving system shortcuts, modifier state and the user's input method. Finish only after the tap acknowledges the replays. New hardware input waits behind pending replays.

Production taps hold only events whose poster pid is 0. Agent Screen's events and events posted by other tools pass unchanged. Replays need the guard's poster pid and a per-transaction nonce so they pass the tap and cannot be mistaken for new hardware input. Test-only marked events may stand in for hardware only when the test has verified that input can reach solely the stand-in process it started.

## Failure modes and release gates

- The session-wide theft counter can race with another focus change. Check the known menu process before stealing and after releasing N. If the observations disagree, cancel the menu. Releasing an unrelated ID is unacceptable.
- Secure input can turn on after the initial check and hide keys from a tap. Cancel as soon as it changes. This race needs a measured failure policy before runtime release; an initial check does not prove zero lost keys.
- Out-of-process menus can belong to `TrustedUIService`, not the app. Verify the menu process and install its protection before attempting a theft release. Do not infer ownership from layer 101 alone.
- A crashed process loses its in-memory queue. A disabled tap or blocked AX endpoint can prevent cleanup within the budget. The runtime must prove its watchdog and cleanup path; it may not silently discard input or report success while a menu still tracks.
- Replayed mouse events can carry an old position after the pointer has moved. Prove that replay preserves the user's pointer and routes the delayed input as intended. Pointer warps, activation and workspace changes remain forbidden under ADR 0001.
- An Electron menu bar action closes #39 only after a posted click inside the guarded menu reaches its handler with `triggeredByAccelerator=false` while a text field has focus. An AX press is not that proof.

Before enabling the runtime, measure keys lost, event order, maximum added latency, correct item choice and whether the stand-in user's window stays key. Cover native pop-ups, pull-downs, context menus, Chromium and Electron selects, and Electron menu bar items. Start both test apps through `launchApp` on the same Agent Display. Send no test input to an app the test did not start. Keep the display lock and check for leftover processes and windows after each run.

## Alternatives

Leaving a menu open gives it the user's keyboard until the next call. Routing the user's keys directly to a pid bypasses session shortcuts and window activation. Holding every key until closure delays typing for the entire call. A separate login session or VM isolates input but has none of the user's current apps or logins. The focus guard keeps existing apps available and limits the initial key hold, at the cost of private API and event-tap failure modes.
