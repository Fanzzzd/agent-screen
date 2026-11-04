---
agent-screen: patch
---

An agent's `type`, `key` and `hold_key` now check which window will get the keys before sending any. When an app you use also has a window on an Agent Display, keys go to whichever of its windows has focus. If that is one of your windows and Agent Screen can't give the agent's window focus without bringing the app to the front, the call fails, names your window and sends nothing. The agent can then set the field's text with `act_on_element` instead. Agent Screen also has one more way to give the agent's window focus in the background, so more of these calls go through.
