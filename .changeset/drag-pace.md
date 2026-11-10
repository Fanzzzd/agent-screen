---
default: patch
---

A drag with your pointer, such as a file from Finder onto a web page, now takes about a second instead of several. In 0.1.27 a 580-point drag took almost 8 seconds after you allowed it: it moved 4 points at a time and read every window on your Mac twice before each move. Now the drag moves in 12 to 24 steps, checks the same things before each step with one cheaper read, and waits only as long as the drop needs: half a second for the window to come forward while AeroSpace runs, a short wait for the drag to start, and a moment on the drop point so a web page takes the file. A completed drag tells the agent to say it is done, which brings your pointer back at once. `input_queue status` now reports when each request was asked, allowed, started and ended, so an agent can tell how long you took to answer from how long the drag ran.
