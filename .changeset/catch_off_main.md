---
default: patch
---

Launching an app no longer stalls the menu bar icon, Peek and the Viewer for a quarter second or more: Agent Screen watches for the app's new windows on a thread of its own, since registering that watch waits on the app.
