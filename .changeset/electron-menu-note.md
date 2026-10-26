---
default: patch
---

Pressing a menu item of an Electron app with `act_on_element` now comes back with a note. Electron tells the app that the press was the item's keyboard shortcut, and some apps ignore shortcuts while a text field has focus. The note, the tool's description and the agent instructions all say to click outside the field first. Agent Screen can't make the press look like a click: macOS runs an accessibility press with no current event.
