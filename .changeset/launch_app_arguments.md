---
default: minor
---

# launch_app starts development builds

`launch_app` takes the path of an `.app` bundle, and `arguments` and `environment` for a new instance of the app, so an agent can start a development build without a wrapper bundle. An app that makes itself an accessory app after it starts, as Electron builds that open windows in the background do, now shows its windows on the display, and `list_windows` lists them. An app that takes more than a few seconds to open its first window now gets that window on the display too.
