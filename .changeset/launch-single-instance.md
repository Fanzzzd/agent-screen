---
default: patch
---

`launch_app` with arguments or an environment, on an app that allows only one instance and already runs, answers with the running instance's window on your display and says the app was already running. It used to wait about 10 seconds and answer that the app was still starting, or fail with "did not start". When the running instance has no window on the display, the error says the app is already running.
