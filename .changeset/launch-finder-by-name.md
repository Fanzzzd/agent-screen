---
default: patch
---

`launch_app` finds Finder and the other apps in /System/Library/CoreServices by name, and any other app macOS knows by name. It used to answer `not_found` for "Finder" and work only with `com.apple.finder`.
