---
default: patch
---

When an agent works in one window of a browser while you use another window of it, the answer to a click on a link or button in the agent's window now says whether anything in that window changed afterwards, so a link whose page cancels the click no longer reads as one that worked (#110). A click, press or Return on a select there no longer opens its menu, which took your keyboard; the agent is told to choose the item with `act_on_element set_value`. The agent's context menu there isn't opened either. When the browser doesn't say which of its windows has focus while one is on your screen, Agent Screen no longer sends the agent's click as a real click, which could take your window's focus; it presses through accessibility or refuses.
