---
default: minor
---

# Sidescreen changes the Mac's displays far less often

Every display Sidescreen added or removed made each app on the Mac reconfigure its screens, and with a virtual display connected each change left macOS doing more color work for minutes. After many agent displays in a short while, the whole Mac could slow down or freeze. Sidescreen now keeps an emptied display for a few minutes and gives it to the next agent that needs one of the same size, adds each new display with one change, and lets agents make at most ten new displays in ten minutes. `~/Library/Logs/Sidescreen/displays.log` records every display it adds, keeps, reuses and removes.
