---
default: patch
---

# launch_app answers at once for a single-window app

For a running app whose menus have no New Window command, such as Calculator, `launch_app` answers `no_new_window` right away instead of after 3 seconds. Its answers now include `wasRunning`.
