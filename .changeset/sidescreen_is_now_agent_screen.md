---
default: minor
---

# Sidescreen is now Agent Screen

The app has a new name and a website, [agentscreen.app](https://agentscreen.app). It installs as Agent Screen.app with a new bundle id, `app.agentscreen.AgentScreen`, so macOS sees a new app. Do this once after updating:

1. Quit Sidescreen and delete Sidescreen.app. With Homebrew, run `brew update` and then `brew upgrade --cask agent-screen`: the `sidescreen` cask is now `agent-screen`, in the tap `fanzzzd/agent-screen`. A new install is `brew tap fanzzzd/agent-screen && brew install --cask agent-screen`.
2. Open Agent Screen and turn on Screen & System Audio Recording and Accessibility again in the setup window. macOS ties both to the bundle id, so Sidescreen's grants don't carry over. You can remove Sidescreen from both lists in System Settings.
3. Press **Connect…** in the setup window. Your agents get the `agent-screen` MCP server, and Connect removes the old `sidescreen` entry from Claude Code and Codex when it runs Sidescreen.app. An entry that points anywhere else stays. Start a new agent session to load the tools.
4. If you set up Smooth Display Changes, the setup window offers it again as **Update…**. It asks for your administrator password once, installs the helper under its new name and removes Sidescreen's.

Agent Screen carries over the rest by itself:

- Agent Displays are now named "Agent Screen 1", "Agent Screen 2" and so on. If your AeroSpace config has `ignored-monitors = ['^Sidescreen']`, Agent Screen changes it to `['^Agent Screen', '^Sidescreen']` at launch, keeps the old file beside it as `.before-agent-screen` and reloads AeroSpace.
- The wallpaper you chose for Agent Displays is copied to `~/Library/Application Support/Agent Screen`.
- Logs go to `~/Library/Logs/Agent Screen` and recordings to `~/Movies/Agent Screen`. Sidescreen's folders stay where they are. Move any recordings you want to keep, then delete `~/Library/Application Support/Sidescreen`, `~/Library/Logs/Sidescreen` and `~/Movies/Sidescreen`.

Scripts that set `SIDESCREEN_INSTANCE`, `SIDESCREEN_LANG` or another `SIDESCREEN_` variable now use `AGENTSCREEN_`. Agent Screen's build of AeroSpace keeps its cask name, `aerospace-sidescreen`, and comes from the renamed tap.
