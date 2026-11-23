---
default: patch
---

An agent's click right after it chose a menu item with Return now reaches the page. The menu fades out for about a quarter second after Return, and Chrome dropped a click that came in that time, though the click reported success. The next click or key now waits until the menu is gone, 400 ms at most, and only right after an input closed a menu.
