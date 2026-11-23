---
default: patch
---

When two agents launch the same running app at once, each now gets its own window. The second agent's new window opens on your screen for a moment before Agent Screen moves it, and the first agent's display, still within its 30 s start-up watch of that app, took it as its own. `launch_app` then failed for the second agent, saying the window could not be placed. A display that is launching the app now counts as a possible maker of the window, so the other display leaves it to that launch.
