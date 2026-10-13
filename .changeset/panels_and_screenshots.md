---
default: patch
---

# Firefox's file panel, fresher screenshots, web pages in window state

- Firefox shows its file panel at the modal panel level; clicks and keys now reach it too.
- After an action, the screenshot waits up to 500 ms for a first change, so it shows a panel or sheet the action opened instead of the moment before.
- The first `get_window_state` of a Chromium or Electron window waits for the page's accessibility tree, instead of listing only the browser's toolbar.
