---
default: patch
---

Kept display luma change counting active while an accessibility press check is in flight so `watch_for_changes`'s 2 s safety timer does not drop the display signal when `AXPress` blocks for more than 2 s.
