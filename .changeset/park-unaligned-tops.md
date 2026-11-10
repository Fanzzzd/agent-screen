---
default: patch
---

Agent Displays work again on a Mac whose screens' tops don't line up, such as a built-in display placed lower than the main one. Since 0.1.20 every `create_display` failed there with "macOS could not create a virtual display at a verified safe position" (#155). Agent Screen now parks a new display at the top-right corner of your rightmost screen, where macOS keeps it, and accepts the position macOS chooses when that position touches your screens at most at a corner. A display that would share an edge with one of your screens, overlap one, or sit between them is still refused, so your pointer can't slide onto it.
