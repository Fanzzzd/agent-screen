---
default: minor
---

# Recordings have sound, and agent displays can wear your wallpaper

A recording made with `record_display` now has the sound of the apps on the display. Only apps whose every window is on that display are heard: macOS separates sound by app, not by window, so an app you also use elsewhere, such as your browser, stays silent, and `record_display` stop lists which apps were heard and which were left out. In a test, a tone from an app on the display came through at -7.5 dB, and one from a process elsewhere at -100 dB.

Agent Display Wallpaper… in the menu sets the picture agent displays show in the Viewer and in recordings; Use Sidescreen's Wallpaper puts the default back. Agents' screenshots leave the wallpaper out either way.
