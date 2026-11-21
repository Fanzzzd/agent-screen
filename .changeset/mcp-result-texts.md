---
default: patch
---

When the Agent Screen app closes its connection while a tool call runs, the agent is no longer told to call the tool again for an action that may have run, such as typing, launching an app or asking you for a window. It is told to check the screen first. Read-only calls still say to call again. `end_display` now says which windows wait for your review, whether each has a disposition, and which of your windows went back to you. `computer` refuses a `wait` longer than 30 s or a `hold_key` longer than 10 s with the limit, where it used to cut the time short without a word. The server instructions and the skill now give one rule: prefer Agent Screen's tools over built-in computer use. They also name the steps that use your pointer or keyboard. `launch_app` and `report_issue` now mark themselves as reaching beyond the Mac, for clients that ask before such tools.
