---
default: patch
---

`place_window` finds an app's window by the app's name as you see it, its bundle id, its .app path, or the owner name macOS gives its windows, in any case. An agent that asked for WeCom as 企业微信 or `com.tencent.WeWorkMac` got `not_found`, because its windows are listed as "WeCom". When `place_window` finds no window, it now tells the agent why: no app has that name or bundle id, the app isn't running, it has no window on screen, its only windows are dialogs, its window is already on the agent's display, or the window is minimized, hidden, a dialog, or on another agent's display.
