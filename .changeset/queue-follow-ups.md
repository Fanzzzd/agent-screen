---
agent-screen: patch
---

A Help Request's card now shows before the input queue's card, so a queue that cannot run no longer hides it. After you allow a batch and its approval lapses with nothing run, the card and notification don't ask about the same requests again; a new request or a revocation asks again.

A slider drag that grabs beside the thumb's centre now moves the value by the drag's distance, the way the drag goes, and a drag that goes nowhere leaves the value alone.

An agent that drags A, then B, then A again gets a fresh request for A instead of the replaced one; a declined or completed request still stands. One agent session can keep at most 64 input requests, so it can't fill the queue for others, and `input_queue` `done` no longer reports an error after it succeeded when given a request id: it now refuses the id before doing anything.

When an approval you gave lapses with nothing run, the agent now sees `approval_lapsed` and stops waiting, instead of waiting up to a minute for a card that won't come back.
