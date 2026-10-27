---
default: patch
---

Viewer clicks can be traced with `AGENTSCREEN_VIEWER_CLICK_DIAGNOSTICS=1` at startup. The opt-in stderr records link a press and release to their coordinate mapping, window target, focus handshake and event stamp. They contain no text, window titles, keys or screenshots. Diagnostics are off by default and do not confirm that the target app executed an action.
