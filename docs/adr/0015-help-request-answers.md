# A Help Request has four answers, and a Takeover is one of them

A Help Request used to end only when the user finished a Takeover. Users who had already done the step on their phone, or who only wanted to say which account to use, had to take over the display and hand it back to tell the agent to go on. A user who wanted to give the agent a window had to take over first too.

The user now answers a Help Request in one of four ways, from the Help Card or its notification:

- **Take Over.** The Takeover rules of ADR 0004 are unchanged: it ends only when the user hands control back, and the hand-back answers the Help Request. During a Takeover, only the hand-back answers it, so an agent never resumes while the user operates its display. An answer the agent hasn't heard yet waits for the hand-back too. The card steps aside during a Takeover, but the orange menu bar icon and the notification stay, as the user may have collapsed the Viewer.
- **Continue.** The agent goes on without a Takeover. It is told the user may have done the step themselves.
- **Give a Window.** The card opens the Viewer's Give menu for that display. A Handover the user makes with Give, from the Viewer or the card, to a display with an open Help Request answers it, and the agent learns which window it got. Other Handovers answer nothing: a Window Request's Hand Over answers only that request, and a drag or a window the launcher rule of ADR 0007 places may be meant for something else. A Placement the agent makes on the user's word in chat, or under a Window Authorization, is the agent's act and answers nothing.
- **Reply.** The user types an instruction, up to 2000 characters. Agent Screen relays the words exactly as typed, spaces and pasted line breaks included, and marks them as the user's. It does not read, check or rewrite them. A reply from the notification that is too long opens the card's field with the draft. This is the reverse of ADR 0013: there the agent relays the user's words to Agent Screen; here Agent Screen relays the user's words to the agent. In both, the relay is trusted and recorded, not proof of anything beyond what the user typed.

Every answer from the card or the notification names the request the user saw. An answer to a request the agent has replaced since changes nothing, and the card drops its draft.

Only an agent whose MCP server says it understands these answers gets them. The new server sends `answers: true` with `requestHelp`. A server built earlier reads every answer but a timeout as "You have control" and would drop a Reply's words. ADR 0005 keeps a running MCP process on its old build after the app is replaced, so every live session would hit this right after an update. For a request without the flag, the card and the notification offer Take Over and Later only, as before, and Continue, Reply and Give a Window answer nothing.

When the user answers in the conversation instead, the agent relays their words with `request_help` and `user_instruction`. That answers the request as Continue, through the same transition as the card's button.

An agent that withdraws its own Help Request is told the request was withdrawn. It used to read "You have control", as if the user had handed the display back.

The requests the user answers, Help, Control and Window Requests, live in one model module (`crates/model/src/request.rs`). Each has an id, so Later puts off that request and a new one shows the card again, even with the same reason. Each ends through one resolve step with a resolution that carries what the user gave, and the agent's wait receives it. An answer that comes while the agent is not waiting, after its call timed out, is kept for its next `wait_for_control` or `request_help`. Asking again returns that answer at once instead of asking the user twice. The control socket's `waitForControl` reply carries `resolution` and `request` (ADR 0005); its older fields stay for an MCP server built before them.
