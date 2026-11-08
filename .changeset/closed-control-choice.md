---
default: patch
---

Agents can choose an item in a pop-up button or a web page's select, such as a country or birth date field, with `act_on_element set_value` and the item's exact title (#79). Agent Screen types the title into the closed control, sent only to that app, and checks what the control shows afterwards. No menu opens, so your keyboard stays yours. If the control shows another item, the call fails and names it. A combo box gets its value and then a confirmation, so the app commits it, and the call fails if the app rejects it. When you take over the display, the choice stops before the next key. A text field that reports an error but shows the new value now counts as set.
