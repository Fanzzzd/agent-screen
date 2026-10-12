---
default: patch
---

# A window you take back stays yours

A window you take back from an agent is never listed by `list_windows` with `strays: true` again, and `place_window` refuses it, even in an app the agent started. Once an agent ends its display, Sidescreen stops moving that app's new windows onto it.
