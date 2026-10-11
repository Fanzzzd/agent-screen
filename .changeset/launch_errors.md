---
default: patch
---

# launch_app reports a failed open

An app or file that macOS can't open fails `launch_app` with "could not open" within about a second, and a URL that didn't open in the new window is an error instead of a success.
