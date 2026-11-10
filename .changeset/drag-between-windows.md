---
default: patch
---

An agent can drag a file from one window to another on its display, such as from Finder onto a web page's upload area. In 0.1.25 the drag failed both ways. In the background it reported success while it only selected the file, since only your pointer starts the system drag session that carries a file. With your pointer it stopped at once after you allowed it, saying the drag was outside its authorized window, because it had to end in the window where it began. Now a drag between two windows asks for your pointer on the approval card instead of claiming it ran. Allowed, it may end in any window on the agent's display and cross the windows between them, but never leaves that display. If the window it drops on moved while you decided, nothing is pressed.
