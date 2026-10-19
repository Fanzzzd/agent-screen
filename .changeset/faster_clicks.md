---
default: patch
---

An agent's click answers in about 75 ms instead of about 450 ms. Agent Screen read the full window list twice per click, about 1,900 windows each time; it now reads it once, with CoreFoundation directly, and skips windows that can't be the target.
