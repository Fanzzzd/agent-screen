---
default: patch
---

When a display has ended, agents are now told why: the session ended it with `end_display`, the session's connection closed for longer than the 2-second grace (as when a client restarts its MCP server), or you closed it. An agent whose client started a new MCP server learns that the display belonged to its own earlier session, instead of reading that it belongs to another session. Each end also leaves one line with its reason in `displays.log`, and `get_diagnostics` lists the last few ends, so a bug report says why a display went away.
