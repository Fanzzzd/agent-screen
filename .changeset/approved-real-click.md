---
default: minor
---

Some apps take a click sent in the background only partway. In WeCom, for example, it selects a chat in the sidebar but doesn't open it (#134). An agent that sees its click had no effect can now ask to make the same click with your pointer. The request shows on the same approval card as drags and names the click, the app and the window. Allow it and the window comes forward on the agent's display, your pointer clicks there, and the approval ends at once. Your pointer and front app come back. Decline it and nothing is clicked, not even in the background. Agents still click in the background first and ask only after a click visibly did nothing.
