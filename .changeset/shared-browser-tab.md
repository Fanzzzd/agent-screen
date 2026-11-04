---
default: patch
---

An agent working in its window of a browser you're also using can now press Tab and Shift-Tab there (#110). Focus moves to the next or previous field, button or link of its page through accessibility, and your window keeps its focus. The order follows the page's structure, so it can differ from the page's own tab order. Return in a web form's text field still doesn't submit the form, because the browser doesn't say which of the form's buttons submits it. The error now names those buttons, so the agent can click the right one.
