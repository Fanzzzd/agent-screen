---
default: patch
---

A window you handed over goes back to where it came from as soon as the agent ends its display, or its session disconnects. It used to stay on the ended display until you dismissed it there, and the same agent's next `place_window` for that app failed, saying other agent sessions held the window when there were none. The error now says which display holds a window: another agent's, one waiting for your review, or one that isn't the agent's.
