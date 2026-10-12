---
default: patch
---

# Windows you make in an agent's app stay yours

A window you open in an app the agent started, while that app is in front or after you took one of its windows back, is never listed by `list_windows` with `strays: true` and never moved onto the agent's display later. A `file://` link opened in a browser gets a window of its own on the agent's display again, and `file://` links with `#` or `?` work.
