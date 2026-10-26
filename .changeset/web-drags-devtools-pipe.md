---
default: minor
---

Drags in web pages work in Chromium and Electron apps an agent starts with `launch_app`: splitters, sliders, sortable lists and HTML5 drag and drop. Before, every move reached the page with no button held, so the page took the drag for hover. `launch_app` now starts these apps itself with a private DevTools pipe that only Agent Screen holds. No port opens. A drag goes through the pipe inside the app, so your mouse button and pointer stay yours. For a browser, the agent starts a new instance on a throwaway profile. A drag in a web page of an app without the pipe, such as Safari or an app you opened, now fails with a message that suggests `launch_app` or `request_help`. Before, it seemed to succeed and did nothing.
