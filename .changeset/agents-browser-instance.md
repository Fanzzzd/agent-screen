---
default: minor
---

When an agent opens Chrome, Dia or another Chromium browser, Agent Screen now starts a separate copy of that browser for agents, on its own profile in Agent Screen's support folder, whether or not you use that browser yourself. Your browser being in front no longer stops the agents' browser work, the agents' copy never takes your focus, and agents never use your own profile. Drags and selects in its pages work, and a sign-in you do there once stays for later sessions, shared by all agents. It has none of your own browser's sign-ins, so an agent that needs your logged-in session still asks you to hand over a window. The agents' copy quits once no Agent Display holds its windows, and when Agent Screen quits. A browser that allows only one copy still opens agents' windows in your own browser: while you work in one of its windows, they ignore clicks, and the agent is told so and pointed to Chrome for long unattended work.
