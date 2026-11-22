---
default: patch
---

When the displays change, for example as Agent Screen makes a spare display while you are away, an agent's window that macOS moves off its Agent Display is now put back reliably. Agent Screen used to try twice and trust the answer, so a window whose move failed once stayed off its display. A window that landed on a spare display stayed there, out of sight, while the agent still saw it listed on its own display. Agent Screen now checks where each window really is several times while the displays settle, and tries again each time. A window it can't bring back goes to your screen rather than staying lost on a hidden display. A window its app closed meanwhile is noted in the log.
