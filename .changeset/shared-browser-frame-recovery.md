---
default: patch
---

An explicit `place_window` frame now stays below the reserved menu strip, so an agent can move or shrink a zoomed window whose toolbar became unreachable. The result reports the actual frame and explains when the constraint shrinks a dimension. For a window already on the display, placement without frame arguments keeps its frame. Shared-browser key refusals now describe foreground/focused-window evidence instead of claiming the user is active. Browser drag guidance explains that a fresh profile has separate sign-ins and cannot add a pipe to an existing window.
