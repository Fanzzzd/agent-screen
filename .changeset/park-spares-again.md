---
default: patch
---

Agent Screen now keeps every display of its own out of your screen's way after the displays change, including the hidden ones it keeps ready for the next agent. Before, a hidden display could end up beside your screen, and your pointer could slide onto it. A display that fails to park, or runs at the wrong size, is tried again a little later each time, instead of being left after three tries. While your displays sleep, Agent Screen no longer makes or parks hidden displays, which used to refuse agents' new displays for 10 minutes after you came back. And displays agents already ended no longer count against the limit on how many displays the Mac can handle.
