---
default: patch
---

# A slow file window moves as it opens

When an app you are running takes a while to open a file, Sidescreen now moves the file's window onto the agent's display the moment it appears, instead of after it showed on your screen. `launch_app` says `existing_window` only when a window the app already had shows the file; otherwise `still_opening`. `file://` URLs open like their paths.
