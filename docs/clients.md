# MCP clients

Agent Screen works with any MCP client that starts a local stdio server. The server is `AgentScreen mcp`: the command `/Applications/Agent Screen.app/Contents/MacOS/AgentScreen` with the argument `mcp`. It forwards the client's messages to the app, which serves the tools (ADR 0021).

`AgentScreen mcp install` registers it with Claude Code and Codex. For any other client, add it by hand as below. After adding it, start a new session or restart the client, since clients load MCP servers when they start.

Some calls wait for the user: `wait_for_control`, `request_help`, `request_control` and `place_window` for a user's window wait up to 600 s. Give the client a tool timeout above that, such as 700 s, or those calls fail in the client while the request stays open in the app. When the app isn't running, the first call starts it and can take 15 s. The entries below set both where the client's default is shorter.

## Setup by client

**Claude Code.** `AgentScreen mcp install`, or by hand:

```sh
claude mcp add --scope user agent-screen -- "/Applications/Agent Screen.app/Contents/MacOS/AgentScreen" mcp
```

**Codex: CLI, IDE extension and desktop app.** They share `~/.codex/config.toml`, which `AgentScreen mcp install` writes ([Codex MCP docs](https://learn.chatgpt.com/docs/extend/mcp?surface=cli)). Codex stops a tool call after 60 s and a server start after 10 s by default. Running install again sets only `command` and `args` and adds a missing timeout; your own keys in the entry, such as `env`, `enabled` or tool approvals, stay.

```toml
[mcp_servers.agent-screen]
command = "/Applications/Agent Screen.app/Contents/MacOS/AgentScreen"
args = ["mcp"]
# wait_for_control can wait up to 10 minutes for the user.
tool_timeout_sec = 700
# A cold start of the app can take 15 s.
startup_timeout_sec = 30
```

**Cursor, editor and CLI.** `~/.cursor/mcp.json` for every project, or `.cursor/mcp.json` in one. Cursor's CLI loads a project's server only after `cursor-agent mcp enable agent-screen`.

```json
{
  "mcpServers": {
    "agent-screen": {
      "command": "/Applications/Agent Screen.app/Contents/MacOS/AgentScreen",
      "args": ["mcp"]
    }
  }
}
```

**Gemini CLI.** The same `mcpServers` entry with `"timeout": 700000` added, in `~/.gemini/settings.json`, or `.gemini/settings.json` in a project ([Gemini CLI MCP docs](https://github.com/google-gemini/gemini-cli/blob/main/docs/tools/mcp-server.md)). Gemini CLI's default timeout is 600,000 ms, the same as the longest wait, so a full wait would fail without it.

**Claude Desktop.** The same `mcpServers` entry in `~/Library/Application Support/Claude/claude_desktop_config.json`, then quit Claude Desktop completely and open it again ([Connect to local MCP servers](https://modelcontextprotocol.io/docs/develop/connect-local-servers)).

**VS Code with Copilot.** The same entry with `"type": "stdio"` added, in `~/.copilot/mcp-config.json`, or `.mcp.json` at the workspace root ([VS Code MCP configuration](https://code.visualstudio.com/docs/agents/reference/mcp-configuration)).

**OpenCode.** In `opencode.jsonc` for a project, or the global config ([OpenCode MCP docs](https://opencode.ai/docs/mcp-servers/)):

```jsonc
{
  "mcp": {
    "servers": {
      "agent-screen": {
        "type": "local",
        "command": ["/Applications/Agent Screen.app/Contents/MacOS/AgentScreen", "mcp"]
      }
    }
  }
}
```

**Zed.** Under `context_servers` in Zed's `settings.json` ([Zed MCP docs](https://zed.dev/docs/ai/mcp)):

```json
{
  "context_servers": {
    "agent-screen": {
      "command": "/Applications/Agent Screen.app/Contents/MacOS/AgentScreen",
      "args": ["mcp"],
      "env": {}
    }
  }
}
```

**Any other client.** A stdio server with that command and the argument `mcp`. Agent Screen needs no environment variables, keys or network access.

## Which session a call belongs to

An Agent Session owns its Agent Displays, its Window Authorization and its Help Requests. Agent Screen picks a tool call's session from the first of these that names one (`crates/mcp/src/session.rs`):

1. **The call's own `_meta`.** Codex names its session on every call in `x-codex-turn-metadata`, so one Codex process serving several conversations keeps them apart.
2. **A variable the client sets for its servers, from that client only.** Claude Code passes `CLAUDE_CODE_SESSION_ID` to stdio servers ([Claude Code env vars](https://code.claude.com/docs/en/env-vars)); it counts only when the client says it is Claude Code (`clientInfo.name` `claude-code`). Another client started from Claude Code's shell inherits the variable, and still gets its own session.
3. **The connection.** Every `AgentScreen mcp` process has a random id for its life. A client that gives no hint, as Cursor, Gemini CLI, OpenCode, Zed, VS Code and Claude Desktop do today, gets one session per server process it starts. Two processes never share a session, even when one reuses the other's pid.

How many server processes a client starts decides how much its conversations share. Claude Code starts one per session, and Codex one per conversation. Claude Desktop starts its servers when it launches ([Connect to local MCP servers](https://modelcontextprotocol.io/docs/develop/connect-local-servers)), so all its conversations are one Agent Session with the same displays. For Cursor we have not confirmed whether it starts one per window; the [research](research/mcp-process-model.md) has what is known.

When a client closes its server, by quitting or by restarting the connection, the session's displays wait 2 seconds for it to come back, then end (ADR 0016).

## Protocol versions

The app answers each client in the MCP version the client asks for, from 2024-11-05 to 2026-07-28, using rmcp 3.5:

- A client that sends `initialize` gets the version it named, or 2025-11-25, the newest with `initialize`, when it names a later one. Results carry no `resultType`, and `tools/list` carries no `ttlMs` or `cacheScope`.
- A client on 2026-07-28 sends no `initialize`: each request names its version and client in `_meta`, and `server/discover` describes the server. Results say `resultType: "complete"`, and `tools/list` adds `ttlMs` (one hour) and `cacheScope: "public"`, since the list is the same for every user of a build.

## What the tests cover

- `cargo test -p agent-screen-mcp --test clients` drives the real `AgentScreen mcp` binary as Claude Code (its variable), Codex (per-call `_meta`, with Claude Code's variable inherited), Gemini CLI started from Claude Code's shell, and two bare 2026-07-28 clients with no hint. Each lists the tools and calls one, and the test checks every session key the app hears. It also checks each protocol version's handshake and wire shape.
- `AGENTSCREEN_INSTANCE=e2e cargo e2e mcpclients` runs the real clients installed on the Mac against the test app: today Cursor's CLI lists the 18 tools. Clients with no command that lists one server's tools without a model call, or whose listing command also starts the user's other servers, are not run there.
