---
default: patch
---

The screenshot after an action no longer shows the display too early when the agent works quickly. Sidescreen now counts the display's changes from the action on, and waits half a second from the action, then until the display has been still for 120 ms. Before, an app that drew its result a moment after a first quick change, such as a page that updates a counter at once and redraws 400 ms later, could be caught before the redraw.
