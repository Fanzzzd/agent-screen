---
default: patch
---

With the AeroSpace build for Agent Screen, a window that leaves an Agent Display now stays claimed until it is on your screen. Before, Agent Screen released the window first and then moved it. If the move failed, the window stayed on a display you can't see, and AeroSpace, which had taken it back once before, left it there. Now a window that doesn't get off keeps its claim and stays the agent's.
