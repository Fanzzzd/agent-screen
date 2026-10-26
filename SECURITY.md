# Security policy

## Supported versions

Only the [latest release](https://github.com/Fanzzzd/agent-screen/releases/latest) gets security fixes. A fix ships as a new release, through the DMG and the Homebrew cask. Update with `brew upgrade --cask agent-screen` or the latest DMG before you report.

## Report a vulnerability

Report it privately. Don't open a public issue.

1. Open the repo's [Security tab](https://github.com/Fanzzzd/agent-screen/security).
2. Press **Report a vulnerability**. The report reaches only the maintainer.

Include:

- the Agent Screen version (`"/Applications/Agent Screen.app/Contents/MacOS/AgentScreen" --version`) and the macOS version
- what an attacker can do, and what they need first: a local account, a process running as the user, an agent connected through MCP, or a web page an agent opens
- steps to reproduce, or a proof of concept
- the parts of `~/Library/Logs/Agent Screen/displays.log` or Console output that show it, if any

The maintainer acknowledges a report within 7 days. After that, the maintainer tells you whether the report is accepted and the plan for a fix. Please keep the report private until a fixed release is out.

## Scope

Agent Screen runs with broad macOS grants, so these areas matter most:

- **Screen Recording and Accessibility.** The app holds both grants to show agent displays, capture them and act on their windows. A way to make it capture or act on the user's own screen, or on a window the agent doesn't own, is in scope.
- **The control socket.** `AgentScreen mcp` talks to the app over a Unix socket that only the user's account can open ([ADR 0005](docs/adr/0005-rust-app-and-mcp.md)). Another account reaching it, a malformed message crashing the app or taking over its grants, or a debug control method answering in a release build started without `AGENTSCREEN_DEBUG=1` is in scope.
- **The MCP server.** One agent session can see and drive only its own display. A session reaching another session's display, or the user's input, is in scope.
- **The ColorSync helper.** `agent-screen-colorsync` runs as root through a sudoers entry ([ADR 0009](docs/adr/0009-colorsync-registry-reclaim.md)). Any way to make it do more than its one job is in scope.

Out of scope: what an agent does with the tools it was given on its own display, and macOS behavior that Agent Screen can't change, such as the private APIs it relies on breaking in a new macOS.
