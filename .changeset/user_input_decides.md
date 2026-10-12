---
default: patch
---

# Your clicks and keys keep your windows yours

A new window of an app the agent started now goes to the agent's display only if the agent acted there in the last 3 seconds and you didn't click or type meanwhile in that app, Finder, the Dock or a launcher such as Spotlight. A window you open yourself stays on your screen. A window that opens while you both act, or with nobody acting, stays where it opened; the agent's next screenshot or window state tells it, and it can place the window with `place_window`. For an app the agent cold-starts, this holds for 30 seconds from the launch. Raycast and Alfred count like Spotlight. Clicks and keys you send through a display's viewer count as driving that display. When a session disconnects, its display stops taking new windows.
