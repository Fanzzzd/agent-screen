---
default: patch
---

When an agent opens a new window of a browser you are typing in, such as Dia, with `launch_app`, Agent Screen now asks for the window through the browser's menu instead of sending Command-N, which reached your window. Your window gets its focus back as soon as the new window reports the agent's address, not after the page finishes loading, so less of your typing lands in the agent's window. If the new window never shows that address, focus comes back when the call ends, as before. Chrome in Chinese gets its new window too. When the browser moves focus during an agent's click or text, the error now names the window that took it.
