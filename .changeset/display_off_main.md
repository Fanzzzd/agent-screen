---
default: patch
---

# Peek and the Viewer keep drawing while a display is made

macOS takes about half a second to make a virtual display. Sidescreen now does that off its main thread, so Peek and the Viewer keep drawing, and parks the display right after. `create_display` measured 771 ms median at load 27, down from 884 to 913 ms. `launch_app` notices a running app's new window up to 25 ms sooner. When another agent or you started an app moments ago, `launch_app` waits up to 3 seconds for its menus and then says it just started, instead of telling the agent to ask you to quit it.
