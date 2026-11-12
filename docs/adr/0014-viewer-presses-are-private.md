# The Viewer's presses are private

Accepted, 2026-11-12.

A press the user makes in the Viewer is a real mouse event. WindowServer routes it to Agent Screen, and it also reaches every other app's NSEvent global monitor as a press outside that app. Qt closes a pop-up on such a press while its parent isn't active (qtbase 5.15, `qcocoawindow.mm`, "Close popup when clicking outside it"). An app on an Agent Display is never active. So when the user clicked a contact in WeCom's search results through the Viewer, the list closed before Agent Screen hit-tested the click, and the click went to the chat row under the list.

Agent Screen takes those presses before WindowServer routes them. An active HID event tap, inserted after the input observer of ADR 0013 so it runs first, takes a press only where the Viewer's picture is the window in front at the point, and only when the Viewer's handler takes it. It hands the press to that handler and drops the original. The drags and the release of a press it took go the same way, wherever the pointer goes meanwhile. Every other event passes unchanged: presses on the Viewer's header, on any other window, on a blocked Viewer, and all keys, moves and scrolls. Input Agent Screen posts for an Input Loan carries ADR 0013's tag and passes too.

The press stays the user's. The tap tells the input observer of ADR 0013 about each press it takes, so a press in the Viewer still revokes an Input Grant. It reports the press to the monitors Peek and Handover by drag read. ADR 0013's observer stays listen-only. The tap suppresses no input the user aimed elsewhere, so this doesn't revive the input suppression ADR 0001 and ADR 0013 rule out.

If macOS disables the tap for a slow callback, Agent Screen enables it again. If the tap can't be made, presses reach other apps as before, and a Qt pop-up on the display closes on them. `AGENTSCREEN_NO_PRIVATE_PRESS=1` leaves the tap out, to tell a problem it causes from one it fixes. `debugViewer` reports `privatePress`. The e2e `viewerpopup` clicks a list that closes on any press in another app, as Qt's does, with the real pointer.

A borderless window has no title bar, so Agent Screen never clicks its top edge to wake it or drags it there as a title bar: a window counts as titled only with a close, minimize, zoom or full screen button. Waking the list that way picked its first item.
