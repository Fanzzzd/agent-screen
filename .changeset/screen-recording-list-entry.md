---
default: patch
---

On a first install, Agent Screen now shows up in System Settings' Screen & System Audio Recording list once you press Turn On… in its Settings window. Before, Agent Screen started watching the keyboard at launch, before it had Accessibility, so macOS asked for Input Monitoring ("Keystroke Receiving") right away, and the Screen Recording request then waited behind that prompt without adding Agent Screen to the list. Agent Screen now starts watching input only once Accessibility is on, so no Input Monitoring prompt comes up. Turn On… always asks macOS and then opens the pane, whatever macOS said before. Agents that hit a missing permission now tell you to press Turn On… in Agent Screen's Settings window, since System Settings opened by hand may not list Agent Screen yet.
