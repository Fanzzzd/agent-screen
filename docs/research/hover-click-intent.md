# Peek hover and click intent

Research date: 2026-10-27. Desk research only. No GUI tests or implementation changes.

## Recommendation

Give Peek explicit `Hidden`, `Hover`, and `Pinned` states. A click while Peek is in `Hover` must promote it to `Pinned`. Only a click while already `Pinned` should close it. Visibility alone cannot determine click intent.

Start testing with a **300 ms hover-open delay** and a **500 ms leave grace period**. Clicks bypass both delays. These are proposed starting values, informed by web UX research. Apple does not prescribe a universal macOS hover threshold in the sources below.

Fix the state transition independently of timing. Increasing the open delay can reduce how often the current bug occurs. It cannot prevent a deliberate click from closing a hover-opened Peek after a longer dwell.

Here, `Pinned` means pointer exit no longer closes Peek. It does not imply always-on-top persistence across every outside click or app interaction. Keep explicit dismissal separate from hover dismissal.

## What the sources establish

### Apple guidance and AppKit contracts

[Apple HIG, Popovers](https://developer.apple.com/design/human-interface-guidelines/popovers) says a popover generally closes when people click outside it or select an item. For multiple selections, it should remain open until explicit dismissal or an outside click. Its macOS section describes detachable popovers that become panels and remain visible while people interact elsewhere.

This supports distinguishing temporary and persistent presentation. It does not specify hover-to-pin behavior, click-toggle rules, or millisecond thresholds. Peek uses a custom panel, so NSPopover behavior is a reference contract, not an implementation requirement.

AppKit provides distinct dismissal policies:

- [`transient`](https://developer.apple.com/documentation/appkit/nspopover/behavior-swift.enum/transient) closes on interaction outside the popover. Apple explicitly says, "The exact interactions that will cause transient popovers to close are not specified." Menus and panels that become key only when needed have exceptions.
- [`semitransient`](https://developer.apple.com/documentation/appkit/nspopover/behavior-swift.enum/semitransient) closes on interaction with UI in the window containing its positioning view. This is not a synonym for hover-only or pinned.
- [`applicationDefined`](https://developer.apple.com/documentation/appkit/nspopover/behavior-swift.enum/applicationdefined) makes the app responsible for closing. AppKit still closes it in some circumstances. Apple suggests considering Escape handling.

[`NSStatusItem`](https://developer.apple.com/documentation/appkit/nsstatusitem) directs developers to its `button` property for behavior customization. [`NSControl.sendAction(on:)`](https://developer.apple.com/documentation/appkit/nscontrol/sendaction(on:)) and [`NSCell.sendAction(on:)`](https://developer.apple.com/documentation/appkit/nscell/sendaction(on:)) expose mouse-down and mouse-up conditions. They provide event dispatch, not an intent model. Registering both phases and toggling on each risks two transitions for one press.

The Apple pages were verified through Apple's documentation JSON because the rendered pages supplied little text. Examples are the [HIG data](https://developer.apple.com/tutorials/data/design/human-interface-guidelines/popovers.json) and the [NSControl action data](https://developer.apple.com/tutorials/data/documentation/appkit/nscontrol/sendaction(on:).json).

### Published macOS app source

**Stats** explicitly dispatches its status button action on `.leftMouseDown` and `.rightMouseDown`. Its custom widget also implements `mouseDown`. See [widget.swift, lines 230–235 and 355–357](https://github.com/exelban/stats/blob/69efd1c0d581af80c27d3fbe23d62b729588cbff/Kit/module/widget.swift#L230-L235).

Stats also stores a separate `locked` state. Moving its panel sets `locked = true`; losing key status closes the panel only when it is unlocked. See [popup.swift, lines 80–129](https://github.com/exelban/stats/blob/69efd1c0d581af80c27d3fbe23d62b729588cbff/Kit/module/popup.swift#L80-L129).

**Maccy** assigns a status-button action and calls `panel.toggle` from that action. The inspected setup does not override the event mask. See [AppDelegate.swift, lines 10–19](https://github.com/p0deje/Maccy/blob/f0b571e32e4031d7b91ba811f50d0f33e8e5548b/Maccy/AppDelegate.swift#L10-L19) and [the action, lines 183–202](https://github.com/p0deje/Maccy/blob/f0b571e32e4031d7b91ba811f50d0f33e8e5548b/Maccy/AppDelegate.swift#L183-L202).

These examples show click-driven presentation and separately modeled persistence. Neither inspected path establishes hover-to-pin as an industry-wide macOS convention. Stats demonstrates that mouse-down dispatch is used in a maintained menu-bar app. Maccy shows that explicit mouse-down dispatch is not universal.

### Hover research, with its limits

[Nielsen Norman Group, "Timing Guidelines for Exposing Hidden Content"](https://www.nngroup.com/articles/timing-exposing-content/), Aurora Harley, 2015-01-11, distinguishes deliberate click/tap from ambiguous hover. It recommends:

- Visual feedback within 0.1 seconds.
- For hover, "Wait 0.3–0.5 seconds." Reveal content within another 0.1 seconds if the pointer remains stationary in the target.
- Retain content until the pointer has been outside both the trigger and revealed content for longer than 0.5 seconds.
- For click/tap, start revealing content within 0.1 seconds. Do not impose the hover-intent wait.

The article discusses hidden web content such as menus, tooltips, product previews, and expanding ads. Its stationary-pointer condition is stricter than a simple enter timer. These recommendations are not measurements of macOS status items.

[Baymard, "Provide a Hover Delay of 300–500 MS for Hover-Based Drop-Down Menus"](https://baymard.com/blog/dropdown-menu-flickering-issue), Edward Scott, 2023-01-31, reports that a 300–500 ms delay prevents most accidental menu activation. It also recommends pointer-path analysis for diagonal movement toward submenus. The [current article URL](https://baymard.com/research-articles/dropdown-menu-flickering-issue) contains the same research topic.

Baymard's context is desktop ecommerce navigation. Its public article cites Product Finding usability research and benchmarking, not a controlled macOS experiment. Its 60% figure concerns benchmarked sites lacking suitable hover delay. It is not a user failure rate. The article does not provide enough experimental detail to derive a unique optimal threshold for Peek.

## State and event rules

| State before deliberate icon activation | Result |
| --- | --- |
| Hidden, including pending hover-open | Cancel hover-open and open Pinned immediately |
| Hover | Promote to Pinned without hiding or replaying the opening animation |
| Pinned | Close and suppress hover reopening until the pointer leaves the icon |

Pointer exit schedules dismissal only in `Hover`. Entering the icon, Peek, or the short connecting gap cancels that dismissal. Right-click remains a separate menu action. Keyboard and accessibility activation should use the same logical activation rules without requiring mouse events.

Do not infer close intent from how recently Peek opened. A click after either a short or long hover should pin it. A separate later click on Pinned should close it without an arbitrary minimum-visible interval.

### Mouse-down versus mouse-up

Capture the intended action at **mouse-down**, before timers or outside-click dismissal can change the state. Choose one commit policy:

1. Commit at mouse-down. This matches Stats' explicit status-button dispatch. Mouse-up must not toggle again.
2. Preserve release-to-activate behavior. Store `OpenPinned`, `PromotePinned`, or `Close` at mouse-down. Commit that stored action once on a valid release. Cancel it on an aborted press or drag according to the control's tracking behavior.

Prefer the second policy if Agent Screen keeps its existing mouse-up action. Changing the action mask alone does not resolve every timer and monitor race.

For either policy:

- Suspend hover opening and hover dismissal during the icon press. Invalidate pending callbacks when the press begins or presentation mode changes.
- Give the press one action owner. A mouse monitor and the button action must not both toggle it.
- Exclude a press on the icon from outside-click dismissal. Otherwise mouse-down can close Pinned and mouse-up can reopen it.
- Check generation and current mode inside timer callbacks. Canceling a timer is insufficient if its callback is already queued.
- After an explicit close under a stationary pointer, require icon exit and re-entry before hover can open again.

These rules are engineering recommendations derived from the event model and Peek's promised behavior. Apple and the inspected apps do not publish this exact state machine.

## Timing proposal and validation

| Mechanism | Initial value | Rationale |
| --- | --- | --- |
| Hover-open | 300 ms continuous dwell within the icon | Lower end of both web recommendations. Compare 400 and 500 ms if accidental openings persist. Test pointer jitter rather than requiring exact coordinate equality. |
| Hover dismissal | 500 ms continuously outside icon, Peek, and connecting gap | Based on NN/g's leave tolerance. Compare a shorter value only if measured dismissal feels slow. |
| Click activation | No intentional wait; start visible feedback within 100 ms | Explicit activation supplies intent. 100 ms is the NN/g target, not measured Agent Screen latency. |

Use deadlines on a monotonic clock. Measure actual presentation times. A 100 ms polling loop adds detection and scheduling delay; the constant in code is not the observed delay.

At research time, `apps/macos/src/ui/shell.rs` opens after 120 ms and checks pointer departure every 100 ms against a 280 ms threshold. The observed close deadline can therefore be roughly 300–400 ms after departure before scheduling delays. Its icon-click branch explicitly hides Peek whenever `peek_shown` is true, including hover-opened Peek. `apps/macos/src/mac/native.rs` dispatches status-button actions on mouse-up and reports mouse-down separately. This is a code-based explanation, not a GUI reproduction.

Before choosing final values, run these checks:

- Fake-clock state tests at open/close deadlines minus 1 ms, exactly the deadline, and plus 1 ms. A completed first activation from Hidden or Hover must always end Pinned.
- Press before the hover deadline, release after it. Fire stale timer callbacks after pinning and closing. No callback may change Pinned or reopen explicitly closed Peek.
- Exercise both press policies. Include canceled presses, drag-out, duplicate monitor delivery, and icon presses seen by outside-click handling.
- Traverse the icon-to-Peek gap diagonally. Leave and re-enter during the grace period. Peek must remain available throughout the intended path.
- Verify a later activation closes Pinned and does not immediately reopen under the stationary pointer. Verify keyboard and accessibility activation.

For human testing, compare 300, 400, and 500 ms entry delays with a fixed 500 ms leave delay. Record accidental opens per pass over the icon, time from deliberate dwell to first visible content, and first-click pin failures. Include long hovers before clicking. Require zero first-click pin failures in the deterministic cases. Select the shortest entry delay that avoids accidental openings in the tested tasks. Report participant count and raw outcomes; no user study ran for this report.

## Confidence

**High** for Apple's documented API contracts, the cited app source, and the failure in the inspected Peek click branch. **Moderate** for transferring hover-delay research from web navigation to Peek. **Unvalidated** for the proposed 300/500 ms defaults and user preference between press policies. The evidence supports explicit presentation state and one transition per activation. It does not establish a universal macOS hover delay.
