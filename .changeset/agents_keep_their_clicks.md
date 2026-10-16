---
default: patch
---

# Agents' clicks land when several apps run at once

When another app came to the front, even for a moment as when a second agent started an app, Chrome on an agent display ignored every click in its page, and an app such as Numbers dropped the first click on a window it had just swapped in. A window of an app in the background takes clicks only after one click makes it key. Sidescreen now spends that click itself, on an empty spot of the window's title bar, whenever a window hasn't had one since the front app last changed. It never does this in your front app.

The same goes for an app you and an agent both use: after you open a new TextEdit document, the agent's typing goes to its own window again instead of yours.
