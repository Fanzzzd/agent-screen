---
default: patch
---

`place_window` now brings a window you minimized to the Dock onto the agent's display without showing it on your screen, once you have told the agent to take it or answered Hand Over, and puts it back in the Dock where it was when the agent is done. It does this only while the window's app is in the background with no other window you could see; otherwise the agent asks you to click the window in the Dock. Before, it answered that no window was on screen, and you had to restore the window yourself first. For an app you hid, a closed window, or one on another macOS Space, the agent is now told which, so it knows to ask you to bring it up; Agent Screen doesn't show an app you hid, since that would bring back all its windows.
