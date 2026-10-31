# Agent Screen

Agent Screen gives each AI agent session its own virtual display on the user's Mac, so the agent can operate GUI apps without taking over the user's screen, cursor or keyboard. The user can watch any agent's display and step in.

## Language

**Agent Display**:
A virtual display owned by exactly one Agent Session. It is created the first time the session needs a screen. When the session stops using it, it closes by itself if no window is left on it, and otherwise becomes a Review Display. The user can also create one from Peek; it then has no Agent Session.
_Avoid_: Agent Screen (the app's name), virtual screen, screen, bay

**Spare**:
An Agent Display that closed while empty and stays connected, out of sight, for a few minutes, so the next display of its size reuses it instead of adding one. It belongs to no Agent Session and shows nowhere in the UI.
_Avoid_: pooled display, idle display

**Display Label**:
How the user tells Agent Displays apart: the client's name plus a task title the agent gives when creating the display, or the project folder when it gives none.
_Avoid_: display name, title

**Review Display** (待查看显示器):
An Agent Display whose Agent Session stopped using it, by `end_display` or a closed connection, while windows remain on it. It accepts no agent input and waits for the user to decide where its windows go. It says nothing about whether the task succeeded.
_Avoid_: Ended Display, finished display, orphaned display, dead display

**Agent Session**:
One run of one agent (a Claude Code or Codex conversation, for example) connected to Agent Screen. It has at most one Agent Display, and many sessions never get one.
_Avoid_: agent, client, worker

**Viewer**:
The floating panel that shows one Agent Display live and lets the user operate it.
_Avoid_: control panel, popup, window

**Peek**:
The strip of live thumbnails of every Agent Display that appears when the user hovers the menu bar icon, and stays up after a click on it. Displays in use come first, Review Displays after them.
_Avoid_: preview, hover panel

**Takeover**:
A state of an Agent Display in which the user operates it and the owning Agent Session's input is refused. It begins with the user's first input in the Viewer, or 接手操作. It pauses any Window Authorization. It ends only when the user hands control back, through the interface or an explicit instruction in the conversation that the Agent Session relays. Collapsing the Viewer, switching displays or waiting keeps it.
_Avoid_: pause, lock, manual mode

**Help Request**:
An Agent Session asking the user to step in on its Agent Display, with a reason (a login, a captcha) and up to five steps. It ends when the user finishes a Takeover.
_Avoid_: escalation, alert, ping

**Help Card** (求助卡片):
The card that drops from the menu bar icon while a Help Request is open: the display's picture, the agent's reason, and a way to take over or put it off. It stays until the Help Request ends or the user puts it off.
_Avoid_: alert, popup, banner

**Recording** (录制):
A video of an Agent Display that its Agent Session asked for, with a timeline of the session's actions and the Takeovers. It ends when the session stops it, when the display ends, or after 30 minutes.
_Avoid_: capture, screencast

**Keyboard Borrow** (借用键盘):
One `type` or `key` call that goes through the user's input method. For the call, the agent's app has keyboard focus and the user's keys reach it (ADR 0008).
_Avoid_: focus steal, IME mode

**Input Request** (申请指针和键盘):
An Agent Session asking to use the user's own pointer and keyboard on its Agent Display, for an action that background input can't do, such as a drag in a web page of the user's browser. It waits in the Input Queue until the user allows it, and is declined when the user answers Not Now.
_Avoid_: borrow, lease, takeover

**Input Queue**:
The Input Requests of every Agent Session waiting their turn, one at a time, in the order they came.
_Avoid_: pool, lock

**Input Grant** (放行期):
The user's permission for the Input Queue to run, including requests that arrive while it runs. It ends when the user touches the mouse or keyboard, when the queue has been idle for 30 seconds, or when no operation is waiting or running and every participating Agent Session has declared itself done. One session cannot end the others' permission.
_Avoid_: blanket access, Takeover

**Input Loan** (借出):
One complete operation's exclusive use of the user's pointer and keyboard on its own Agent Display during an Input Grant. Other sessions wait until it has released its inputs. Completing a loan leaves the grant open for the next operation. Normal grant completion restores the user's pointer and front window; user-initiated revocation leaves the pointer and front window where the user took control. Both release the agent's held keys and buttons. This build has no executor, so no loan runs: touch revocation, restoration and held-input release are the design in ADR 0013, not shipped behavior.
_Avoid_: lease, borrow. Keyboard Borrow is a different thing: one input-method call that needs no permission.

**Agent Pointer**:
The marker the Viewer and Peek draw where the Agent Session last pointed or clicked. It is never part of the display itself, so the agent's screenshots do not contain it.
_Avoid_: agent cursor, ghost cursor

**Placement**:
Moving an app window onto or off an Agent Display. Either the Agent Session or the user can place a window.
_Avoid_: pull, move, send back

**Handover**:
The user giving one of their own windows to an Agent Display so the Agent Session can work in it, through the interface or a conversation instruction. A Window Authorization can allow the Agent Session to do the Placement without asking again. A handed-over window keeps its return location and always returns to the user when its display is dismissed.
_Avoid_: share, assign. The Viewer's button for it is labelled Give; the concept is still Handover.

**Window Request**:
The Agent Session asking the user for one of their windows, by app or window. The user answers Hand Over, which makes it a Handover, or Keep, and the window stays theirs.
_Avoid_: window permission, borrow

**Control Request**:
During a Takeover, the Agent Session asking the user to hand its display back, with a reason. The user answers Let … Continue, which ends the Takeover, or Keep Control, and the Takeover goes on. They can also explicitly hand control back in the conversation through the Agent Session. A request alone never ends the Takeover (ADR 0004).
_Avoid_: resume request, reclaim

**Window Authorization**:
The user's explicit instruction allowing an Agent Session to take a user window without another confirmation. It can cover one Placement or persist for that Agent Session, for all apps or named apps. A Takeover pauses a continuing authorization; handing control back resumes it. Revoking it prevents future Placements without moving windows already handed over. Ending the display or disconnecting the session clears it. A window the user takes back stays excluded until they explicitly authorize that window again.
_Avoid_: full access, unattended mode, blanket permission

**Disposition**:
The Agent Session's standing verdict on one of its windows: keep it for the user to review, or close it. The agent can revise it at any time; the user has the final say when dismissing the display.
_Avoid_: cleanup flag, keep/delete mark
