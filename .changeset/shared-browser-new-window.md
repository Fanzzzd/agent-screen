---
default: patch
---

When an agent opens a new window of a browser you are typing in, such as Dia, with `launch_app`, Agent Screen now asks for the window through the browser's menu instead of sending Command-N, which reached your window. Your window gets its focus back as soon as the new window shows the agent's page, not after the page finishes loading, so less of your typing lands in the agent's window. When the browser moves focus during an agent's click or text, the error now names the window that took it.
