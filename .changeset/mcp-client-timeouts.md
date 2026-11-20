---
default: patch
---

Long waits no longer fail in Codex and Gemini CLI. `AgentScreen mcp install` now also gives Codex a 30 s startup timeout, since the app's cold start can pass Codex's 10 s default and Codex then drops the server for the session. Running install again keeps your own settings in the Codex entry, such as `env`, `enabled` or tool approvals, where it used to replace the whole entry; it also finds an entry written as `[mcp_servers."agent-screen"]` instead of adding a second one. [docs/clients.md](https://github.com/Fanzzzd/agent-screen/blob/main/docs/clients.md) now shows the tool timeout for Codex and Gemini CLI configs written by hand, since their defaults (60 s and 600 s) cut off waits for the user. Every `timeout_seconds` now takes a number, so a wait of 1.5 s is no longer refused.
