# MCP clients

Agent Screen works with any MCP client that starts a local stdio server. The server is `AgentScreen mcp`: the command `/Applications/Agent Screen.app/Contents/MacOS/AgentScreen` with the argument `mcp`. It forwards the client's messages to the app, which serves the tools (ADR 0021).

Some calls wait for the user: `wait_for_control`, `request_help`, `request_control` and `place_window` for a user's window wait up to 600 s. Give the client a tool timeout above that, such as 700 s, or those calls fail in the client while the request stays open in the app. When the app isn't running, the first call starts it and can take 15 s. The entries below set both where the client's default is shorter.

## Connect from the command line

`AgentScreen mcp install` registers the server with every client below that this Mac has, by editing the client's own config file. Settings' **Connect All Detected** does the same, and its Agents list shows each client's state with a Connect, Fix Path or Disconnect button. After it, start a new session in the client; Claude Desktop needs a full quit and reopen.

```sh
AS="/Applications/Agent Screen.app/Contents/MacOS/AgentScreen"
"$AS" mcp status [--json]          # each client: connected, stale, not_connected or invalid
"$AS" mcp install [--client cursor,zed] [--no-skill] [--dry-run] [--json]
"$AS" mcp uninstall [--client <id,...>] [--dry-run] [--json]
```

- `status` lists the clients it found, the config file of each, and what the file says: `connected` (it runs this copy), `stale` (it runs another copy, such as one opened from the disk image or Downloads before you moved it; the command it runs is shown), `not_connected`, or `invalid` (the file can't be read). It also says where the skill is and whether it is this version.
- `install` adds the entry, or repoints a stale one. It sets the command and arguments, and adds a timeout the entry lacks; your own keys in the entry, such as `env`, `disabled` or a timeout you chose, stay, and so do other servers, other settings and comments. Running it again changes nothing. `--client` takes the ids in the table, comma-separated, and writes those clients' configs even when it doesn't find the client. `--dry-run` says what would change and writes nothing.
- `uninstall` takes out only Agent Screen's entry, from the named clients or from every client that has it.
- A config file that can't be read is never written: `install` reports it and leaves it as it is. Each write goes to a temporary file in the same folder, then replaces the file, so a crash never leaves half a config; the file keeps its permissions. A config that is a link, as a dotfiles manager makes, is written through the link when it points to a file; any other link is reported and left alone.
- The development build (`Agent Screen Dev.app`) registers as `agent-screen-dev`, beside the installed app's `agent-screen`, and writes no skill.

`install` also writes the Agent Screen skill that matches this version to `~/.agents/skills/agent-screen/SKILL.md`, which Codex and other agents read, and to `~/.claude/skills/agent-screen/SKILL.md` for Claude Code. It leaves a copy alone when the file or any folder above it in your home folder is a link, as a dotfiles manager makes, or when `npx skills` installed it, and writes no Claude Code copy when the agent-screen plugin is installed. Codex gets its own copy in `~/.codex/skills` only when the shared one can't be written. Each update of the app brings the copies it wrote up to its version.

## The clients

| Client | `--client` id | Config file | Entry | Checked against |
| :- | :- | :- | :- | :- |
| Claude Code | `claude-code` | `~/.claude.json`, or `$CLAUDE_CONFIG_DIR/.claude.json` | `mcpServers`: `type: "stdio"`, `command`, `args` | [Claude directory](https://code.claude.com/docs/en/claude-directory) |
| Codex (CLI, IDE extension, app) | `codex` | `$CODEX_HOME/config.toml`, default `~/.codex/config.toml` | `[mcp_servers.agent-screen]`: `command`, `args`, `tool_timeout_sec = 700`, `startup_timeout_sec = 30` | [Codex MCP docs](https://learn.chatgpt.com/docs/extend/mcp?surface=cli) |
| Cursor (editor and CLI) | `cursor` | `~/.cursor/mcp.json` | `mcpServers`: `command`, `args` | this Mac's Cursor config |
| Claude Desktop | `claude-desktop` | `~/Library/Application Support/Claude/claude_desktop_config.json` | `mcpServers`: `command`, `args`; quit and reopen | [Connect to local MCP servers](https://modelcontextprotocol.io/docs/develop/connect-local-servers) |
| Antigravity (app and `agy` CLI) | `antigravity` (also `antigravity-ide`, `antigravity-cli`) | `~/.gemini/config/mcp_config.json`; Antigravity 1 used `~/.gemini/antigravity/mcp_config.json` | `mcpServers`: `command`, `args` | the MCP guide inside Antigravity 2.26 and `agy` |
| Gemini CLI | `gemini-cli` | `~/.gemini/settings.json` | `mcpServers`: `command`, `args`, `timeout: 700000` | [Gemini CLI MCP docs](https://github.com/google-gemini/gemini-cli/blob/main/docs/tools/mcp-server.md) |
| OpenCode | `opencode` | `~/.config/opencode/opencode.jsonc` if it exists, else `opencode.json` (`$XDG_CONFIG_HOME` moves both) | `mcp.servers`: `type: "local"`, `command: [<path>, "mcp"]` | [OpenCode MCP docs](https://opencode.ai/docs/mcp-servers/), [config schema](https://opencode.ai/config.json) |
| VS Code, VS Code Insiders and Copilot CLI | `vscode` (also `vscode-insiders`, `copilot`) | `~/.copilot/mcp-config.json`, or `$COPILOT_HOME/mcp-config.json` | `mcpServers`: `type: "stdio"`, `command`, `args`, `tools: ["*"]` | [VS Code MCP configuration](https://code.visualstudio.com/docs/agents/reference/mcp-configuration) |
| Windsurf | `windsurf` | `~/.codeium/windsurf/mcp_config.json` | `mcpServers`: `command`, `args` | unverified: the current docs describe Devin instead |
| Devin (Desktop and CLI) | `devin` | `~/.config/devin/mcp_config.json` | `mcpServers`: `command`, `args` | [Devin MCP configuration](https://docs.devin.ai/cli/extensibility/mcp/configuration) |
| Zed | `zed` | `~/.config/zed/settings.json` | `context_servers`: `command`, `args`, `env` | [Zed MCP docs](https://zed.dev/docs/ai/mcp) |
| Cline (VS Code extension) | `cline` | `~/Library/Application Support/Code/User/globalStorage/saoudrizwan.claude-dev/settings/cline_mcp_settings.json` | `mcpServers`: `command`, `args`, `timeout: 700` | [Cline MCP docs](https://docs.cline.bot/mcp/configuring-mcp-servers); path and `timeout` unverified |
| Roo Code (VS Code extension) | `roo-code` | `~/Library/Application Support/Code/User/globalStorage/rooveterinaryinc.roo-cline/settings/mcp_settings.json` | `mcpServers`: `command`, `args`, `timeout: 700` | [Roo Code MCP docs](https://roocodeinc.github.io/Roo-Code/features/mcp/using-mcp-in-roo); path unverified |
| Goose | `goose` | `~/.config/goose/config.yaml` (`$XDG_CONFIG_HOME` moves it) | `extensions.agent-screen`: `type: stdio`, `name`, `cmd`, `args`, `enabled`, `envs`, `env_keys`, `timeout: 700` | [Goose config files](https://github.com/block/goose/blob/main/documentation/docs/guides/config-files.md) |

A client counts as found when this Mac has its app in `/Applications` or `~/Applications`, its command line on `PATH` or in an installer's usual folder (such as `~/.local/bin` or `/opt/homebrew/bin`), its config file, or its config folder. Finding one never asks your shell for its `PATH`, so it can't hang on a slow shell profile.

Notes by client:

- **Claude Code.** `install` edits `~/.claude.json` directly instead of running `claude mcp add`, so it works without the CLI on the app's `PATH`. The entry is what `claude mcp add --scope user agent-screen -- <path> mcp` writes. If Claude Code saves the file while `install` writes it, `install` reads it again and retries.
- **Codex** stops a tool call after 60 s and a server start after 10 s by default, hence the two timeouts.
- **Gemini CLI**'s default timeout is 600,000 ms, the same as the longest wait, so a full wait would fail without `timeout`.
- **Cline and Roo Code** stop a call after 60 s by default. Their `timeout` is in seconds; Roo Code accepts up to 3600.
- **Goose** stops a call after 300 s by default. `install` changes only Agent Screen's lines under `extensions:`, then reads the file back; when anything else would change, as with an `extensions` mapping written on one line, it leaves the file for you to edit.
- **OpenCode** 2 reads servers under `mcp.servers`. A config that already lists its servers straight under `mcp`, as earlier versions did, gets the entry there too, with `"enabled": true`, since OpenCode 2 still reads that form.
- **VS Code** now reads Copilot's own file. Its profile `mcp.json` (`servers` root) is deprecated and goes away in February 2027; add Agent Screen there by hand only to keep an existing setup, as `"servers": {"agent-screen": {"type": "stdio", "command": "<path>", "args": ["mcp"]}}`.
- **Zed** before 2025 wrote the command as `"command": {"path": …, "args": […]}`. `status` reads that form; `install` rewrites it to the current one.

## Setup by hand

For a client not in the table, or a project's own config, add a stdio server with that command and the argument `mcp`. Agent Screen needs no environment variables, keys or network access. The common form:

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

Codex:

```toml
[mcp_servers.agent-screen]
command = "/Applications/Agent Screen.app/Contents/MacOS/AgentScreen"
args = ["mcp"]
# wait_for_control can wait up to 10 minutes for the user.
tool_timeout_sec = 700
# A cold start of the app can take 15 s.
startup_timeout_sec = 30
```

OpenCode, in `opencode.jsonc` for a project or the global config:

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

Cursor also reads `.cursor/mcp.json` in a project; its CLI loads a project's server only after `cursor-agent mcp enable agent-screen`.

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

- `cargo test -p agent-screen-mcp --lib install` runs every client in the table in a temporary home folder: found by its app, command line or folder; install, install again, repoint a stale path, keep other servers and the user's keys and comments, refuse a file it can't read, keep `agent-screen-dev` apart, uninstall. It also covers JSON with comments and trailing commas, writes through a link, the skill copies, and the `mcp status | install | uninstall` output.
- `cargo test -p agent-screen-mcp --test clients` drives the real `AgentScreen mcp` binary as Claude Code (its variable), Codex (per-call `_meta`, with Claude Code's variable inherited), Gemini CLI started from Claude Code's shell, and two bare 2026-07-28 clients with no hint. Each lists the tools and calls one, and the test checks every session key the app hears. It also checks each protocol version's handshake and wire shape.
- `AGENTSCREEN_INSTANCE=e2e cargo e2e mcpclients` runs the real clients installed on the Mac against the test app: today Cursor's CLI lists the 18 tools. Clients with no command that lists one server's tools without a model call, or whose listing command also starts the user's other servers, are not run there.
