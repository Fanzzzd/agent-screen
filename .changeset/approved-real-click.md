---
default: minor
---

Some apps take a click sent in the background only partway. In WeCom, for example, it selects a chat in the sidebar but doesn't open it (#134). An agent that sees its click had no effect can now ask to make the same click with your pointer. The request shows on the same approval card as drags and names the click, the app and the window. Allow it and the window comes forward on the agent's display, and your pointer clicks there. When no other agent is using the approval, it then ends at once, and your pointer, your front app and the window you were using in it come back. If what is under the point changed while you decided, nothing is clicked and the agent is told to look again. Decline it and nothing is clicked, not even in the background. Right and middle clicks can't use your pointer, since the menu they open would close as soon as your front app comes back. Agents still click in the background first and ask only after a click visibly did nothing.
