---
default: patch
---

A Chrome window an agent opens with `launch_app` now shows in front on its Agent Display. Since 0.1.42 the agents' own Chrome opened its window behind any window already on the display, such as another Chrome's, so the agent's screenshot showed the other window and its clicks and typing went there. Agent Screen now raises the new window on the display, which activates nothing and leaves your front app in front.
