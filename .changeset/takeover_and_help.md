---
default: patch
---

# Taking over and asking for help

While you answer a help request in the Viewer, its reason stays in the header. When you take over, the agent's typing or drag still in progress stops at once instead of finishing into the field. A finger resting on the trackpad over the Viewer no longer takes the display over; a scroll that moves does. An agent whose help request times out hears that the request is still open, and can take it back with `withdraw: true`. If you close the display while the agent waits, it is told you closed it. When two agents use the same app, a new window that either could have opened stays put for its owner to place. Closing a display now also closes or returns windows the agent opened while you were deciding.
