---
default: patch
---

Every MCP client now gets its own Agent Session, with its own displays. Before, a client started from a Claude Code terminal, such as Gemini CLI, inherited Claude Code's session variable and shared that Claude Code session's displays. Now Agent Screen trusts that variable only from Claude Code, and a client that names no session gets one per `AgentScreen mcp` process it starts. Codex still keeps each conversation apart. [docs/clients.md](https://github.com/Fanzzzd/agent-screen/blob/main/docs/clients.md) has the setup for Cursor, Gemini CLI, OpenCode, Claude Desktop, VS Code, Zed and other clients.
