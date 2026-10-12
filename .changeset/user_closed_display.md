---
default: patch
---

# Closing a display stops the agent

When you close a display the agent is using, the agent is told you closed it and to ask before making another, instead of being told to create a new one. When Screen Recording needs Sidescreen to reopen, the menu offers **Finish Setup: Reopen Sidescreen…**, and agents are told to ask you to reopen it. `wait` reports the time it waited, at most 30 seconds.
