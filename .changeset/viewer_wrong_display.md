---
default: patch
---

# The Viewer and Peek never show another display's picture

With several virtual displays connected, macOS can send a display's live picture from another one, such as AeroSpace's display for hidden windows. Sidescreen now checks every frame, and when the picture comes from the wrong display it shows screenshots of the right one instead.
