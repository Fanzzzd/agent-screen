---
default: patch
---

# Agent keys never reach the app you're using

When the agent's window belongs to the app you're working in and isn't that app's focused window, typing, key presses and held keys stop with a message telling the agent to set text with `act_on_element` instead: macOS would send those keys to your window. Long typing checks again every 20 characters, and Sidescreen re-focuses the agent's window when the app has moved its keys to another of its windows. Select All only selects in the agent's window. On displays larger than 1280×800, window bounds and element positions now come in screenshot pixels, so the agent clicks where it means to. Dragging a tiled window onto the Viewer hands it over again. A takeover lets go of a button the agent held where its drag was, not where you clicked, and a held key is released in the window it was pressed in. Keys typed in the Viewer no longer follow a window you took back.
