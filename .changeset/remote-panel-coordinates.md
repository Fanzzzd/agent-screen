---
default: patch
---

Clicks to remote panels drawn by an app's background service now use the target window's current frame when macOS omits it from a targeted window lookup. Before, the event could carry global coordinates in place of window-local coordinates. The fallback checks both the process and window id; it does not use the visible host's frame.
