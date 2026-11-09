---
default: patch
---

Agents can choose an item in a pop-up button or a web page's select, such as a country or birth date field, with `act_on_element set_value` and the item's exact title (#79). No menu opens, so your keyboard stays yours.

- In a Chromium browser or Electron app the agent started with `launch_app`, a page's select is chosen directly through the app's private DevTools pipe, with one change event.
- In an AppKit app, Agent Screen types the title into the closed pop-up, sent only to that app, once the pop-up has the app's focus. It never types a space, so a title with one is refused. Keys stop if another window or field takes them, if the window leaves the display, or if a key would come too late. The app may run the pop-up's action for items shown on the way, and the answer says so.
- Anything else gets no key at all: a Safari or other WebKit page's select, a select in a frame from another site or in a browser you started yourself, a button that opens a menu, and Qt, Java or Flutter pop-ups. The call says why and suggests asking you. If Agent Screen can't tell what kind of control it is, it sends nothing.

If the control ends up showing another item, the call fails and names it. A control that already shows the title gets no keys. A combo box gets its value and then a confirmation, and the call fails if the app rejects it. A web page's combobox has nothing to confirm, so the call says the text is set but no option is chosen. When you take over the display, the choice stops before the next key and says what the control shows. A text field that reports an error but shows the new value now counts as set.
