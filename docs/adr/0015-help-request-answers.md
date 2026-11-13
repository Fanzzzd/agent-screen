# A Help Request has four answers, and a Takeover is one of them

A Help Request used to end only when the user finished a Takeover. Users who had already done the step on their phone, or who only wanted to say which account to use, had to take over the display and hand it back to tell the agent to go on. A user who wanted to give the agent a window had to take over first too.

The user now answers a Help Request in one of four ways, from the Help Card or its notification:

- **Take Over.** The Takeover rules of ADR 0004 are unchanged: it ends only when the user hands control back, and the hand-back answers the Help Request. During a Takeover, only the hand-back answers it, so an agent never resumes while the user operates its display.
- **Continue.** The agent goes on without a Takeover. It is told the user may have done the step themselves.
- **Give a Window.** The card opens the Viewer's Give menu for that display. The user's own Handover to a display with an open Help Request answers it, and the agent learns which window it got. A Placement the agent makes on the user's word in chat, or under a Window Authorization, is the agent's act and answers nothing.
- **Reply.** The user types an instruction. Agent Screen relays the words unchanged and marks them as the user's. It does not read, check or rewrite them. This is the reverse of ADR 0013: there the agent relays the user's words to Agent Screen; here Agent Screen relays the user's words to the agent. In both, the relay is trusted and recorded, not proof of anything beyond what the user typed.

When the user answers in the conversation instead, the agent relays their words with `request_help` and `user_instruction`. That answers the request as Continue, through the same transition as the card's button.

An agent that withdraws its own Help Request is told the request was withdrawn. It used to read "You have control", as if the user had handed the display back.

The requests the user answers, Help, Control and Window Requests, live in one model module (`crates/model/src/request.rs`). Each has an id, so Later puts off that request and a new one shows the card again, even with the same reason. Each ends through one resolve step with a resolution that carries what the user gave, and the agent's wait receives it. An answer that comes while the agent is not waiting, after its call timed out, is kept for its next `wait_for_control`. The control socket's `waitForControl` reply carries `resolution` and `request` (ADR 0005); its older fields stay for an MCP server built before them.
