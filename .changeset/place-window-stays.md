---
default: patch
---

`place_window` answers `placed` only once the window has stayed on the display for half a second after its last move. When the app keeps moving it back to the user's screen, the call fails and says so. A window of the agent's that leaves the display later, as a UTM VM window did, is listed by `list_windows` with `strays: true` and `place_window` places it again. Before, `place_window` could answer `placed` for a window that was already on its way back, and then refused it, or asked the user for it as if it were theirs.
