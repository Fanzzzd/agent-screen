---
default: patch
---

# Sidescreen takes a running app's windows for at most 4 seconds

When an agent opens a file in an app you are running, Sidescreen stops taking that app's windows 4 seconds after the request at most, even if the app is still opening the file; `launch_app` then answers with `still_opening`. Before, an app slow to open a file kept that window open for 6 seconds.
