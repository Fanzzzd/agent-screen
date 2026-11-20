---
default: patch
---

Sped up `get_window_state`, `act_on_element`, and `launch_app` by capturing window screenshots in parallel with accessibility tree reads, skipping window capture for `act_on_element` calls with `result: "none"`, moving screenshot base64 encoding off the main thread, batching menu-item accessibility reads, and parsing `Info.plist` URL schemes in process without spawning `plutil`.
