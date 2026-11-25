---
default: patch
---

Agent Displays get their own wallpaper on a first install too. Agent Screen now draws the picture itself, at each display's size and with the display's name, and keeps it in its Application Support folder, so it no longer depends on a picture file inside the app or on where the app was opened from. The old bundled picture still said Sidescreen; the drawn one says the display's name, which is how System Settings tells agent displays apart from your monitors. A display that macOS listed late gets its picture at the next screen change instead of going without one. A wallpaper you chose with Choose… stays as it was.
