---
default: patch
---

# Sidescreen logs when it stops responding

When Sidescreen's main thread, which draws the menu bar icon, Peek and the Viewer, doesn't answer for 150 ms or more, `~/Library/Logs/Sidescreen/displays.log` says so beside the display changes. A stall still going at 300 ms also saves a one-second sample of Sidescreen to that folder, at most once every ten minutes, keeping the last five.
