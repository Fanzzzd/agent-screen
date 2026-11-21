---
default: patch
---

An app you took back from an Agent Display now stays yours when Agent Screen restarts, as after an update. Before, the agent's next display adopted the app again, and the agent could move your window back onto its display without asking. A display you closed, or one the agent ended, is no longer reported to the agent as ended by the app quitting. And if the agent's first create_display after a restart is refused, its windows that macOS moved to your screen are still handed to its next display.
