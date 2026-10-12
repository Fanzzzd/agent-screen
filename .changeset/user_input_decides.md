---
default: patch
---

# Your clicks and keys keep your windows yours

A new window of an app the agent started now goes to the agent's display only if the agent acted there in the last 3 seconds and you didn't click or type in that app, or click in Finder or the Dock. A window you open yourself stays on your screen. A slow window that opens with nobody acting stays where it opened, and the agent can still place it with `place_window`. An app the agent cold-starts gets 30 seconds for its first window. Clicks and keys you send through a display's viewer count as driving that display. When a session disconnects, its display stops taking new windows.
