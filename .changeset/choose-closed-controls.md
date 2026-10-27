---
agent-screen: patch
---

`act_on_element set_value` can choose a pop-up button or HTML select by its full item title without opening a menu. It types into the closed control and checks the value. Combo boxes confirm the value after setting it, so the app receives the change.

`set_value` on a menu-only button now refuses before sending a value because the runtime menu guard is not enabled. Unsupported custom combo boxes report that the value may have changed and do not fall back to a click or open menu. A Takeover during role lookup or queued AX dispatch prevents the next write.
