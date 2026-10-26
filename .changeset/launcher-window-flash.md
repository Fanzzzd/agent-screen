---
default: patch
---

When you open an app from Spotlight, Raycast or Alfred while you work in a Viewer, its window now leaves your screen within about 70 ms. Before, it showed there for 150 to 430 ms, and AeroSpace re-tiled your windows around it meanwhile. Agent Screen now checks for the new window every frame instead of every 150 ms, and moves it before it asks AeroSpace anything.
