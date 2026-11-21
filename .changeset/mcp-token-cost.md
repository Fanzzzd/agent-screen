---
default: minor
---

Agents spend fewer tokens on Agent Screen. The tool descriptions are about half as long: about 8,300 tokens for all tool definitions, down from about 11,400, which clients such as Codex, Cursor, Gemini CLI and OpenCode load at the start of every session. Rules for rare cases now come in the result or error text when the case happens, or in the skill. `computer` takes `screenshot: false` to skip the screenshot after a step the agent doesn't need to see, such as a run of key presses, and `get_window_state` takes `image: false` to return the tree without the window image. Both default to the image, so agents that don't pass them work as before.
