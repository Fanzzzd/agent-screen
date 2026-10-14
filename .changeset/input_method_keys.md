---
default: minor
---

# Agents can type through your input method

`computer` `type` and `key` take `via_input_method`, so an agent can type pinyin or another input method's keys and see the composition, as when testing an input-method bug. For the call, the agent's app gets keyboard focus and the agent uses your keyboard; the Viewer and Peek say so. Sidescreen waits until you've left the keyboard and pointer alone for 2 seconds, stops when you touch either, and sends nothing while a password field or a system prompt has the keyboard. See ADR 0008.
