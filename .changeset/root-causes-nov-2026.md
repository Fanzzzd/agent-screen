---
default: patch
---

- Creating or ending an Agent Display keeps existing Agent Displays in place and moves their windows along whenever a display must shift, so an agent's windows stay on its own display instead of ending up on another agent's display or out of sight.
- Dragging a window by its title bar in the Viewer keeps tall, wide, and sheet-bearing windows on that Agent Display instead of letting them cross onto another display or your screen.
- Pop-up menus inside sheets and System Settings panes now work with `act_on_element` value selection, background clicks, clicks outside the menu to close it, Takeover clicks in the Viewer, and approved clicks or drags with your pointer.
- When `place_window` cannot take a window on another Agent Display, the error now names every holding agent, display, session, last activity time, and whether that connection is still open. When an agent's MCP server restarts, Agent Screen keeps its displays for two seconds so the reconnected client resumes them on any tool call instead of losing them right away.
- Pending Help, Input, Window, and Window Authorization requests now show an orange count on the menu bar icon, highlight the approval card, and play a notification sound for Input Requests as well.
- Displays kept ready between tasks stay at their sharpness and size and are reused in a consistent order, reducing extra display flashes when an agent asks for a display and keeping Peek previews responsive without repeated restarts when a display's picture is unavailable.
