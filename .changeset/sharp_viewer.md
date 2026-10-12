---
default: patch
---

# The Viewer is sharp again

Agent displays could come up at 1x and look blurry in the Viewer: macOS remembers the resolution last used for a display's serial number, and Sidescreen's serials repeat across launches, so a display could inherit another size from an earlier one. Sidescreen now sets each display to its size at 2x as it makes it. The Viewer also no longer keeps showing a small preview frame, scaled up, when the display it opens on is still.
