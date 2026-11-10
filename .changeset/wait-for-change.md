---
default: minor
---

Agents can wait for a window to update with the new `wait_for_change` tool, instead of sleeping and taking another screenshot. One call watches a window, or one part of it, and returns as soon as anything in it changes, or an element with a given role or text appears or disappears, such as a "Delivered" mark replacing "Sending…". The answer lists only the elements that changed, in the same form as `act_on_element` with `result: "changes"`. A wait lasts 30 seconds unless the agent asks for up to 120, and also ends when the window closes or the display ends.
