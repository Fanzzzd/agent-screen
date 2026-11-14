---
default: patch
---

- Creating or ending an Agent Display keeps existing Agent Displays connected in place and shifts windows with any display that must move, so windows stay with the session that placed them and never linger on a hidden spare display.
- Dragging a window in the Viewer keeps tall, wide, sheet-attached, and custom-title-bar windows on the Agent Display instead of letting them jump to your main screen.
- Pop-up menus inside sheets and System Settings panes now support element value selection, background clicks, outside-click menu dismissal, Takeover clicks in the Viewer, and approved foreground input.
- Window-access errors now name the holding client, display, session, idle time, and connection state. When an agent disconnects without closing its displays, Agent Screen waits two seconds for a reconnect and then ends its displays and returns handed-over windows to your screen.
- Pending window, input, and help requests now highlight the approval card, show a pending count on the menu bar icon, and play a notification chime.
- Spare displays reuse the lowest unit numbers first and hold their HiDPI resolution while idle, reducing duplicate virtual display retries and keeping Peek streams warm while preserving capture failure backoff.
