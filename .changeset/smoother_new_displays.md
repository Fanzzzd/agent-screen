---
default: patch
---

# Peek and the Viewer keep responding while agents make displays and start apps

Parking a new display waited on macOS for 100 ms or more on Sidescreen's main thread, and finding a starting app asked macOS about every running app 50 times a second. Both stalled the menu bar icon, Peek and the Viewer. Display changes now run on a thread of their own, one at a time, and Sidescreen finds a starting app in the process table.
