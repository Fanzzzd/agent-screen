---
default: minor
---

Connect now sets up every agent client it finds on your Mac, not only Claude Code and Codex: Cursor, Claude Desktop, Antigravity (app and `agy`), Gemini CLI, OpenCode, VS Code and Copilot CLI, Windsurf, Devin, Zed, Cline, Roo Code and Goose too. Agent Screen edits each client's own config file and keeps your other servers, your settings in its entry and your comments. A config file it can't read is left as it is, and a crash during a write never leaves half a file. Claude Code no longer needs its command line on the app's PATH, so Connect no longer waits seconds for your shell.

An entry left by a copy you ran from the disk image or Downloads is now repointed to the copy you run. Three commands show and change this from Terminal: `AgentScreen mcp status` lists each client and whether its config runs this copy, `mcp install --client cursor,zed` connects only the clients you name (`--dry-run` shows what would change), and `mcp uninstall` takes Agent Screen's entry out again. Each takes `--json`.

The Agent Screen skill now also goes to `~/.agents/skills/agent-screen`, which Codex and other agents read, so Codex gets no second copy. A copy that `npx skills` installed or that is a link stays as it is.
