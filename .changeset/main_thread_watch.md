---
default: patch
---

# Agent Screen logs when it stops responding

When Agent Screen's main thread, which draws the menu bar icon, Peek and the Viewer, doesn't answer for 150 ms or more, `~/Library/Logs/Agent Screen/displays.log` says so beside the display changes. A stall still going at 300 ms also saves a one-second sample of Agent Screen to that folder, at most once every ten minutes, keeping the last five.
