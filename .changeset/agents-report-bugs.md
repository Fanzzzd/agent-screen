---
default: minor
---

Agents can now report an Agent Screen bug themselves with the new `report_issue` tool, in any MCP client, with no `gh` or token on your Mac. Agent Screen adds its version and build, your macOS version, the agent's client and version and the session's diagnostics, and removes window titles, typed text, links, file paths and pictures before anything leaves the Mac. The agent sees the report exactly as it would be sent. It checks GitHub first: a known issue comes back with its number, and a bug already fixed in a newer release comes back with a request to update, so nothing is filed. The first report waits on a card under the menu bar icon for your OK; after that, the prefilled bug report form opens in the background in your browser for you to submit, at most once a day for the same failure. The menu's Agent Bug Reports lists what was reported, and you can turn reports off there, on the card or in Settings.
