---
default: patch
---

`act_on_element` with `set_value` on a dropdown, such as a web page's select, now chooses the item it names in one call. It opens the menu, presses the item and checks that the dropdown shows it, with the menu open for about a tenth of a second. Agents used to open the menu in one call and pick in the next. macOS closes an open menu at your next click or key, even on your own screen, and sends your keys to it while it is open. A name that matches no item, or several, fails, lists the items and changes nothing.
