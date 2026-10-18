---
default: patch
---

The displays log names where each main-thread stall of 150 ms or more went: while the main thread is late, Sidescreen reads its stack every 10 ms, and the full counts go to a `hang` file beside the log.
