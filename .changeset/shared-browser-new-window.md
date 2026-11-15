---
default: patch
---

When an agent opens a new window of a browser you are typing in, such as Dia, with `launch_app`, Agent Screen now asks for the window through the browser's menu instead of sending Command-N, which reached your window. Your window gets its focus back as soon as the new window reports the agent's address, not after the page finishes loading. Keys you type in the meantime, about a second in Dia, are held and then delivered to your window in order, none to the agent's, for 3 seconds at most. Chrome in Chinese gets its new window too. When the browser moves focus during an agent's click or text, the error now names the window that took it. In a browser you are typing in, a click on a web page's link or button that the page doesn't act on, as Dia ignores such presses, now fails and points to `real_input` instead of reporting success.
