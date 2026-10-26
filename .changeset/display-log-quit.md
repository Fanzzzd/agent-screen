---
default: patch
---

The display log in `~/Library/Logs/Agent Screen/displays.log` records when the app starts and quits, and a `released … quit` line for each display still connected at quit. Before, most displays left with the process and the log never said so, so a slow or frozen Mac couldn't be matched to those display changes.
