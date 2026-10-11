---
default: patch
---

# AeroSpace config edits keep dotfile links and comments

Adding `ignored-monitors` from the menu writes to the file a symlinked config points to, so dotfile managers keep tracking it, and reads lists with comments in them. Sidescreen also notices when AeroSpace is started, quit or reconfigured while it runs.
