---
agent-screen: patch
---

Agents can use `act_on_element` with `increment` or `decrement` to adjust a slider or stepper by one app-defined step when the control supports accessibility adjustment. These actions do not move your pointer or fall back to a click. If the app does not confirm an action, the error asks the agent to inspect the value before retrying, since the adjustment may already have run.
