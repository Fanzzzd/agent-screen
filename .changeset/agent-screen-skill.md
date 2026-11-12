---
default: patch
---

Agents now use their Agent Display on their own when a task needs a screen. Before, Claude Code often opened apps with `open -a` or AppleScript on your screen unless you told it to use Agent Screen, since it loads MCP tools only when it searches for them. Three changes fix that. Claude Code now sees `create_display` and what it is for from the start of a session. The server's instructions name the tasks that need a screen and the commands its tools replace. And a new Agent Screen skill tells Claude Code, Codex, Cursor and Gemini CLI when and how to use the tools. Install it with `claude plugin install agent-screen --marketplace Fanzzzd/agent-screen` (Claude Code 2.1.292 or later), or `npx skills add Fanzzzd/agent-screen -g` for other agents (README, Install step 4).
