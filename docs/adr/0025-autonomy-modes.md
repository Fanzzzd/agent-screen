# The user picks how far agents cross into their territory: Ask, Auto or YOLO

Accepted, 2026-11-27.

An Agent Session crosses into the user's territory in three ways: it takes one of their windows (ADR 0013, conversation), it uses their pointer or keyboard (ADR 0008, ADR 0013, input queue), or it takes the front from the app they are in (the front hold). Each crossing had its own consent: a card, a button, or a rule of quiet. A user running several agents answered the same cards all day for windows they were not using and a mouse they had let go of. A user who had decided to let the agents run could not say so.

So the user sets one Autonomy Mode for the whole app. Ask is the behavior before this ADR, unchanged at every crossing. Auto watches what the user is using: it takes a window that is not In Use and uses the pointer and keyboard while they are At Rest, and it stops at the user's touch. YOLO takes them at once.

## What a mode decides

Six crossings, in `agent_screen_model::autonomy::decide`:

- A user window. Auto takes one the user has not clicked, typed into, scrolled or pointed at for 120 s and whose app plays no video and holds no call; for one In Use the Window Request waits as a line, with no card, and answers itself. YOLO takes any. Every take is a Handover and returns to the user.
- A window another live session holds stays with ADR 0017 in every mode. The monitor watches the user, and nothing tells Agent Screen whether the holder is in the middle of a step; two sessions in YOLO would pull the window from each other without end. A Review Display's window is asked for in Auto, since not yet reviewed is not unused, and moves at once in YOLO.
- A Kept Window, one the user took back or answered Keep for, and a window of a Protected App take the Ask path in every mode. Protected Apps are System Settings, Keychain Access, Passwords, password managers, the app each agent's client runs in, and what the user adds. The client's app is there because an agent that took its own client's window could press its own permission prompt.
- The Input Queue. In Auto the mode opens the Input Grant, with no card, once the hands have been At Rest for 5 s: no input, no button held, no input-method composition, no secure input, the screen unlocked. The Red Glow shows 400 ms before the first event. A touch ends the grant; after one, the next needs 30 s of rest. In YOLO the grant opens at once. A grant a mode opened ends as soon as nothing waits or runs.
- The front hold and Keyboard Borrow. In Auto, Agent Screen starts the front hold itself when the hands are At Rest, and a Keyboard Borrow waits up to 10 s for ADR 0008's 2 s of quiet. In YOLO both start at once.
- A Takeover and a Help Request are outside every mode. A mode decides what an agent may take. It does not answer a question the agent has for the user, and it does not end the user's own operation of a display (ADR 0004).

## A grant the user did not open restores, and holds their keys

ADR 0013 leaves the pointer and the front where they are when the user's touch ends a grant, because the user allowed it and then chose to act. A grant a mode opened took the front from a user who was not asked. Left that way, the agent's app stays in front and the user's next keys go into it. So such a grant always restores the pointer, the front app and its focused window, and it runs behind an armed hold: an active tap that holds the user's keys and presses from the first one until the front is back, then sends them on in order, a press at the pointer's saved place. The limits are the key hold's: 3 s and 512 events, and held keys go out only to the user's confirmed window, else they are dropped. After a hold, 1 s passes before another may start.

In Auto the first held event ends the grant. In YOLO the running loan gets 1.5 s more, the mouse is detached from the cursor for the loan, and then the grant ends and restores. This is the input suppression ADR 0013 ruled out, brought back for YOLO alone, for 3 s at most and with every event delivered. Without it YOLO would be Auto with no wait: the user's next move would revoke every loan, and YOLO would work only while they happened to hold still.

Because a touch no longer stops YOLO, a chord does: Control-Option-Command-Period, registered while the mode is Auto or YOLO and read by the hold's tap before it holds anything. It, Stop on the Input Bar and the menu's Stop item end the grant and return the mode to Ask. Stop that only ended one grant would be followed by the next 5 s later.

## What no mode changes

Background input stays first (ADR 0001): a mode removes the asking, never the reason for real input. Secure input, a macOS system prompt in front and a locked screen send nothing. Real input stays inside the Agent Display. The Red Glow shows before any real input and without it none is sent; it now shows for a Keyboard Borrow and a front hold too. A Takeover outranks the mode. Captcha and account consent stay with the user.

## Only the user sets it, and YOLO does not outlive the sitting

The mode is changed in Settings and the menu bar. No tool sets it and a relayed `user_instruction` does not either: ADR 0013's relay has never reached the user's input, and an agent that could relay "turn on YOLO" could give itself the pointer. The mode is global. The pointer, the keyboard and the front are one resource, and one answer is what the icon and the glow can show.

Ask and Auto are stored. YOLO is kept in memory and ends when the Mac locks or sleeps, when the user's session is switched out, and when Agent Screen restarts; the stored mode then applies. While it is on, the menu bar icon is red. A user away from the Mac loses nothing: with nobody there, every window is free and the hands are at rest, so Auto does what YOLO would.

## Considered options

- A mode per Agent Session. It needs a badge per display, a rule for one queue shared by sessions in different modes, and a guard against one session asking for another's level.
- YOLO that a touch still revokes. Simple, and no different from Auto without the wait.
- YOLO stored across restarts. An update or a reinstall would bring the Mac back in YOLO with nobody having chosen it that day.
- A longer or shorter rest. Under 5 s the pointer leaves during a pause to think; much longer and an agent's click waits on a user who is only reading.
- Showing the Red Glow for as long as YOLO is on. The glow would stop meaning "now".
- Letting Auto move a live holder's window after the holder has been quiet. An agent thinking between two steps is quiet too.

## Consequences

- In Auto and YOLO the Mac's front app and pointer can change with no click of the user's. The Red Glow, the Input Bar and the restoration are what make that acceptable, and all three are checked before input goes out.
- In YOLO the user's typing can freeze for up to 3 s at a time. It is delivered after.
- A composition with no candidate window, as Korean 2-set shows none, is not seen. A front change commits its syllable.
- A video paused in a front window stops counting as use 120 s after the last touch.
- Agent Screen now registers a global hotkey and, for mode-opened operations, an active event tap. macOS can disable a tap that runs late; the armed hold enables it again while it holds.
- `autonomy-<instance>.json` can be written by anything with the user's file access, an agent's shell included, which could set Auto for the next launch: the same trust the MCP clients' config files already rest on. A file that can't be read, or whose Protected Apps can't, loads as Ask, and none ever loads as YOLO.
