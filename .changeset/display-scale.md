---
default: patch
---

create_display and list_windows now return scale, screenshot pixels per display point, so an agent can tell why a window's size in the tools is smaller than the app reports on a large display.
