---
default: patch
---

# A new display no longer makes every other display move

When a monitor you unplugged left a gap in macOS's display numbers, every new agent display came out with a number another display had, so Agent Screen made it twice and removed the first. Each of those changes moved all the other displays and could drop their windows on your screen. Agent Screen now keeps that first display, out of the way, until its last display goes, so each new display is one change.
