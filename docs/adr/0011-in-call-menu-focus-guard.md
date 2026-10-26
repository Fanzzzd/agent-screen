---
status: proposed
---

# Proposed in-call menu focus guard

An AppKit menu steals the session's key focus and captures clicks and scrolls outside its window. A menu left open between tool calls can take the user's typing or close before the agent chooses. The user approved the design direction. The runtime below remains Proposed until its bounds and recovery paths have been validated. The proposed runtime would keep menus inside one call and protect input while they track. It uses private SkyLight calls and a tap on the user's front app because per-process event posting alone cannot prevent a menu's session-wide capture.

**Implementation status.** Closed pop-up type-ahead and combo confirmation are implemented separately. `crates/model/src/menu_guard.rs` defines queue ordering and cancellation rules. An isolated helper prototype is available only in debug e2e mode with an owned stand-in and an explicit live-validation flag. Ordinary tool calls do not use it. Its native validation session stopped before a helper started because the fixtures published unrealized window IDs. Runtime interception, menu snapshots and reopen-and-pick remain Proposed. This ADR does not claim current menu calls meet those requirements.

## Avoid opening a menu

For a pop-up button or HTML select, focus the closed control through AX, type the full item title through per-process events, and verify its value. Type-ahead cannot distinguish equal titles and may change the choice before verification fails. Let an earlier search expire before typing another title. For a combo box, set `AXValue` and perform `AXConfirm`. Menu bar items keep the existing AX press path unless the Electron mouse-event path is verified.

## Measured closed-control duration

Closed-control selection currently waits 1.2 s for an earlier type-ahead search to expire. The last Chromium checks took 1303 ms and 1312 ms after startup. These are total tool-call durations, not input-interception durations. This path installs no HID or per-pid tap and holds no user events. It does not implement or demonstrate the proposed 500 ms interception target. Native type-ahead can also defer its committed value, so verification may wait longer.

## Proposed menu transaction

The proposed tool path would return a numbered item list from a menu-opening click and close the menu before returning. A later item action reopens it and chooses within that same call. Store the opener, owning window and item title path. Revalidate all of them when reopening. Fail rather than press a different or ambiguous item when the menu changes. Pull-downs, context menus and menus opened by an ordinary click follow the same rule.

The proposed target is at most 500 ms of input interception. This is not a measured bound or a guarantee of the current code. Start an independent watchdog before any action that might open a menu. Reserve time for cancellation instead of waiting for the budget to expire. An AX action can block, so the watchdog must not share its executor or wait behind it. Close the menu before replaying mouse input, including on errors, timeout, cancellation or a dropped request.

## Proposed focus release

Before opening, refuse if secure input is enabled, key focus cannot be identified, a required private symbol is missing, or any required tap cannot run. Serialize menu calls across Agent Displays.

1. Install an active HID tap before posting the opening action. Hold hardware keys and modifier changes until key focus returns. Hold hardware button presses, releases and scrolls for the whole menu call. Pointer moves pass.
2. Tap the user's front app to suppress the lost-key-focus notice caused by this menu transaction. Tap the menu app to suppress the notice caused by Agent Screen's temporary steal, which would otherwise close the menu. Match event type 21, subtype field 64 equal to `0x4000`, and thief pid field 73. Pass unrelated focus notices.
3. Once the menu window is on screen and its process demonstrably holds key focus, call `SLPSStealKeyFocusReturningID` on Agent Screen's own connection to obtain N. Call `SLPSReleaseKeyFocusWithID(N)`, then `SLPSReleaseKeyFocusWithID(N-1)`. The release function takes one argument. Verify that key focus returned to the app recorded before opening. Never scan older theft IDs after a failure.
4. Replay held keys in order after focus returns. Keep buttons and scrolls until menu closure. A later key waits behind an earlier held mouse event so input cannot reorder across event kinds.
5. Close the menu and verify focus return. Drain the remaining queue through the normal input path, preserving system shortcuts, modifier state and the user's input method. Finish only after the tap acknowledges the replays. New hardware input waits behind pending replays.

Production taps hold only events whose poster pid is 0. Agent Screen's events and events posted by other tools pass unchanged. Replays need the guard's poster pid and a per-transaction nonce so they pass the tap and cannot be mistaken for new hardware input. Test-only marked events may stand in for hardware only when the test has verified that input can reach solely the stand-in process it started.

## Recovery custody prototype

The helper process owns captured event originals independently of its requesting pipe. The queue retains originals after posting a copy and removes them only at the helper's final annotated tap acknowledgement. That acknowledgement is an OS hand-back observation, not proof that an app consumed the key. Native receiver measurements are still required.

Controller panic and parent EOF enter cancellation and drain. If automatic restoration cannot be verified, the helper writes a complete, synced private checkpoint before relinquishing its in-memory originals. The recovery directory uses mode 0700 and event files use mode 0600. Records contain serialized input, so they are not app logs and their payloads are never returned to agents. Each helper has one canonical recovery transaction. Finish with retained input atomically replaces its live journal with the current remaining events. The fallback uses the same transaction and path; temporary checkpoints are excluded from the inventory. Already acknowledged events and their old ACK history do not reappear as another pending record. No journal-reading path posts input.

Ambiguous dispatch is not retried automatically. The summary CLI reports counts only; owner-facing recovery UX is not implemented yet.

The debug prototype has a two-second recovery observation limit. Its custody writer can wait longer when storage fails. Neither limit proves a 500 ms input-interception bound. Target hangs, helper hard termination, global tap revocation, blind secure-input intervals, cross-window mouse replay and complete drag semantics remain unverified. These gaps block enabling ordinary tool interception.

## Failure modes and release gates

- The session-wide theft counter can race with another focus change. Check the known menu process before stealing and after releasing N. If the observations disagree, cancel the menu. Releasing an unrelated ID is unacceptable.
- Secure input can turn on after the initial check and hide keys from a tap. Cancel as soon as it changes. This race needs a measured failure policy before runtime release; an initial check does not prove zero lost keys.
- Out-of-process menus can belong to `TrustedUIService`, not the app. Verify the menu process and install its protection before attempting a theft release. Do not infer ownership from layer 101 alone.
- A crashed process loses its in-memory queue. A disabled tap or blocked AX endpoint can prevent cleanup within the budget. The runtime must prove its watchdog and cleanup path; it may not silently discard input or report success while a menu still tracks.
- Replayed mouse events can carry an old position after the pointer has moved. Prove that replay preserves the user's pointer and routes the delayed input as intended. Pointer warps, activation and workspace changes remain forbidden under ADR 0001.
- An Electron menu bar action closes #39 only after a posted click inside the guarded menu reaches its handler with `triggeredByAccelerator=false` while a text field has focus. An AX press is not that proof.

Before enabling the runtime, measure keys lost, event order, maximum added latency, correct item choice and whether the stand-in user's window stays key. Cover native pop-ups, pull-downs, context menus, Chromium and Electron selects, and Electron menu bar items. Start both test apps through `launchApp` on the same Agent Display. Send no test input to an app the test did not start. Keep the display lock and check for leftover processes and windows after each run.

## Alternatives

Leaving a menu open gives it the user's keyboard until the next call. Routing the user's keys directly to a pid bypasses session shortcuts and window activation. Holding every key until closure delays typing for the entire call. A separate login session or VM isolates input but has none of the user's current apps or logins. The proposed guard would keep existing apps available and limit the initial key hold, at the cost of private API and event-tap failure modes.

## Fixture unexpected-input custody

The owned native fixture retains the original NSEvent and CGEvent before serialization. Untagged input is never replayed to a real user app. Nil serialization, failed open, short or failed write, fsync, close and rename failures keep the in-memory owner. Per-event files become authoritative only after complete writes, file sync, atomic rename and directory sync.

The fixture stops emitting and releases its own focus lease on unexpected input. While custody cannot become durable, it refuses Quit, SIGTERM and its normal 55-second exit. It retries storage without discarding the event. This deliberate pending-recovery exception can keep the fixture alive beyond its normal test lifetime and must be reported rather than force-killed. Hard termination and allocation failure remain outside the proof. Offline fault injection constructs events but never starts NSApp, opens a window, installs a tap or posts input.
