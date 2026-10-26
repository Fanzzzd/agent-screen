---
default: minor
---

`place_window` takes `x`, `y`, `width` and `height` to move or resize a window on the agent's display, in screenshot pixels from the display's top-left corner. It returns the frame the window ended with, since apps keep to their own minimum sizes, and says when that isn't the frame asked for. An agent no longer needs System Events to resize a window, where a position meant for the display moved the window onto the user's screen.
