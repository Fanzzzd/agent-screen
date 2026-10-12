---
default: patch
---

# A file opened in a running app finds its way to the agent's display

When an agent opens a file in an app you are running, Sidescreen recognizes the file's window by the document it shows. If the app takes longer than 4 seconds, the window still moves to the agent's display when it opens, for up to 30 seconds, and `still_opening` says so. Your other windows in that app stay where they are.
