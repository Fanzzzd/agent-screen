---
default: patch
---

When an agent opens a new window of a browser you are typing in, such as Dia, with `launch_app`, Agent Screen now asks for the window through the browser's menu instead of sending Command-N, which reached your window. Your window gets its focus back as soon as the new window reports the agent's address, not after the page finishes loading. Keys and clicks in the meantime, about a second in Dia and 3 seconds at most, are held, and delivered in order once your window has its focus back, none to the agent's. If your window can't get its focus back, they are dropped rather than sent to the agent's window. Chrome in Chinese gets its new window too. When the browser moves focus during an agent's click or text, the error now names the window that took it. In a browser you are typing in, a click on a web page's link or button that shows no change, as when Dia ignores the press, now says it can't tell whether the click acted instead of reporting success.
