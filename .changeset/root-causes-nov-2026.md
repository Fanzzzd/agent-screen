---
default: patch
---

- Creating a new Agent Display no longer shifts an existing Agent Display's position or hands its windows over to the new session or to an idle spare display. Spare displays now reuse the lowest display unit numbers first and keep their full resolution while idle, preventing duplicate virtual displays and false stream restarts.
- When an agent asks for a window that another agent holds, or when a request for that window times out or is declined, the error now names the display holding the window.
- Opening Peek on a display whose screen has been still now shows its picture right away instead of an empty tile with a window outline.
- Pop-up buttons, sheets and System Settings panes on an Agent Display now accept closed-menu selection, background menu item clicks, click-outside dismissal of stuck pop-up menus, and user-approved clicks and drags.
