---
default: patch
---

When an agent works in one window of a browser while you use another window of it, a click that changes nothing in the agent's window, such as a link whose page cancels it, now tells the agent nothing changed instead of reporting success (#110). A click, press or Return on a select there no longer opens its menu, which took your keyboard; the agent is told to choose the item with `act_on_element set_value`.
