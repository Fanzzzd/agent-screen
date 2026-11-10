---
default: patch
---

An agent can click and type in an app that is your front app when the app's focused window is on the agent's display, such as WeCom with its only window handed over. Every click and key used to be refused as reaching "the user's window", which was that handed-over window. When the app's focused window is on your screen, input is still refused as before. When `place_window` finds no window of a running app, its error now says that the user can reopen a closed main window from the app's Dock icon.
