---
default: patch
---

Screenshots answer in about half the time, 26 ms instead of 50 ms in the median. Sidescreen asked ScreenCaptureKit to list every window before each one, about 20 ms. It now asks only when a window has come or gone since, which the window server tells in under a millisecond, and reads where windows are now from the window server.
