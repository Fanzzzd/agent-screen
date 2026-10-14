---
default: patch
---

# Dragging a window in the Viewer keeps it on the agent's display

Dragging a window by its title bar in the Viewer moved it to your own screen, where your window manager could tile it. The window now moves on the agent's display, as far as you drag, and stays inside it. This also works for title bars drawn by SwiftUI, such as Calculator's. When an app draws its own title bar, as Electron apps do, a window that a drag carries off the display goes back onto it when you let go.
