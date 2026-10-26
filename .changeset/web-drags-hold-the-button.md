---
default: patch
---

An agent's drag in a web page now holds the left button, as a hand's does. Before, Chromium and Electron pages saw the moves with no button held, so splitters, sliders and sortable lists didn't follow, and an HTML5 drag ended as soon as it started. For the drag only, under a second, apps read the left button as held; your cursor stays where it is, and no app gets a click.
