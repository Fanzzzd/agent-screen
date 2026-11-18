---
default: patch
---

The Agent Screen tools now run inside the app. `AgentScreen mcp`, the process each agent session starts, only passes the session's messages to the app, so an update reaches every session at its next tool call instead of waiting for each client to restart its MCP connection. When the app restarts, a session's next call reconnects, and the agent's client is asked to load the tool list again. Sessions started before this version keep working with their old MCP process until their client restarts it.

Agent Screen now starts when an agent's client connects, not at the agent's first tool call. `AgentScreen mcp` also ends when the client that started it exits, even if the client left its input open, and the app ends any older MCP process whose client has exited and notes it in its log. An MCP process whose client still runs is never ended.

`get_diagnostics` now reports the build of the session's `AgentScreen mcp` beside the app's.
