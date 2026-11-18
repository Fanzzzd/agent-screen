# `AgentScreen mcp` is a pipe into the app

Accepted, 2026-11-18. Supersedes the part of [ADR 0005](0005-rust-app-and-mcp.md) that runs the MCP server, its tools and session detection inside `AgentScreen mcp`.

Each client session starts its own `AgentScreen mcp` over stdio. Until now that process held the tool definitions, the tool code and the session detection, and called the app over the control socket. A running process keeps the binary it started from, so an app update did not reach it: on the owner's Mac, 28 of 47 MCP processes ran a replaced build ([research](../research/mcp-process-model.md)). Their agents saw the old tool list and sent old requests to the new app.

## Decision

Stdio stays, one process per client session. A shared HTTP endpoint in the app is out: Claude Code passes its session ID only to stdio servers, in `CLAUDE_CODE_SESSION_ID`, and the MCP security guidance recommends stdio for local servers.

The MCP server moves into the app. `AgentScreen mcp` becomes the pipe (`crates/mcp/src/pipe.rs`). It does four things:

1. It reads what only it can see: its pid and its parent's, its working directory, `CLAUDE_CODE_SESSION_ID`, `CLAUDE_PROJECT_DIR`, and its own build. On the client's first line it connects to the control socket and sends these once, as the hello.
2. It forwards JSON-RPC lines both ways, unchanged.
3. When the app is not running, it starts it with `open -g`, as before, and waits up to 15 s.
4. When the app closes the connection, it answers the client's requests still in flight with an error. On the client's next line it connects again, sends the hello with `resumed: true`, and replays the client's `initialize` and `notifications/initialized` under its own id. It drops the app's reply to the replay.

It exits when stdin closes, and when the parent process it started under exits (kqueue `NOTE_EXIT` on macOS). The parent watch covers clients that leave stdin open in another process. There is no idle exit: Claude Code marks a stdio server that exits as failed until the user reconnects it.

The app serves MCP in the MCP host (`crates/mcp/src/host.rs`): the rmcp server, the tools, and session detection, which now reads the client's environment from the hello instead of its own. The host calls the app through the same `Control` as before. `Control` reaches the app through one of two adapters: the control socket, for a v1 process, or an in-process duplex stream into the app's own connection handler. So the app handles every tool's requests with one code path, and the host's connection ends when the pipe's does, which starts the session's 2-second Disconnect Grace as before (ADR 0016).

The note that a display ended because the app quit used to live in the MCP process's memory. It now lives in the app's display ledger, `ledger-<instance>.json` in the state directory. The app rewrites it whenever a session makes a display or launches an app there, and reads it once at start: those are the previous run's displays, so a call naming one says the app quit, and the session's next `create_display` adopts the apps it launched there.

## The control protocol, version 2

The first line on a control connection decides its kind:

```text
→ {"id": 0, "method": "pipe", "params": {"version": 2, "pid": 123, "parentPid": 45, "cwd": "/w/proj",
     "env": {"CLAUDE_CODE_SESSION_ID": "…", "CLAUDE_PROJECT_DIR": "/w/proj"},
     "build": {…}, "resumed": false}}
← {"id": 0, "result": {"version": 2}}
```

After the reply, both sides send MCP JSON-RPC lines until one closes. Any other first line is a v1 connection: the app-level NDJSON methods of ADR 0005, unchanged. An app that predates version 2 answers `pipe` with `unknown method`; the pipe then answers each client request with an error that asks the user to update Agent Screen.

The pipe's interface to the app is only the hello and the line framing. A new tool, a changed tool or a fix to session detection changes the app alone. Version 3 is for a change to the hello or the framing.

## Compatibility

MCP processes already running from older builds speak v1. The app keeps serving v1 connections until those clients restart, and the test script `mcpv1` drives the tools through a v1 process against the new app. `AgentScreen mcp --v1` (and `agent-screen-mcp --v1`) runs the old in-process server for that test and for the mock-socket tests in `crates/mcp/tests`; clients are never configured with it.

## Orphans

The app reads each control connection's peer pid (`LOCAL_PEERPID`). Every 30 s, and when a connection opens, it ends the peers whose parent is launchd (pid 1) and whose executable is an Agent Screen binary, with SIGTERM, and logs each one. A peer whose parent is still running is never touched, however long it has been idle.

## Consequences

- An app update reaches every session at its next call. The agent's client fetches the tool list again when the host sends `notifications/tools/list_changed` after a resumed hello; a client that ignores it keeps its cached list until it reloads.
- The pipe's first line starts the app. Before, only the first tool call did, so a client that starts with Agent Screen configured now starts the app too.
- `get_diagnostics` reports the pipe's build beside the app's. The MCP server's build is the app's.
- A pipe holds about one process's worth of memory and no tool code; what it does can only change with the hello's version.
