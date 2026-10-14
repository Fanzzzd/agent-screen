---
default: patch
---

# Clicks no longer crash Electron and Chrome pages, and drags start on the first move

Sidescreen posted input without a time. A click that changed a page's URL, as in most single-page apps, made Chromium kill the page. Every event now carries the time it is sent. A press just outside a resizable window's edge now resizes the window, and the first `mouse_move` after `left_mouse_down` now moves the drag. A screenshot an action returns now waits for what the app draws a moment after the action, also when the display was idle before.
