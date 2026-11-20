---
default: patch
---

`launch_app` into a running app that is busy as its new window shows, as Dia can be, no longer fails with "opened no new window" while that window stays on your screen. Agent Screen waits up to 5 seconds for the app to say what the window is, then moves it to the agent's display. If the app stays busy longer, the agent is told so, and the window still moves to the agent's display once the app answers, unless you use that app first.
