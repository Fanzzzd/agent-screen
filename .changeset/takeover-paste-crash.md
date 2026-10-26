---
default: patch
---

Agent Screen no longer quits when you click a field in the Viewer and paste into it during a Takeover. If the Viewer gets redrawn while it is still handling your click or key, it now finishes handling the input first and then applies the new layout.
