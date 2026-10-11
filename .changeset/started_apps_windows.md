---
default: patch
---

# Windows of an app the agent started stay on its display

A later window of an app `launch_app` started, such as the extra window TextEdit opens when two sessions start it at once, now goes to the agent's display instead of your screen. `list_windows` with `strays: true` lists such windows that still landed elsewhere, and `place_window` takes only those.
