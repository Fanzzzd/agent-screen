---
default: patch
---

# Which windows of an agent's app are the agent's

A new window of an app the agent started goes to the agent's display only when it appears within 3 seconds of the agent acting on that display. Bringing the app to the front no longer decides it, so the agent's own late window can't get stuck on your screen. After you take back a window of the app, `place_window` says the app is yours now. A `file://` link with `#` or `?` opens its file in apps other than browsers, and a missing file is reported before anything opens.
