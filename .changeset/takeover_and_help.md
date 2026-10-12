---
default: patch
---

# Taking over and asking for help

While you answer a help request in the Viewer, its reason stays in the header. When you take over, the agent's typing stops at the next character and its drag lets go where the pointer is, short of the drop; the agent is told how far it got. A finger resting on the trackpad over the Viewer no longer takes the display over; a scroll that moves does. An agent whose help request goes unanswered hears that the request is still open, and can take it back with `withdraw: true`. If you close the display while the agent waits, it is told you closed it. A new window of an app that another agent is also using on its own display, opened while both were acting, stays where it opened. Closing a display now also closes or returns windows the agent opened while you were deciding.

`hold_key` lets go as soon as you take over, so your typing doesn't come out with the agent's Shift held. A drag stopped right after it starts no longer turns into a click on what it grabbed. Text with Windows line endings types one Return per line.
