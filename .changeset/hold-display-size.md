---
default: patch
---

An Agent Display keeps the size it was made with. When macOS switches it to another mode by itself, as it did under heavy load from 1600×1000 to 800×600, Agent Screen switches it back as soon as macOS reports the change, and notes it in the display log (`~/Library/Logs/Agent Screen/displays.log`).
