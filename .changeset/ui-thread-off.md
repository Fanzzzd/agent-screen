---
default: patch
---

Agent Screen's menu bar icon, Peek and Viewer no longer freeze for up to a second and a half while the setup window is open. Checking permissions, following System Settings, counting windows, guarding the cursor and the window lookups behind agents' requests now wait on WindowServer off the UI thread.
