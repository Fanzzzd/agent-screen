---
default: patch
---

# The Viewer, Peek and agent screenshots never show another display

macOS sometimes gives a new virtual display the same internal number as another display, such as the one AeroSpace keeps hidden windows on. Every picture of the new display then showed the other one. Sidescreen now makes the display again until its number is free. If two displays share a number later, Sidescreen shows no picture of them and says why.
