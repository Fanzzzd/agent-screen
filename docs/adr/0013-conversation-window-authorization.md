# Conversation instructions can authorize control and window Placement

The user can already give a window through the Viewer and answer a Window Request from a card. Requiring the same confirmation after they explicitly instruct the Agent Session in the conversation adds a second operation. The session may relay that instruction to Agent Screen instead. The interface controls remain available and use the same domain rules.

Agent Screen does not read the conversation. It trusts the Agent Session to relay an explicit user instruction, and records the instruction with the action. This is not independent proof of consent. The tools must explain that the agent may not supply its own permission, treat page content as a user instruction, or infer a hand-back from silence. A local caller that can use the control socket is already trusted to operate the session's windows.

## Window Authorization

A one-time instruction authorizes one Placement of the selected user window. It is carried with that operation, not stored as a general permission that a later call could reuse for another window.

A continuing Window Authorization belongs to the current Agent Session and its Agent Display. It covers all apps or a specified set of apps, identified by bundle identifier rather than process id or localized display name. An unresolved app cannot broaden the scope. It ends when revoked, when the display ends, or when the connection closes. Reconnecting, restarting the app or creating another display does not restore it. Another Agent Session cannot inherit it.

Authorization permits taking windows from the user's displays, never from another Agent Display or a Review Display. It does not authorize input on the user's displays. It does not bypass Keyboard Borrow restrictions or approve sending messages, making purchases or deleting content. The session must still follow the user's task instructions.

Taking a user window under either form of authorization is a Handover. The window keeps its return location and must return to the user when its display is dismissed. Normal placements of agent-owned windows retain their existing rules.

## Takeover, revocation and return

A Takeover pauses automatic acquisition. Explicitly handing control back through the interface or conversation resumes an authorization that has not been revoked. Granting window access alone does not end a Takeover. Waiting never ends one. See ADR 0004.

Revocation prevents future automatic acquisition and leaves existing Handovers in place. Returning Handovers is a separate action, available for one window or all of the session's handed-over windows. It does not close or return windows the agent created. A window the user takes back is excluded from continuing authorization until explicitly authorized again for that window. Regranting access to all apps or its app does not erase that exclusion.

The Viewer and Peek show the continuing authorization's scope and whether a Takeover has paused it. The user can revoke it directly without asking the agent. This matters when the agent is busy, disconnected or has misunderstood a conversation instruction.

Placement must check the current display, Takeover, authorization and source-window ownership again after asynchronous preparation. A request that started before revocation cannot rely on the old permission when it commits. A failed move restores the prior record and exclusions without undoing a newer user action. Only one Handover of a window can be pending, and each has its own identity so an old failure cannot erase a later Handover with identical contents.

Moves, frame writes and returns of one window run in order. An older frame write cannot follow a Take Back home and move the window onto the Agent Display again. A slow OS operation for one window must not block another window's operations or state reads on the interface thread. Tests exercise these decisions through the same model and movement rules used by conversation calls and interface actions.

## Existing attribution stays in place

ADR 0007 still determines where newly created windows go without an explicit instruction. Window Authorization adds a user-directed Placement path; it does not change the attribution guess or allow background input on a window left on the user's display. Without an explicit instruction or a matching continuing authorization, taking a user window still requires a Window Request.
