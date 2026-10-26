---
default: patch
---

`launch_app` with arguments or an environment takes the instance it started. Every checkout of an Electron project runs the same Electron.app, and a copy another tool started from a shell during the launch could be taken for it: that copy's windows went to the agent's display, and the agent's own window was lost. Agent Screen now picks the new process launchd started with the arguments it asked for.
