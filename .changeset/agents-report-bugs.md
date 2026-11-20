---
default: minor
---

Agents can now report an Agent Screen bug themselves with the new `report_issue` tool, in any MCP client, with no `gh` or token on your Mac. Agent Screen adds its version and build, your macOS version, the agent's client and version and the session's diagnostics. It redacts known patterns, such as window titles, quoted text, links, host names, IP addresses, paths, file names, email addresses, keys and your name, but it can't recognize every private word, so review each report before sending it.

Every report waits on a card under the menu bar icon. Show Report shows the whole report, and Open on GitHub opens that one report's prefilled bug report form for you to submit. Nothing leaves your Mac before your click. Once you have opened a report, later ones first check GitHub: a known issue comes back to the agent with its number, and a bug already fixed in a newer release comes back with a request to update, so nothing waits. The menu's Agent Bug Reports lists what you reported, and you can turn reports off there, on the card or in Settings.
