---
default: patch
---

If Agent Screen quits while an agent works, the agent's next call says the app quit instead of "does not exist". After create_display, the windows of apps it launched on the old display are listed as strays, and place_window brings them back. A crash leaves its message and backtrace in displays.log.
