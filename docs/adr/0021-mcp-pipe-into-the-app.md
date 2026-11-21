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

The app serves MCP in the MCP host (`crates/mcp/src/host.rs`): the rmcp server, the tools, and session detection, which now reads the client's environment from the hello instead of its own. The host calls the app through the same `Control` as before. `Control` reaches the app through one of two adapters: the control socket, for a v1 process, or an in-process duplex stream into the app's own connection handler. So the app handles every tool's requests with one code path. When the pipe's connection reaches its end, the app closes the host's connections into it at once, even while a call such as `wait_for_control` is still waiting, which starts the session's 2-second Disconnect Grace as before (ADR 0016).

The note that a display ended because the app quit used to live in the MCP process's memory. It now lives in the app's display ledger, `ledger-<instance>.json` in the state directory. The app rewrites it from its model whenever the live displays or their apps change: each Agent Display still live, with the apps it started that are still its own. A display that ended, by `end_display`, its grace running out or the user closing it, leaves the ledger, and so does an app the user took back (ADR 0016). The app reads the ledger once at start: those are the displays live when the previous run ended, so a call naming one says the app quit, and the session's next `create_display` adopts the apps it launched there. The orphans are taken only once that `create_display` makes a display, so a refused one hands them to the next. An earlier ledger also kept ended displays and taken-back apps, and after a restart `place_window` could move a window the user had taken back without a Window Request (#285). Each app is kept with its process's start time and executable, and handed on only while the process with that pid still has both, so a pid macOS has given to one of the user's apps since is never adopted. A ledger written before the Mac last booted is ignored.

## The control protocol, version 2

The first line on a control connection decides its kind:

```text
→ {"id": 0, "method": "pipe", "params": {"version": 2, "pid": 123, "connection": "123-9f2c…", "parentPid": 45, "cwd": "/w/proj",
     "env": {"CLAUDE_CODE_SESSION_ID": "…", "CLAUDE_PROJECT_DIR": "/w/proj"},
     "build": {…}, "resumed": false}}
← {"id": 0, "result": {"version": 2}}
```

After the reply, both sides send MCP JSON-RPC lines until one closes. Any other first line is a v1 connection: the app-level NDJSON methods of ADR 0005, unchanged. An app that predates version 2 answers `pipe` with `unknown method`; the pipe then answers each client request with an error that asks the user to update Agent Screen.

The pipe's interface to the app is only the hello and the line framing. A new tool, a changed tool or a fix to session detection changes the app alone. Version 3 is for a change to the hello or the framing.

## Sessions

The hello's `connection` is a random id the pipe makes when it starts, kept for its life and sent again on every reconnect. The app refuses a version 2 hello without one. The host picks each tool call's session from the first of these that names one (`crates/mcp/src/session.rs`):

1. The call's `_meta`: Codex's `x-codex-turn-metadata.session_id`, which separates the conversations one Codex process serves.
2. `CLAUDE_CODE_SESSION_ID` from the hello, only when the client's `clientInfo.name` is `claude-code`. A client started from Claude Code's shell inherits the variable; trusting it there would put two agents in one session.
3. The connection: `pipe:<connection>`. A client with no hint gets one session per pipe, and a later pipe that reuses a pid gets a new one.

A v1 process sends no hello and keeps its old key, `pid:<pid>`. The host reads the client's name and protocol version from `initialize`, or from each request's `_meta` for a client on MCP 2026-07-28, which sends no `initialize`. rmcp 3.5 negotiates the version. The host adds `ttlMs` and `cacheScope` to `tools/list` only for 2026-07-28 clients, since rmcp does not strip them for older ones. [docs/clients.md](../clients.md) has the setup for each client and what each one gets.

## Compatibility

MCP processes already running from older builds speak v1. The app keeps serving v1 connections until those clients restart, and the test script `mcpv1` drives the tools through a v1 process against the new app. `AgentScreen mcp --v1` (and `agent-screen-mcp --v1`) runs the old in-process server for that test and for the mock-socket tests in `crates/mcp/tests`; clients are never configured with it.

## Orphans

The app refuses a control connection from another user (the peer's uid), on top of the socket's mode 0600. It notes a connection's peer pid and pid version (`LOCAL_PEERTOKEN`) once the peer says it is an MCP server, with a pipe's hello or a v1 request, and forgets it when the peer's input ends. A second launch of the app, which only checks whether the socket answers, is never noted. Every 30 s, and when a peer is noted, the app ends with SIGTERM, and logs, each noted peer whose parent is launchd (pid 1) and that runs as an MCP server: `AgentScreen` with `mcp` as its first argument, or the development `agent-screen-mcp`. It first checks that the process with that pid still has the pid version it connected with. A peer whose parent is still running is never touched, however long it has been idle, and neither is the app itself.

## Consequences

- An app update reaches every session at its next call. The agent's client fetches the tool list again when the host sends `notifications/tools/list_changed` after a resumed hello; a client that ignores it keeps its cached list until it reloads.
- The pipe's first line starts the app. Before, only the first tool call did, so a client that starts with Agent Screen configured now starts the app too.
- `get_diagnostics` reports the pipe's build beside the app's. The MCP server's build is the app's.
- A pipe holds about one process's worth of memory and no tool code; what it does can only change with the hello's version.
