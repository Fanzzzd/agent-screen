---
default: minor
---

An agent session can now use more than one display at once, up to four, when its task needs two screens side by side. An agent asks for another display only when it needs one; calling `create_display` again still returns the display it has, so agents don't pile up displays. Each display has its own label in Peek and the menu, and a repeated task title gets a number. The agent can move a window between its own displays without asking you, and a window you gave it stays yours wherever it moves. Window access you grant covers all of the session's displays. Ending one display leaves the others running; when the agent disconnects, all of them end. After Agent Screen restarts, an agent gets back only the apps it started itself, never another agent's.
