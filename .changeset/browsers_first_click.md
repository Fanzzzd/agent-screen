---
default: patch
---

# Browsers take the first click, open URLs in the new window, and choose select items

- `launch_app` with a URL now returns once the page has loaded, so a click right after it reaches a page whose scripts have run. An app started by `launch_app` gets a click on its top edge first, because its first click only makes its window key; the agent's first click now lands.
- A click on an item of a web page's select menu chooses it. The menu follows the real pointer, so the click now presses the menu item through Accessibility.
- `launch_app` starts Safari. It looked for Safari at its symlinked path and reported that it did not start.
- A URL opened in a running Safari goes to the new window on the display, through its address field. Before, Safari opened it in the window it had last made active.
- `get_window_state` reads web pages in Chrome. Chrome ignores the switch Dia and Electron apps answer, and builds its page tree only for the one VoiceOver uses; Sidescreen sets that one, and turns it off while it moves a Chrome window, which it slows.
