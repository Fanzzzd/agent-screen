---
default: patch
---

# Screenshots of large displays show the whole display

A `computer` screenshot of a display wider than the image limit used to show only its top-left part, while clicks and `list_windows` used the scaled-down whole. It now shows the whole display scaled down. A zoom over the full screenshot works at the size the screenshot reports, and a zoom whose capture fails for a moment, as after a window was resized, tries again and then cuts the region from a whole capture.
