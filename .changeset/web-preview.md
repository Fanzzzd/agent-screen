---
default: minor
---

You can now watch any live Agent Display in a local browser without opening the Viewer. `create_display` returns a `preview_url` on `127.0.0.1` with a random token for that display so your agent can paste the link in chat, and **Copy Web Link** in the display's menu in Peek or the menu bar copies it at any time. The page streams the display live while a tab is open, encodes nothing when nobody is watching, sends no clicks or keys, and returns `404` as soon as the display ends.
