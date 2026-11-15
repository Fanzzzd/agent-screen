---
default: patch
---

When `place_window` cannot take a window on another Agent Display, the error now names every holding agent, display, session, last activity time, and whether that connection is still open. When an agent's MCP server restarts, Agent Screen keeps its displays for two seconds so the reconnected client resumes them on any tool call instead of losing them right away.
