---
default: patch
---

Reduced window-list scanning overhead during agent clicks and window lookups by filtering CoreGraphics window lists before allocating strings and caching off-screen remote panel lookups while the on-screen window layout is unchanged.
