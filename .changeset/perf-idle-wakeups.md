---
default: patch
---

Reduced idle CPU usage and wakeups when no Agent Display is open: stopped spawning a 2-second reconnect polling loop for in-process MCP hosts, made the cursor guard and alert watcher event-driven with zero idle timer wakeups, skipped hidden windows on mouse moves, checked parent PID before reading peer executable and arguments during orphan sweeps, and slowed idle permission and shell ticks.
