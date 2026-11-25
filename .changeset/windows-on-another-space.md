---
default: patch
---

Agents no longer report a window as closed when it only went out of sight. When an agent's window is minimized, its app is hidden, or it sits on a macOS Space (desktop) the agent's display isn't showing, `wait_for_change` now says which (`window_minimized`, `window_hidden`, `window_on_other_space`) instead of `window_closed`. `list_windows` keeps such a window with an `offScreen` entry instead of dropping it, and `get_window_state` and the other window tools say why they can't reach it and what to ask you, instead of "no window". Agent Screen still never switches your Space or shows a hidden app for the agent.

With "Displays have separate Spaces" off in System Settings > Desktop & Dock > Mission Control, as many AeroSpace users run it, every display shares one Space, so switching desktops on your screen also takes the agents' windows off their displays. Agent Screen now notices this. It tells the agent at `create_display`, shows you a card once, and lists the setting in Settings under Agent Displays with a button that opens Desktop & Dock. Turning the setting on takes a logout; Agent Screen doesn't change it for you.
