---
default: patch
---

When a click in a browser window you work in goes in as an accessibility press, the agent is no longer told that the page sees no mouse events. In Chrome the page gets the same pointer, mouse and click events as from a click, so the agent no longer avoids controls that react to a press.
