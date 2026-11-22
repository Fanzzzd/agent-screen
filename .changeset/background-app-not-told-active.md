---
default: patch
---

When an agent clicks or types in its window of an app you also use, such as a browser or another Electron app, while that app is in the background and one of your own windows of it is key, the app is no longer told it became the active app. Your front app always stayed in front. But Electron apps reported themselves active several times per action, so an app that reacts to that, for example by bringing a window forward or marking messages read, could act as though you had switched to it. The agent's window still gets the input, and your window of that app gets key focus back afterwards.
