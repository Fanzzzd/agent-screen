---
default: patch
---

A new display is ready about a second sooner. Sidescreen parks it right after macOS makes it, without waiting for the app's main thread, which every new display stalls for a while, and parks it for the session only: made permanent, macOS took 2.3 s instead of 1.3 s and still showed the next display at its default spot first.
