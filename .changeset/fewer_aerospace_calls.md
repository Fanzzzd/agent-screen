---
default: patch
---

# Sidescreen asks AeroSpace far less often

Every command makes AeroSpace refresh all of your windows first, which takes up to two seconds on a busy Mac and stalls AeroSpace meanwhile. Sidescreen asked it on every display change, up to 25 times a minute while agents worked. It now asks once, when the first agent display appears, and not at all when no agent display is online.
