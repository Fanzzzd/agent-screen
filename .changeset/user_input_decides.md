---
default: patch
---

# Your clicks and keys keep your windows yours

A new window of an app the agent started now goes to the agent's display only if the agent acted there in the last 3 seconds and you didn't click or type in that app, or click in Finder or the Dock. A window you open yourself stays on your screen. A window that opens while you both act, or with nobody acting, stays where it opened; the agent's next screenshot tells it, and it can place the window with `place_window`. An app the agent cold-starts gets 30 seconds for its first window. Pressing Return in Spotlight, Raycast or Alfred counts as your input. Clicks and keys you send through a display's viewer count as driving that display. When a session disconnects, its display stops taking new windows.
