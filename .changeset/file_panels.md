---
default: patch
---

# Agents can use file Open and Save panels

An app's Open or Save panel is drawn by a separate macOS process, behind an empty window of the app's, so an agent's clicks and keys never reached it and the panel looked frozen. Clicks and keys now go to the process that draws the panel, and the first keys wake it the way a click would, so an agent can pick a file with Go to Folder (cmd+shift+g) and a path. `get_window_state` also reads a panel that a browser such as Dia or Chrome shows as a sheet.
