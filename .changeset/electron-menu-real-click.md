---
default: minor
---

An agent can choose a menu item of an Electron app while a text field has focus. Electron takes an item pressed in the background as its keyboard shortcut, and many apps ignore shortcuts while you type, so the press did nothing (#39). Now such a press asks to use your pointer, on the same approval card as drags, and the card names the item. Allow it and the app's menus show on the agent's display, your pointer clicks the item there, and the approval ends once it is chosen. Your pointer and front app come back. Decline it and nothing is chosen. With no field focused, the press still runs in the background. Agents can ask for the click, or skip it, with `real_input` on `act_on_element`.
