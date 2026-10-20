---
default: patch
---

Agent Screen's windows keep responding while a display is added or removed. Each window now draws into a texture of its own, and a separate thread hands finished frames to macOS. Before, waiting on macOS for a frame could freeze the Viewer and Peek for up to a second.
