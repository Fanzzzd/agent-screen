---
default: patch
---

Moved blocking WindowServer IPCs off the main UI thread: the Input Queue HID event tap and its observer recovery now run on a background run-loop thread, screen lists and display names are cached between display changes, and Viewer/end-display window counts and mode checks run in the background.
