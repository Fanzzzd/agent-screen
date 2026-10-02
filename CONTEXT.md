# Sidescreen

Sidescreen gives each AI agent session its own virtual display on the user's Mac, so the agent can operate GUI apps without taking over the user's screen, cursor or keyboard. The user can watch any agent's display and step in.

## Language

**Agent Display**:
A virtual display owned by exactly one Agent Session. It is created the first time the session needs a screen. When the session stops using it, it closes by itself if no window is left on it, and otherwise becomes a Review Display. The user can also create one from Peek; it then has no Agent Session.
_Avoid_: Sidescreen (the app's name), virtual screen, screen, bay

**Display Label**:
How the user tells Agent Displays apart: the client's name plus a task title the agent gives when creating the display, or the project folder when it gives none.
_Avoid_: display name, title

**Review Display** (待查看显示器):
An Agent Display whose Agent Session stopped using it, by `end_display` or a closed connection, while windows remain on it. It accepts no agent input and waits for the user to decide where its windows go. It says nothing about whether the task succeeded.
_Avoid_: Ended Display, finished display, orphaned display, dead display

**Agent Session**:
One run of one agent (a Claude Code or Codex conversation, for example) connected to Sidescreen. It has at most one Agent Display, and many sessions never get one.
_Avoid_: agent, client, worker

**Viewer**:
The floating panel that shows one Agent Display live and lets the user operate it.
_Avoid_: control panel, popup, window

**Peek**:
The strip of live thumbnails of every Agent Display that appears when the user hovers the menu bar icon, and stays up after a click on it. Displays in use come first, Review Displays after them.
_Avoid_: preview, hover panel

**Takeover**:
A state of an Agent Display in which the user operates it and the owning Agent Session's input is refused. It begins with the user's first input in the Viewer, or 接手操作. It ends only when the user hands control back; collapsing the Viewer or switching displays keeps it.
_Avoid_: pause, lock, manual mode

**Help Request**:
An Agent Session asking the user to step in on its Agent Display, with a reason (a login, a captcha). It ends when the user finishes a Takeover.
_Avoid_: escalation, alert, ping

**Agent Pointer**:
The marker the Viewer and Peek draw where the Agent Session last pointed or clicked. It is never part of the display itself, so the agent's screenshots do not contain it.
_Avoid_: agent cursor, ghost cursor

**Placement**:
Moving an app window onto or off an Agent Display. Either the Agent Session or the user can place a window.
_Avoid_: pull, move, send back

**Handover**:
The user placing one of their own windows onto an Agent Display so the Agent Session can work in it. A handed-over window always returns to the user when its display is dismissed.
_Avoid_: share, assign, give

**Disposition**:
The Agent Session's standing verdict on one of its windows: keep it for the user to review, or close it. The agent can revise it at any time; the user has the final say when dismissing the display.
_Avoid_: cleanup flag, keep/delete mark
