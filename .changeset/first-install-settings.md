---
default: minor
---

Settings now asks you to move Agent Screen to Applications when you open it from the disk image, from Downloads, or from the temporary copy macOS runs a downloaded app from. Permissions and agent connections set up from there stop working once the app moves. **Move to Applications…** copies it into Applications with its signature intact, asks before replacing a copy that is already there, and opens the moved copy at Settings. The disk image and the download are left as they were.

**Reset Permissions…** beside the Permissions title in Settings clears Agent Screen's own Screen Recording, Accessibility and Input Monitoring entries, ends open Agent Displays and reopens Settings so you can turn them on again. When a switch is on in System Settings but macOS still says no, the row offers **Reset & Reopen…** instead of asking you to remove and re-add the app by hand. It asks first, and it never touches another app's entries. `AgentScreen permissions reset` does the same from Terminal.

The Agents section of Settings now lists every agent client found on your Mac, each with its state: live now, configured (new sessions pick it up, or restart the app for Claude Desktop), an old path, not connected, or a config it can't read. Each row has one button, Connect, Fix Path or Disconnect, and **Connect All Detected** connects the rest at once, with no dialog to dismiss. **Other MCP Clients** copies the config as `mcpServers` JSON, OpenCode JSON, TOML or a command line. Once permissions are on and a client is configured, Settings shows a prompt to copy into a new session, which turns green when the agent loads the tools.
