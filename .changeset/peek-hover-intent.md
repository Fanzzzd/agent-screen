---
default: patch
---

Clicking the menu bar icon after hovering now pins Peek instead of closing it. A later click closes the pin. Hover opening uses a 300 ms dwell, and an unpinned Peek uses a 500 ms grace period after the pointer leaves both the icon and Peek. A press keeps its original intent while hover timers run, so its release cannot turn a close into a reopen.
