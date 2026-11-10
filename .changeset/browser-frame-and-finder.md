---
default: patch
---

`place_window` reports the frame a window ended with when the app answers a resize with an error but takes it anyway, as some apps such as Dia do. It used to fail with "the app rejected the change" for a window that had moved and resized. It fails only when the window kept its frame, and then says which frame that is.

`launch_app` finds Finder and the other apps in /System/Library/CoreServices by name, and any other app macOS knows by name. It used to answer `not_found` for "Finder" and work only with `com.apple.finder`.

`launch_app` with a file path for a browser that is already running opens the file in a new window on the agent's display, as it does a URL. The browser used to open it as a tab of its front window, which may be yours, and the agent got `still_opening` with no window.
