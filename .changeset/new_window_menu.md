---
default: patch
---

# A running app gets asked for a window, not sent Command-N

For an app you are running, `launch_app` now looks in its menus for New Window, or a document app's New, and uses that item. In Notes, Music or Calendar, where Command-N makes a note, playlist or event, it creates nothing and answers `no_new_window`. A greyed-out item answers `app_busy`. A URL that can't get its own window fails instead of opening in your window.
