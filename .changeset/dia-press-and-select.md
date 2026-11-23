---
default: patch
---

An agent's `act_on_element` press in Dia no longer reports success when nothing happened. Dia's page elements offer no accessibility press and ignored every one, so a press on such an element now goes in as a background click at its center, and the agent is told. While your Dia window has focus, it waits in the Input Queue as a click, as a `computer` click already does. An agent can also choose an option of a page's select in Dia, or in another Chromium browser you started yourself, without opening the menu: `set_value` types the option's title into the closed select. A title with a space is still refused, since a space opens the menu, and nothing is typed while your window of that browser has focus.
