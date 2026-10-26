# An agent's drag holds the session's left button

Proposed with #37; it needs the user's approval.

ADR 0001 posts input to the target process and leaves the user's pointer alone. A drag posted that way reaches the app as `LeftMouseDragged`, but Chromium, Electron and WebKit build a web pointer event's `buttons` from `[NSEvent pressedMouseButtons]`, WindowServer's button state for the whole session. Events posted to one process don't change it, so web pages saw the agent's drags with no button held, and HTML5 drags ended as they began.

For an agent's drag, Agent Screen holds that state down: it posts a left press at the session tap, at the cursor's own location, and its event tap at the head of the annotated session tap drops the press before any app gets it. The release goes the same way. `docs/research/37-web-drag.md` has the experiments.

This bends ADR 0001 for the length of the drag. The cursor doesn't move, no app is activated or raised, and no app receives the press, but every app that asks reads the left button as held. So:

- It holds for a gesture's events only: about 0.6 s for `left_click_drag`, and a step plus 60 ms for `left_mouse_down` and each `mouse_move` while the agent holds the button. Between tool calls the button reads up.
- Nothing is posted unless the tap that drops the press is running. Without Accessibility there is no tap, and drags behave as before.
- When the user presses their own button during the hold, Agent Screen leaves the release to them.
- A Takeover lets go at once, before the drag's release.
- The Viewer doesn't use it: the user's own button is down there.

The alternatives were Chrome DevTools Protocol input for apps the agent launched, which covers only Chromium apps started with a debugging switch, and lending the user's real pointer for the drag after they allow it, which moves the cursor. Both stay open for cases the hold doesn't reach, such as a native drag into another process.
