---
default: patch
---

A new window that an app the agent started opens by itself, long after the agent's last action, goes to the agent's display when none of the app's other windows is on your screen. Before, it opened on your screen until the agent called `place_window`. UTM did this on every guest reboot, opening a new VM window each time.
