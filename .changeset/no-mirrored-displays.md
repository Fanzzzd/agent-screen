---
default: patch
---

Creating an Agent Display no longer leaves your screen at the wrong resolution for about 10 seconds when macOS mirrors the new display with your main display. On 18 November, macOS put every new virtual display that got a certain unit number on top of the main display, which then switched to the new display's size until that display went, and this lasted across Agent Screen restarts. Agent Screen now releases such a display as soon as macOS makes it and puts back any display that moved. It remembers the unit number until you log out or restart the Mac, and makes no display that would get it again. `create_display` then tells the agent not to retry, so sessions no longer resize your screen every few minutes. Displays work again once the number of displays online changes, and a spare display already made still goes to the next session. Two failed attempts in a row to place a new display out of your way stop new displays until Agent Screen restarts.
