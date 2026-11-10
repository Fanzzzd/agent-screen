# Changelog

## 0.1.27 (2026-11-10)

### Fixes

- An agent can click and type in an app that is your front app when the app's focused window is on the agent's display, such as WeCom with its only window handed over. Every click and key used to be refused as reaching "the user's window", which was that handed-over window. When the app's focused window is on your screen, or the app has another window there, input is still refused as before. When `place_window` finds no window of a running app, its error now says that the user can reopen a closed main window from the app's Dock icon.
- A window you handed over goes back to where it came from as soon as the agent ends its display, or its session disconnects. It used to stay on the ended display until you dismissed it there, and the same agent's next `place_window` for that app failed, saying other agent sessions held the window when there were none. The error now says which display holds a window: another agent's, one waiting for your review, or one that isn't the agent's.

## 0.1.26 (2026-11-10)

### Fixes

- `launch_app` with a file path for a browser that is already running opens the file in a new window on the agent's display, as it does a URL. The browser used to open it as a tab of its front window, which may be yours, and the agent got `still_opening` with no window.
- `place_window` reports the frame a window ended with when the app answers a resize with an error but takes it anyway, as some apps such as Dia do. It used to fail with "the app rejected the change" for a window that had moved and resized. It fails only when the window kept its frame, and then says which frame that is.
- An agent can drag a file from one window to another on its display, such as from Finder onto a web page's upload area. In 0.1.25 the drag failed both ways. In the background it reported success while it only selected the file, since only your pointer starts the system drag session that carries a file. With your pointer it stopped at once after you allowed it, saying the drag was outside its authorized window, because it had to end in the window where it began. Now a drag between two windows asks for your pointer on the approval card instead of claiming it ran. Allowed, it may end in any window on the agent's display and cross the windows between them, but never leaves that display. If the window it drops on moved while you decided, nothing is pressed.
- `launch_app` finds Finder and the other apps in /System/Library/CoreServices by name, and any other app macOS knows by name. It used to answer `not_found` for "Finder" and work only with `com.apple.finder`.

## 0.1.25 (2026-11-10)

### Fixes

- Your Chinese, Japanese or Korean input method composes in the Viewer after you click on the display's picture. In 0.1.24, a click on the picture brought Agent Screen to the front but left the Viewer without the keyboard focus an input method needs, so it composed nothing there: the letters you typed went to the app as typed, such as "nihao" into WeCom's message box, and a number typed a digit instead of picking a candidate. Now the Viewer takes the keyboard focus before Agent Screen comes to the front. The candidates show at the app's text cursor in the Viewer, and the app gets only the text you pick. The Viewer no longer draws a box with the letters you are typing over the picture; the input method's own window shows them.

## 0.1.24 (2026-11-10)

### Fixes

- Your Chinese, Japanese or Korean input method works in the Viewer during a Takeover (#117). Before, the Viewer sent every key straight to the app, so the input method never saw one and you could type only letters. Now the input method composes in the Viewer: the text you are typing shows at the app's text cursor, underlined, with the input method's candidates under it, and the app gets only the text you pick. With a keyboard layout such as ABC, keys go to the app as before. Keys the input method passes through, such as the arrows, Return and Escape when you aren't composing, and shortcuts with Command or Control, go to the app as keys. In a password field, keys skip the input method, including one you reach with Tab or a page focuses by itself. Korean and Vietnamese Return, Tab and arrows after a syllable commit it and still reach the app. Clicking elsewhere on the display or leaving the Viewer commits the text you were composing, as in a native field. Handing the display back drops it.

## 0.1.23 (2026-11-10)

### Fixes

- Agent Displays work again on a Mac whose screens' tops don't line up, such as a built-in display placed lower than the main one. Since 0.1.20 every `create_display` failed there with "macOS could not create a virtual display at a verified safe position" (#155). Agent Screen now parks a new display at the top-right corner of your rightmost screen, where macOS keeps it, and accepts the position macOS chooses when that position touches your screens at most at a corner. A display that would share an edge with one of your screens, overlap one, or sit between them is still refused, so your pointer can't slide onto it.
- When an agent works in one window of a browser while you use another window of it, the answer to a click on a link or button in the agent's window now says whether anything in that window changed afterwards, so a link whose page cancels the click no longer reads as one that worked (#110). A click, press or Return on a select there no longer opens its menu, which took your keyboard; the agent is told to choose the item with `act_on_element set_value`. The agent's context menu there isn't opened either, and the agent can't press an item of the browser's menu bar, such as Close Tab, whose command would act on your window. When the browser doesn't say which of its windows has focus while one is on your screen, Agent Screen no longer sends the agent's click as a real click, which could take your window's focus; it presses through accessibility or refuses.

## 0.1.22 (2026-11-09)

### Fixes

#### Agents can choose an item in a pop-up button or a web page's select, such as a country or birth date field, with `act_on_element set_value` and the item's exact title (#79). No menu opens, so your keyboard stays yours.

- In a Chromium browser or Electron app the agent started with `launch_app`, a page's select is chosen directly through the app's private DevTools pipe, with one change event.
- In an AppKit app, Agent Screen types the title into the closed pop-up, sent only to that app, once the pop-up has the app's focus. It never types a space, so a title with one is refused. Keys stop if another window or field takes them, if the window leaves the display, or if a key would come too late. The app may run the pop-up's action for items shown on the way, and the answer says so.
- Anything else gets no key at all: a Safari or other WebKit page's select, a select in a frame from another site or in a browser you started yourself, a button that opens a menu, and Qt, Java or Flutter pop-ups. The call says why and suggests asking you. If Agent Screen can't tell what kind of control it is, it sends nothing.

If the control ends up showing another item, the call fails and names it. A control that already shows the title gets no keys. A combo box gets its value and then a confirmation, and the call fails if the app rejects it. A web page's combobox has nothing to confirm, so the call says the text is set but no option is chosen. When you take over the display, the choice stops before the next key and says what the control shows. A text field that reports an error but shows the new value now counts as set.

## 0.1.21 (2026-11-09)

### Fixes

- Recover input monitoring after a macOS event-tap timeout. Previously, Allow could appear to do nothing because the app immediately revoked every new approval after that timeout. The app now re-enables and checks the input monitor after ending the previous grant and releasing its inputs. Queued actions still need a fresh Allow; recovery never resumes an old approval.

## 0.1.20 (2026-11-09)

### Features

- Some apps take a click sent in the background only partway. In WeCom, for example, it selects a chat in the sidebar but doesn't open it (#134). An agent that sees its click had no effect can now ask to make the same click with your pointer. The request shows on the same approval card as drags, names the click, the app and the window, and marks the point on the display's picture, with a ring for each press of a double or triple click. Allow it and the window comes forward on the agent's display, and your pointer clicks there. When no other agent is using the approval, it then ends at once, and your pointer, your front app and the window you were using in it come back. If what is under the point changed while you decided, nothing is clicked and the agent is told to look again. Decline it and nothing is clicked, not even in the background. Right, middle and Control clicks can't use your pointer, since the menu they open would close as soon as your front app comes back. Agents still click in the background first and ask only after a click visibly did nothing.
- An agent can choose a menu item of an Electron app while a text field has focus. Electron takes an item pressed in the background as its keyboard shortcut, and many apps ignore shortcuts while you type, so the press did nothing (#39). Now such a press asks to use your pointer, on the same approval card as drags, and the card names the item. Allow it and the app's menus show on the agent's display, your pointer clicks the item there, and the approval ends once it is chosen. Your pointer and front app come back. Decline it and nothing is chosen. With no field focused, the press still runs in the background. Agents can ask for the click, or skip it, with `real_input` on `act_on_element`.

#### After you approve the input queue, complete web drags that have no background route can use your pointer, one at a time. The approval card now looks like the other cards from the menu bar icon: it names the agent that wants your mouse and draws each drag's path on its display's picture, numbered when there are several. While approval lasts, a thin red edge with a soft inner glow runs around the inside of your screens, following a 14- or 16-inch MacBook Pro display's rounded corners and notch. The card shows which step runs, and a small pill at the bottom of the screen names the agent using your mouse, or how long the queue has been idle, with a Stop button. Move your mouse or press a key to take control back. A stopped drag releases its button and reports that it stopped. When approval ends normally, your pointer and front app come back.

Background input stays first and needs no approval. Split presses held across calls and real-keyboard actions are not executed by this queue.

### Fixes

- Agents can use `act_on_element` with `increment` or `decrement` to adjust a slider or stepper by one app-defined step when the control supports accessibility adjustment. These actions do not move your pointer or fall back to a click. If the app does not confirm an action, the error asks the agent to inspect the value before retrying, since the adjustment may already have run.
- Agents can pass `root` to `get_window_state` to read one numbered element's subtree, or to `act_on_element` to scope its returned state. `max_depth` now stops the accessibility traversal before deeper elements spend its budget. A shallow read marks each branch it left unread with how many children it has, so it can find the message pane after a long conversation list, then a rooted read can reach its contents. Partial reads keep other element numbers available, and `result: "changes"` compares only reads with the same root and depth. A stale root fails before an action runs, and an action on an app too busy to answer Accessibility fails without running instead of reporting the element gone. If an action removes the subtree requested for its result, the reply still acknowledges that the action ran.
- When `launch_app` starts Chrome or another Chromium browser, its first window opens on the Agent Display. Before, it was made on your main screen and moved, and about one launch in three showed it there for 40 to 70 ms (#123).
- With the AeroSpace build for Agent Screen, a window that leaves an Agent Display now stays claimed until it is on your screen. Before, Agent Screen released the window first and then moved it. If the move failed, the window stayed on a display you can't see, and AeroSpace, which had taken it back once before, left it there. Now, when a window you take back can't leave the display, Agent Screen tells you, the window stays in the Viewer with its place to return to, and Take Back can try again. An agent that returns a window on your word hears that it didn't leave.
- Agent Displays park in a compact grid off the top-right corner of your screens instead of one row that grew to the right with every display, so System Settings → Displays shows your own screens at a usable size and the same displays always land in the same places. A window zoomed to fill an Agent Display is moved below the strip that holds your menu bar, so its toolbar can be clicked (#103).
- A display made to fill a gap in macOS's display numbering is no longer kept when it shares its number with another Agent Display. Kept, it stopped Agent Screen from capturing that display for as long as both were online, and the Viewer showed its last small thumbnail stretched up and blurry. A display Agent Screen can't capture for that reason now says so in Peek and the Viewer instead of showing an old frame.
- An app that `launch_app` starts and that opens its main window a few seconds after a loading window, as WeCom does, now opens that window on the Agent Display. Before, if the app was too busy to answer Accessibility as the window appeared, the window stayed on your screen and AeroSpace tiled it into your workspace. For 30 seconds after the agent starts an app, its new windows go to the agent's display unless you click or type in that app, Finder, the Dock or a launcher. If you click or type in that app, Finder, the Dock or a launcher as it opens a window, or take a window of it back, the 30 seconds end, and if you hide the app, Agent Screen leaves it hidden.
- Peek's card no longer flashes wider after a closed display's tile has shrunk out, and a new display's tile grows in once instead of twice. A closed display's tile now shrinks out with its last picture and label instead of turning into a blank box, and Peek's shadow and outline no longer trail behind the card while it changes width.
- `place_window` finds an app's window by the app's name as you see it, its bundle id, its .app path, or the owner name macOS gives its windows, in any case. An agent that asked for WeCom as 企业微信 or `com.tencent.WeWorkMac` got `not_found`, because its windows are listed as "WeCom". When `place_window` finds no window, it now tells the agent why: no app has that name or bundle id, the app isn't running, it has no window on screen, its only windows are dialogs, its window is already on the agent's display, or the window is minimized, hidden, a dialog, or on another agent's display.
- Recordings lists videos newest first, with each display's label and start time. Dragging the progress bar no longer blanks the picture: the video pauses, keeps its last frame and resumes on release if it was playing. Space, the arrow keys and ⌘W work in the Recordings window, and Show in Finder reveals the file. The menu bar icon's menu groups its items and adds Report a Problem…, which copies this build's diagnostics after asking and opens the bug report form.
- Every explicit click on an app's Dock icon can show its orange window-location reminder again, including repeated clicks within five seconds. A reminder already open restarts its five-second close timer from the latest trigger instead of disappearing on the first trigger's deadline. Automatic app activation remains limited to one reminder a minute, and a pinned Peek stays pinned.
- Agent Screen verifies that a new or reused Agent Display is above and to the right of your screens before giving it to an agent. A taller reused display is moved there first. If macOS does not accept that position after retrying, creation fails and releases the display instead of leaving an invisible screen beside yours for your pointer to enter.
- A scroll of many wheel clicks now moves a page by all of them. Agent Screen sends them a few lines at a time, a frame apart, as a mouse wheel does; Chrome scrolled at most one page for a single event, so 60 or 800 clicks moved a long list no further than about 15 (#104).
- An agent's `type`, `key` and `hold_key` now check which window will get the keys before sending any. When an app you use also has a window on an Agent Display, keys go to whichever of its windows has focus. If that is one of your windows and Agent Screen can't give the agent's window focus without bringing the app to the front, the call fails, names your window and sends nothing. The agent can then set the field's text with `act_on_element` instead. Agent Screen also has one more way to give the agent's window focus in the background, so more of these calls go through.
- An agent can keep working in its window of a browser you're using in another window (#110). It used to have its keys refused while that browser was your front app, and its clicks there were sometimes lost. Now Agent Screen sends no keys or clicks to that browser at all, since they would reach your window. Instead, a plain left click presses the button or link under it, or picks the text field under it, through accessibility. Typed text goes into the picked field and is read back. Return, Escape, Backspace and Command-A act on what the click picked. Other keys, such as Tab or shortcuts, still fail, and the error names the window that has focus. Your window keeps its focus and selection throughout.
- An agent working in its window of a browser you're also using can now press Tab and Shift-Tab there (#110). Focus moves to the next or previous field, button or link of its page through accessibility, and your window keeps its focus. The order follows the page's structure, so it can differ from the page's own tab order. Return in a web form's text field still doesn't submit the form, because the browser doesn't say which of the form's buttons submits it. The error now names those buttons, so the agent can click the right one.
- Pressing a menu item in an app built on VS Code, such as Cursor or Windsurf, now answers with a note: the app ran the item as its keyboard shortcut in whatever has focus, so with the terminal focused Edit > Find opens the terminal's find box. The note points to the Command Palette to run the command itself (#39).
- After you take a window back, hand it over and take another window of that app back, the app's new windows open on the screen the latest window returned to. Before, they could open on the screen from an earlier Take Back.

#### Unsupported web drags now create an approval request in a queue shared by agent sessions, without sending a mouse-down first. The card names the agent that wants your mouse, shows each request, spells out what approval covers, and fits its content. During approval it shows a live thumbnail, and lets you stop and take control. New requests share an active approval until 30 seconds of queue inactivity. Agents can inspect, wait for or withdraw their own requests with `input_queue`; disconnecting or ending a display clears its work. This build provides approval and scheduling only. It has no real-input executor, and approval never reports an unexecuted drag as completed.

A drag can also use a slider's accessibility value when its range, orientation and current thumb position determine the direction. Ambiguous controls stay untouched. A value write must be confirmed before it is reported as successful; an uncertain result does not trigger a second drag. Successful drags identify their background route.

#### A Help Request's card now shows before the input queue's card, so a queue that cannot run no longer hides it. After you allow a batch and its approval lapses with nothing run, the card and notification don't ask about the same requests again; a new request or a revocation asks again.

A slider drag that grabs beside the thumb's centre now moves the value by the drag's distance, the way the drag goes, and a drag that goes nowhere leaves the value alone.

An agent that drags A, then B, then A again gets a fresh request for A instead of the replaced one; a declined or completed request still stands. One agent session can keep at most 64 input requests, so it can't fill the queue for others, and `input_queue` `done` no longer reports an error after it succeeded when given a request id: it now refuses the id before doing anything.

When an approval you gave lapses with nothing run, the agent now sees `approval_lapsed` and stops waiting, instead of waiting up to a minute for a card that won't come back.

A slider in a Chromium page with no DevTools pipe, such as Electron or Chrome started outside `launch_app`, now moves when an agent drags it: Chromium ignored the value Agent Screen wrote, so the drag went to the input queue. It now moves by the slider's own steps, each confirmed.

## 0.1.19 (2026-10-29)

### Features

#### Tell your agent to continue, pause, take a window or return handed-over windows in the conversation. It can relay your explicit instruction without asking you to click a second confirmation. Agent Screen trusts the agent to relay your words; it does not read or independently verify the chat.

You can also authorize the session to take windows from selected apps or all apps. Viewer and Peek show the scope and a Revoke action. Taking over pauses automatic acquisition. Revoking stops future acquisition without moving existing windows, and authorization expires when the display ends or the agent disconnects. A window you take back stays excluded until you explicitly give it again.

Repeated requests to hand control back keep one pending request without another notification. Only your explicit hand-back resumes the agent.

## 0.1.18 (2026-10-28)

### Features

- `get_window_state` and `act_on_element` take `max_depth`, which leaves out elements nested deeper and says how many. `act_on_element` takes `result`: `changes` returns only the elements that appeared, changed or went away since the window's last tree, with the window image, and `none` returns only "Done.". The default still returns the whole window state. On Calculator, a press with `changes` returned 166 bytes of text in place of 1,859.
- `place_window` takes `x`, `y`, `width` and `height` to move or resize a window on the agent's display, in screenshot pixels from the display's top-left corner. It returns the frame the window ended with, since apps keep to their own minimum sizes, and says when that isn't the frame asked for. An agent no longer needs System Events to resize a window, where a position meant for the display moved the window onto the user's screen.
- Recordings… in the menu bar icon's menu opens a local video library. Select a recording to play, pause or seek in the app. Show Pointer toggles a visual overlay without changing the video, sound or playhead. New recordings store timed pointer coordinates; older videos still play when their timelines cannot accurately replay a pointer. The overlay hides during Takeover because user pointer movements were not recorded. New recordings request HEVC/MP4 when macOS advertises it and use H.264/MP4 when HEVC is unavailable. The recording result reports the selected codec. Hardware encoder use and same-quality size or CPU improvements have not been verified for ScreenCaptureKit. Sound still joins through passthrough export, and the pointer stays outside the encoded video.
- `request_control` lets an agent ask for its display back while you are operating it. You get a notification and a card under the menu bar icon with its reason, and answer Let It Continue, which hands the display back, or Keep Control, which tells the agent you kept it. Nothing else ends your Takeover: an unanswered request stays open, and the agent can withdraw it. When the agent already has control, the call returns at once.
- Agent Screen now updates itself. It checks for a new version once a day, downloads it in the background and installs it once no Agent Display is open, so an update never ends an agent's work. The menu bar icon's menu has **Check for Updates…**, and **Restart to Update…** once an update is waiting; with Agent Displays open, it asks before the restart ends them. This version is the last to install by hand.
- Drags in web pages work in Chromium and Electron apps an agent starts with `launch_app`: splitters, sliders, sortable lists and HTML5 drag and drop. Before, every move reached the page with no button held, so the page took the drag for hover. `launch_app` now starts these apps itself with a private DevTools pipe that only Agent Screen holds. No port opens. A drag goes through the pipe inside the app, so your mouse button and pointer stay yours. For a browser, the agent starts a new instance on a throwaway profile. A drag in a web page of an app without the pipe, such as Safari or an app you opened, now fails with a message that suggests `launch_app` or `request_help`. Before, it seemed to succeed and did nothing.

### Fixes

- The Apple Account sign-in alert ("Your Apple Account is being used to sign in to a new device", with Don't Allow and Allow) and its verification code window now move to your main display when macOS opens them on an Agent Display. Before, they stayed on the agent's display, where you didn't see them and an agent could answer them.
- Collect build information for a bug with the MCP tool `get_diagnostics` in the failing session, or `AgentScreen diagnostics --json` for the CLI and responding app. Reports use compiled package versions and source revisions, distinguish stale or unknown builds, and leave the app stopped if it is not running. The bug report form and reporting guide explain older-build fallbacks and what to attach.
- The Help Card and a Window Request's card end a long reason after two lines with "…". Before, the rest of the text ran on in the second line past the card's edge and was cut mid-letter. The Viewer is at least 640 pt wide, so its header's buttons fit for a tall, narrow display. A 640×1600 display made a 365 pt Viewer that pushed Give, Take Back and the close and collapse buttons past its edge.
- Agent Screen retries an AeroSpace claim-file write after it fails, even when the next request repeats the same claim. It keeps the intended displays and windows separate from the last published file, so a failed display registration does not prevent later window claims.
- The display log in `~/Library/Logs/Agent Screen/displays.log` records when the app starts and quits, and a `released … quit` line for each display still connected at quit. Before, most displays left with the process and the log never said so, so a slow or frozen Mac couldn't be matched to those display changes.
- When you bring up an app whose windows are all on Agent Displays, from the Dock or a launcher, Peek drops down from the menu bar icon and shows where they are: that display's tile glows orange with the app's windows outlined. Click the tile to open it in the Viewer, or Take Back to bring the windows to your screen. Peek goes after 5 seconds unless your pointer is on it. Clicking the app's Dock icon does this too, also when the app is already the front app. Before, the app seemed to open with no window. It shows at most once a minute for an app, and not when an app the agent started brings itself forward.
- Pressing a menu item of an Electron app with `act_on_element` now comes back with a note. Electron tells the app that the press was the item's keyboard shortcut, and some apps ignore shortcuts while a text field has focus. The note, the tool's description and the agent instructions all say to click outside the field first. Agent Screen can't make the press look like a click: macOS runs an accessibility press with no current event.
- An Agent Display keeps the size it was made with. When macOS switches it to another mode by itself, as it did under heavy load from 1600×1000 to 800×600, Agent Screen switches it back as soon as macOS reports the change, and notes it in the display log (`~/Library/Logs/Agent Screen/displays.log`).
- `launch_app` with arguments or an environment takes the instance it started. Every checkout of an Electron project runs the same Electron.app, and a copy another tool started from a shell during the launch could be taken for it: that copy's windows went to the agent's display, and the agent's own window was lost. Agent Screen now picks the new process launchd started with the arguments it asked for.
- `launch_app` with arguments or an environment, on an app that allows only one instance and already runs, answers with the running instance's window on your display and says the app was already running. It used to wait about 10 seconds and answer that the app was still starting, or fail with "did not start". When the running instance has no window on the display, the error says the app is already running.
- When you open an app from Spotlight, Raycast or Alfred while you work in a Viewer, its window now leaves your screen within about 70 ms. Before, it showed there for 150 to 430 ms, and AeroSpace re-tiled your windows around it meanwhile. Agent Screen now checks for the new window every frame instead of every 150 ms, and moves it before it asks AeroSpace anything.
- macOS closes an app's open menu, such as a web page's select, at your next click or key anywhere, on your own screen too. An agent that opened one on its display and then clicked or typed to choose an item hit the page under the closed menu instead, or changed the field without seeing it. Now that click or key fails, says the menu closed, and nothing is sent.
- Clicking the menu bar icon after hovering now pins Peek instead of closing it. A later click closes the pin. Hover opening uses a 300 ms dwell, and an unpinned Peek uses a 500 ms grace period after the pointer leaves both the icon and Peek. A left-button press keeps its original intent while hover timers run, so its release cannot turn a close into a reopen.
- `place_window` answers `placed` only once the window has stayed on the display for half a second after its last move. When the app keeps moving it back to the user's screen, the call fails and says so. A window of the agent's that leaves the display later, as a UTM VM window did, is listed by `list_windows` with `strays: true` and `place_window` places it again. Before, `place_window` could answer `placed` for a window that was already on its way back, and then refused it, or asked the user for it as if it were theirs.
- Clicks to remote panels drawn by an app's background service now use the target window's current frame when macOS omits it from a targeted window lookup. Before, the event could carry global coordinates in place of window-local coordinates. The fallback checks both the process and window id; it does not use the visible host's frame.
- A new window that an app the agent started opens by itself, long after the agent's last action, goes to the agent's display when none of the app's other windows is on your screen. Before, it opened on your screen until the agent called `place_window`. UTM did this on every guest reboot, opening a new VM window each time.
- The Agent Pointer in the Viewer and Peek uses a new original cubic arrow with a rounded cap, shoulder and heel. Uniform light and dark outlines keep the theme accent visible on different backgrounds. Existing movement and click feedback stay shared, and the image boundary clips the pointer and ripple without moving the hotspot. The Mac's system cursor is unchanged.
- An agent's screenshot no longer shows your menu bar's status items, such as the clock, battery and CPU, right after an app on its display quits or a window there changes. macOS moves the same status item windows from your screen onto the Agent Display at those moments, and Agent Screen now checks where they are at each capture.
- An app on an Agent Display that brings itself to the front, as Electron apps do when they open a window, no longer keeps your keyboard. Your app is back in front about 10 ms later. Before, the agent's app stayed in front for 2 to 4 seconds. A click in the Dock or a launcher that opens the app still brings it forward.
- An explicit `place_window` frame now stays below the reserved menu strip, so an agent can move or shrink a zoomed window whose toolbar became unreachable. The result reports the actual frame and explains when the constraint shrinks a dimension. For a window already on the display, placement without frame arguments keeps its frame. Shared-browser key refusals now describe foreground/focused-window evidence instead of claiming the user is active. Browser drag guidance explains that a fresh profile has separate sign-ins and cannot add a pipe to an existing window.
- Agent Screen no longer quits when you click a field in the Viewer and paste into it during a Takeover. If the Viewer gets redrawn while it is still handling your click or key, it now finishes handling the input first and then applies the new layout.
- `computer type` checks the field after typing. A page that drops keys arriving faster than its script handles them, as an address autocomplete did, used to lose characters while the call reported success. Now, when the focused field's text lacks some of the typed characters, typing stops, and the call fails with how far it got and what the field holds. Typing into a busy page waits until the page shows the text.
- Viewer clicks can be traced with `AGENTSCREEN_VIEWER_CLICK_DIAGNOSTICS=1` at startup. The opt-in stderr records link a press and release to their coordinate mapping, window target, focus handshake and event stamp. They contain no text, window titles, keys or screenshots. Diagnostics are off by default and do not confirm that the target app executed an action.

## 0.1.17 (2026-10-25)

### Fixes

- Agents ask you to confirm a captcha immediately before they solve it, then fill it when they can. They still open a Help Request if you prefer to do it or they cannot. Confirming the challenge does not approve a new account.
- create_display and list_windows now return scale, screenshot pixels per display point, so an agent can tell why a window's size in the tools is smaller than the app reports on a large display.
- The `key` action types `]` and `[` on Macs whose keyboard is JIS. It used to send `[` and `@`, so `cmd+]` went Back instead of Forward.

## 0.1.16 (2026-10-25)

### Features

- The menu bar icon's right-click menu is shorter: New Display, Settings… (⌘,) and Quit, plus whatever needs fixing, such as unfinished setup or an AeroSpace workspace on an agent's display. Settings groups the permissions, connecting Claude Code and Codex, Smooth Display Changes, the Agent Display wallpaper and AeroSpace. The wallpaper row shows the picture you chose.

### Fixes

- Agent Screen has a new graphite app icon. On macOS 26 it follows the system's Dark, Tinted and Clear icon styles.
- A click on the menu bar icon closes Peek when hover opened it. Before, the first click kept it up and only the second closed it.

## 0.1.15 (2026-10-22)

### Features

#### Sidescreen is now Agent Screen

The app has a new name and a website, [agentscreen.app](https://agentscreen.app). It installs as Agent Screen.app from the tap `fanzzzd/agent-screen`: `brew tap fanzzzd/agent-screen && brew trust fanzzzd/agent-screen && brew install --cask agent-screen`. Agent Displays are named "Agent Screen 1", "Agent Screen 2" and so on, and the agents' MCP server is `agent-screen`.

### Fixes

- With the AeroSpace build from Fanzzzd/AeroSpace, Agent Screen tells AeroSpace which displays and windows are the agents'. AeroSpace no longer moves an agent's window into your workspace when the window's app comes to the front by itself, as a VPN app does to show its login window. It also leaves alone the windows macOS puts back on a display after Agent Screen restarts.
- If Agent Screen quits while an agent works, the agent's next call says the app quit instead of "does not exist". After create_display, the windows of apps it launched on the old display are listed as strays, and place_window brings them back. A crash leaves its message and backtrace in displays.log.
- Launching an app no longer stalls the menu bar icon, Peek and the Viewer for a quarter second or more: Agent Screen watches for the app's new windows on a thread of its own, since registering that watch waits on the app.
- Adding or removing a display stops every window on the Mac from drawing for a moment, so Agent Screen now does it while you are away. Displays you end are released once you have been idle for a minute, and while you use Agent Screen a spare display waits ready, made while you are idle, so the next + shows a display without that stop.
- The setup window opens centered on the screen, after it has grown to fit its content. While it is open, Agent Screen shows in the Dock and the app switcher, so you can find the window behind others, and its menu has Set Up, Hide and Quit. Once it closes, Agent Screen is a menu bar app again: a regular app also takes part in each display's menu bar, which keeps it busier on every display change when many displays are open.
- An agent's click answers in about 75 ms instead of about 450 ms. Agent Screen read the full window list twice per click, about 1,900 windows each time; it now reads it once, with CoreFoundation directly, and skips windows that can't be the target.
- launch_app answers about 0.8 s sooner, 1.3 to 1.4 s instead of about 2.2 s for Calculator started cold. The click that makes a new window key goes ahead of the agent's next input instead of holding up the answer, and a placed window is checked once before the answer, with the watch that follows for 3 s catching a later move.
- A new display is ready about a second sooner. Agent Screen parks it right after macOS makes it, without waiting for the app's main thread, which every new display stalls for a while, and parks it for the session only: made permanent, macOS took 2.3 s instead of 1.3 s and still showed the next display at its default spot first.
- Screenshots answer in about half the time, 26 ms instead of 50 ms in the median. Agent Screen asked ScreenCaptureKit to list every window before each one, about 20 ms. It now asks only when a window has come or gone since, which the window server tells in under a millisecond, and reads where windows are now from the window server.
- Agent Screen's windows keep responding while a display is added or removed. Each window now draws into a texture of its own, and a separate thread hands finished frames to macOS. Before, waiting on macOS for a frame could freeze the Viewer and Peek for up to a second.
- The menu bar icon is a little heavier and sits in the middle of its space, and it stays where you put it in the menu bar after an update instead of being pushed out of sight.
- An app you open with Spotlight, Raycast or Alfred while you work in a Viewer opens its window on that Viewer's display, as a window you handed over. After a click on your own screen, it opens there as before.
- The menu bar icon is new: two screens and a pointer, as the app icon, so it no longer looks like macOS's own screen icons. Its front screen fills while a display is in use, a raised hand takes the pointer's place while an agent asks for help, and a slash crosses it while a permission is missing.
- Your monitors stay where you arranged them when Agent Screen adds a display. macOS adds a display beside the main one and moves a monitor already there out of its way, then remembers that arrangement. Agent Screen now puts such a monitor back in the same change that parks the new display, for displays you add from the menu bar too.
- Peek, the Viewer and the Help Card no longer show an empty, see-through strip when they resize while a display is added or removed. They now draw at their new size in the same step that resizes them.
- Peek no longer shows an older picture of itself over the current one after several displays come in a row.
- One click in the Viewer on a text field of an app that brought itself to the front before, such as aTrust's login window, puts the caret in the field. It used to take two clicks, or the clicks were dropped.
- In Peek, + shows a tile for the new display at once and grows it in, and a closed display's tile shrinks out, while the card follows smoothly at the screen's refresh rate. Peek, the Viewer and the Help Card animate at the screen's full rate instead of 30 fps.
- A display you make with + gets the Agent Display wallpaper, as one an agent makes does. A window that resizes while displays change shows its new frame at once, without the UI waiting for macOS.
- Closing an Agent Display whose app asks whether to save no longer drops that question on your screen. The display stays and the Viewer opens on it, so you answer there; the display goes once the window has closed. After Cancel, the window and the display stay for you to review.
- The screenshot after an action no longer shows the display too early when the agent works quickly. Agent Screen now counts the display's changes from the action on, and waits half a second from the action, then until the display has been still for 120 ms. Before, an app that drew its result a moment after a first quick change, such as a page that updates a counter at once and redraws 400 ms later, could be caught before the redraw.
- A kept spare display now takes any size an agent asks for, with one mode change, instead of Agent Screen adding another display. Fewer displays stay online, so each later display change stalls apps for less time. In a three-round test with a new size each round, display changes went from 9 to 4.
- The displays log names where each main-thread stall of 150 ms or more went: while the main thread is late, Agent Screen reads its stack every 10 ms, and the full counts go to a `hang` file beside the log.
- Two spare displays wait ready instead of one, so two + in a row show displays without the Mac stopping to draw, and up to three ended displays stay as spares.
- Agent Screen's menu bar icon, Peek and Viewer no longer freeze for up to a second and a half while the setup window is open. Checking permissions, following System Settings, counting windows, guarding the cursor and the window lookups behind agents' requests now wait on WindowServer off the UI thread.
- Clicking the Viewer while another window had the keyboard could quit Agent Screen. It doesn't anymore.
- An agent can ask for one of your windows, such as one of an app it opened with a script instead of launch_app. A card drops from the menu bar icon and a notification names the window: Hand Over gives it to the agent as a Handover, and Keep leaves it with you. Before, the agent could only ask you in words to drag the window over, and some agents quit the app to start it again.

#### Browsers an agent starts no longer flash on your screen

Agent Screen starts an app hidden so its first window opens out of sight, but Chrome, Firefox and Brave show themselves as they start, so their first window appeared on your screen for up to half a second. Agent Screen now hides such an app again until its window sits on the agent's display.

#### Agent Screen asks AeroSpace far less often

Every command makes AeroSpace refresh all of your windows first, which takes up to two seconds on a busy Mac and stalls AeroSpace meanwhile. Agent Screen asked it on every display change, up to 25 times a minute while agents worked. It now asks once, when the first agent display appears, and not at all when no agent display is online.

#### A new display no longer makes every other display move

When a monitor you unplugged left a gap in macOS's display numbers, every new agent display came out with a number another display had, so Agent Screen made it twice and removed the first. Each of those changes moved all the other displays and could drop their windows on your screen. Agent Screen now keeps that first display, out of the way, until its last display goes, so each new display is one change.

#### launch_app follows an app to its document window

Pages, given a text file, closes the window it opened first and shows the document in another. launch_app kept trying to place the closed one and failed. It now places the app's new window instead.

#### Agent Screen logs when it stops responding

When Agent Screen's main thread, which draws the menu bar icon, Peek and the Viewer, doesn't answer for 150 ms or more, `~/Library/Logs/Agent Screen/displays.log` says so beside the display changes. A stall still going at 300 ms also saves a one-second sample of Agent Screen to that folder, at most once every ten minutes, keeping the last five.

#### A new agent display is one display change instead of two

Agent Screen moved every new display out of the way after macOS added it, and each move is another change every app on the Mac handles. macOS now remembers where each display goes, so the next display with the same slot appears there at once.

#### Peek and the Viewer keep responding while agents make displays and start apps

Parking a new display waited on macOS for 100 ms or more on Agent Screen's main thread, and finding a starting app asked macOS about every running app 50 times a second. Both stalled the menu bar icon, Peek and the Viewer. Display changes now run on a thread of their own, one at a time, and Agent Screen finds a starting app in the process table.

#### Agents' windows stay on their displays while other agents add or end displays

Each display that comes or goes makes macOS move the others, and it could drop their windows on your screen, where AeroSpace tiled them among yours. Agent Screen puts such a window back as soon as the displays settle, and no longer mistakes a display that is moving for its window leaving.

## 0.1.14 (2026-10-16)

### Features

#### Smooth Display Changes: a helper keeps macOS's display records short

The setup window has a new optional step, Smooth Display Changes. It asks for your administrator password once and installs a small tool that Sidescreen runs as root through `sudo`. It runs after a display is created or ended, and every 30 minutes. It removes macOS's color records of displays that aren't connected, and keeps display names in English and your languages. macOS reads every record, once per display online, on each display change, so with many records the whole Mac stuttered. Profiles you chose stay. `Sidescreen colorsync remove` takes the tool away. `Sidescreen colorsync reclaim` runs it at once.

When the reads would take longer than macOS waits for them, `create_display` now says so and asks the agent to use or end a display it has, instead of slowing the Mac down.

#### Sidescreen changes the Mac's displays far less often

Every display Sidescreen added or removed made each app on the Mac reconfigure its screens, and with a virtual display connected each change left macOS doing more color work for minutes. After many agent displays in a short while, the whole Mac could slow down or freeze. Sidescreen now keeps an emptied display for a few minutes and gives it to the next agent that needs one of the same size, adds each new display with one change, and lets agents make at most ten new displays in ten minutes. `~/Library/Logs/Sidescreen/displays.log` records every display it adds, keeps, reuses and removes.

#### A Help Request can list steps

`request_help` takes up to five `steps` beside its one-line reason. While you are in control, the Viewer lists them under its header, so you can follow them as you work on the display.

#### Agents can type through your input method

`computer` `type` and `key` take `via_input_method`, so an agent can type pinyin or another input method's keys and see the composition, as when testing an input-method bug. For the call, the agent's app gets keyboard focus and the agent uses your keyboard; the Viewer and Peek say so. Sidescreen waits until you've left the keyboard and pointer alone for 2 seconds, stops when you touch either, and sends nothing while a password field or a system prompt has the keyboard. See ADR 0008.

#### launch_app starts development builds

`launch_app` takes the path of an `.app` bundle, and `arguments` and `environment` for a new instance of the app, so an agent can start a development build without a wrapper bundle. An app that makes itself an accessory app after it starts, as Electron builds that open windows in the background do, now shows its windows on the display, and `list_windows` lists them. An app that takes more than a few seconds to open its first window now gets that window on the display too.

#### Agents can record their display

`record_display` records an Agent Display to an MP4 in ~/Movies/Sidescreen, at the display's full size, with a timeline file beside it: one line per agent action and Takeover, each with its seconds into the video. A recording stops when the agent stops it, when the display ends, or after 30 minutes. The Viewer header and Peek show a red Recording mark while it runs.

#### Recordings have sound, and agent displays can wear your wallpaper

A recording made with `record_display` now has the sound of the apps on the display. Only apps whose every window is on that display are heard: macOS separates sound by app, not by window, so an app you also use elsewhere, such as your browser, stays silent, and `record_display` stop lists which apps were heard and which were left out. In a test, a tone from an app on the display came through at -7.5 dB, and one from a process elsewhere at -100 dB.

Agent Display Wallpaper… in the menu sets the picture agent displays show in the Viewer and in recordings; Use Sidescreen's Wallpaper puts the default back. Agents' screenshots leave the wallpaper out either way.

### Fixes

#### A new window stays on its agent display under AeroSpace

AeroSpace sometimes moved a window back into your workspace just after Sidescreen placed it on an agent display, so a browser an agent opened could end up tiled on your screen. For 3 s after placing a window, Sidescreen puts it back on the display if it leaves.

#### The Agent Pointer is a rounded arrow that arcs to each point

The Agent Pointer in the Viewer and Peek is now a drawn arrow in your accent color, with a soft shadow and a glow, in the style of Codex's computer-use cursor. It travels to each point on a curve in 0.09 to 0.22 s, fast off the mark and soft on landing, and catches up when the agent clicks again before it lands, so it no longer trails the clicks. It turns a little with its speed and direction, as if its tail trailed behind, and settles with a small wobble. On a click it presses in and sends out a ripple. For a few seconds after each action it sways slightly while the agent works out its next step.

#### Agents' clicks land when several apps run at once

When another app came to the front, even for a moment as when a second agent started an app, Chrome on an agent display ignored every click in its page, and an app such as Numbers dropped the first click on a window it had just swapped in. A window of an app in the background takes clicks only after one click makes it key. Sidescreen now spends that click itself, on an empty spot of the window's title bar, whenever a window hasn't had one since the front app last changed. It never does this in your front app.

The same goes for an app you and an agent both use: after you open a new TextEdit document, the agent's typing goes to its own window again instead of yours.

#### Close Display is its own button in the Viewer

The Viewer's ⋯ button held only Close Display…, so it is now an × button beside Collapse. Give and Take Back highlight in the capsule's shape on hover.

#### Clicks no longer crash Electron and Chrome pages, and drags start on the first move

Sidescreen posted input without a time. A click that changed a page's URL, as in most single-page apps, made Chromium kill the page. Every event now carries the time it is sent. A press just outside a resizable window's edge now resizes the window, and the first `mouse_move` after `left_mouse_down` now moves the drag. A screenshot an action returns now waits for what the app draws a moment after the action, also when the display was idle before.

#### Typing through the input method works call after call

Several problems with `type` and `key` with `via_input_method` are fixed:

- After a call, an app that ignores the Accessibility request to come to the front, such as an Electron app, stayed behind the agent's app. Switching your input source back then committed the agent's composition as plain letters. Sidescreen now waits until your app is in front, activates it another way when it doesn't come, and only then switches your input source back.
- A second call sent its keys nowhere: the agent's window had lost key status while your app was in front. Each call now makes the window key with a click on its title bar first.
- A call right after another was refused as if you were typing, because the keys Sidescreen had just posted reset the idle timer. Sidescreen now tells its own events apart from yours.
- The agent's input source takes over before the first key, so the first letter is composed too.
- Sidescreen could crash when an input source changed during a call. It now makes input source calls on the main thread.

The tool description now says to compose and commit in one call, such as "nihao " for 你好: what the input method holds is dropped when your app comes back to the front.

#### Releases are notarized by Apple

The downloaded app opens like any other: no more Open Anyway in System Settings or `xattr` to run first.

#### Peek closes once the pointer leaves it

Peek opened by hovering the menu bar icon used to stay up after it had once been closed with Escape while the pointer was on it.

#### Many agent displays no longer leave macOS re-reading color profiles

Every display change makes macOS read its color records of all displays ever connected, once per display online, and retry every 5 seconds until a read finishes within 50 ms. Sidescreen used to leave a new record for each display it made, so after many displays the reads stopped finishing in time, the retries piled up, and apps that ask about displays stuttered. New displays now reuse the numbers of displays that are gone. Smooth Display Changes, in the setup window, removes the records earlier versions left and keeps each record's names in English and your languages, after which 13 displays online read in 19 ms. A new agent display also gets its own color profile right away, where it used to keep sRGB.

#### System prompts stay on your screen

When macOS puts one of its own prompts on an Agent Display, such as a keychain password request or an Apple Account alert, Sidescreen moves it to the middle of your main display within a second. The agent can't see or answer it there.

#### Dragging a window in the Viewer keeps it on the agent's display

Dragging a window by its title bar in the Viewer moved it to your own screen, where your window manager could tile it. The window now moves on the agent's display, as far as you drag, and stays inside it. This also works for title bars drawn by SwiftUI, such as Calculator's. When an app draws its own title bar, as Electron apps do, a window that a drag carries off the display goes back onto it when you let go.

#### The Viewer, Peek and agent screenshots never show another display

macOS sometimes gives a new virtual display the same internal number as another display, such as the one AeroSpace keeps hidden windows on. Every picture of the new display then showed the other one. Sidescreen now makes the display again until its number is free. If two displays share a number later, Sidescreen shows no picture of them and says why.

#### Screenshots of large displays show the whole display

A `computer` screenshot of a display wider than the image limit used to show only its top-left part, while clicks and `list_windows` used the scaled-down whole. It now shows the whole display scaled down. A zoom over the full screenshot works at the size the screenshot reports, and a zoom whose capture fails for a moment, as after a window was resized, tries again and then cuts the region from a whole capture.

## 0.1.13 (2026-10-13)

### Features

#### A Help Card, more notifications, and Give | Take Back in the Viewer

- An agent asking for help now drops a card from the menu bar icon: the display's picture, the agent's reason, Take Over and Later. Later puts it away until the agent asks again; the icon stays orange.
- The help notification has a Take Over button. Clicking the notification itself only opens the display; your first input there takes over.
- Sidescreen also notifies you when an agent finishes or disconnects and leaves windows on its display.
- The setup window asks for notifications. If you set up before, Sidescreen asks once at its next launch.
- The Viewer's Windows button is now Give | Take Back, each opening its list of windows directly, with Close Display… under ⋯.

### Fixes

#### Sidescreen ignores AeroSpace's closet display

The AeroSpace build for Sidescreen keeps hidden windows on a virtual display of its own, the closet. Sidescreen now leaves that display out: its panels and the Viewer never go there, a window handed back never lands there, and agent displays park as if it weren't there.

#### Agent Displays are labelled in System Settings

Each Agent Display gets a desktop picture that says "Sidescreen", and System Settings > Displays shows each display with a thumbnail of its desktop, so agents' displays are told apart from your monitors there. Agents don't see it: their screenshots show windows on a plain backdrop.

#### Browsers take the first click, open URLs in the new window, and choose select items

- `launch_app` with a URL now returns once the page has loaded, so a click right after it reaches a page whose scripts have run. An app started by `launch_app` gets a click on its top edge first, because its first click only makes its window key; the agent's first click now lands.
- A click on an item of a web page's select menu chooses it. The menu follows the real pointer, so the click now presses the menu item through Accessibility.
- `launch_app` starts Safari. It looked for Safari at its symlinked path and reported that it did not start.
- A URL opened in a running Safari goes to the new window on the display, through its address field. Before, Safari opened it in the window it had last made active.
- `get_window_state` reads web pages in Chrome. Chrome ignores the switch Dia and Electron apps answer, and builds its page tree only for the one VoiceOver uses; Sidescreen sets that one, and turns it off while it moves a Chrome window, which it slows.

#### A development build no longer interferes with the installed app

A debug build of Sidescreen is now its own app, "Sidescreen Dev", with its own bundle id, privacy grants, control socket and agent registration (`sidescreen-dev`), and a purple menu bar icon. Before, it shared the installed app's bundle id, which could cost the installed app its Screen Recording and Accessibility grants, and Connect in a dev build replaced the installed app's registration with Claude Code and Codex.

#### New Display is made off the main thread

Displays you make with **New Display** are made off the main thread too. If an agent's session ends, or a parallel call of the same session gets a display, while macOS is still making one, that one is dropped instead of left running.

#### Agents can use file Open and Save panels

An app's Open or Save panel is drawn by a separate macOS process, behind an empty window of the app's, so an agent's clicks and keys never reached it and the panel looked frozen. Clicks and keys now go to the process that draws the panel, and the first keys wake it the way a click would, so an agent can pick a file with Go to Folder (cmd+shift+g) and a path. `get_window_state` also reads a panel that a browser such as Dia or Chrome shows as a sheet.

#### Firefox's file panel, fresher screenshots, web pages in window state

- Firefox shows its file panel at the modal panel level; clicks and keys now reach it too.
- After an action, the screenshot waits up to 500 ms for a first change, so it shows a panel or sheet the action opened instead of the moment before.
- The first `get_window_state` of a Chromium or Electron window waits for the page's accessibility tree, instead of listing only the browser's toolbar.

#### Agent Displays go back out of the way when moved

An Agent Display dragged in System Settings > Displays, or pushed aside by macOS, goes back to its place off the top-right corner of your screens after the change, where the pointer doesn't wander onto it. Your own displays aren't moved. A display macOS keeps putting elsewhere gets three tries, so Sidescreen doesn't fight it.

#### Keys stay on the agent's display

When the window an agent was typing into leaves its display, as when you take it back, the agent's next keys go nowhere and it's told so; they used to reach that app's window on your screen. Resizing a window from its left or top edge over the Viewer no longer hands it over. A Help Request banner that arrives after the request was withdrawn only opens the display. A window returns to the main display when the monitor it came from is gone. `place_window` can be retried after a failed move, and a busy app no longer reads as a closed window.

#### The setup window shows all of itself

After you clicked Turn On, the setup window added a line explaining what to do in System Settings and cut off its last row. The window now grows to fit what it shows, and is a little wider, so "Screen & System Audio Recording" fits on one line.

#### Agent keys never reach the app you're using

When the agent's window belongs to the app you're working in and isn't that app's focused window, typing, key presses and held keys stop with a message telling the agent to set text with `act_on_element` instead: macOS would send those keys to your window. Long typing checks again every 20 characters, and Sidescreen re-focuses the agent's window when the app has moved its keys to another of its windows, then hands focus back to your window when the agent's keys are done, so your typing in that app still goes to your window. Select All only selects in the agent's window. On displays larger than 1280×800, window bounds and element positions now come in screenshot pixels, so the agent clicks where it means to. Dragging a tiled window onto the Viewer hands it over again. A takeover lets go of a button the agent held where its drag was, not where you clicked, and a held key is released in the window it was pressed in. Keys typed in the Viewer no longer follow a window you took back.

#### The Viewer is sharp again

Agent displays could come up at 1x and look blurry in the Viewer: macOS remembers the resolution last used for a display's serial number, and Sidescreen's serials repeat across launches, so a display could inherit another size from an earlier one. Sidescreen now sets each display to its size at 2x as it makes it. The Viewer also no longer keeps showing a small preview frame, scaled up, when the display it opens on is still.

#### Your app's new windows open on your screen

After you take back a window, the app is yours. Some apps, TextEdit among them, open a new document next to their main window, which could still be on the agent's display, out of your sight. Sidescreen now moves such a window back to the screen the window you took came from. It stays only if the agent was acting there at that moment and you weren't using the app. Handing the app over again ends this.

## 0.1.12 (2026-10-12)

### Fixes

#### Peek and the Viewer keep drawing while a display is made

macOS takes about half a second to make a virtual display. Sidescreen now does that off its main thread, so Peek and the Viewer keep drawing, and parks the display right after. `create_display` measured 771 ms median at load 27, down from 884 to 913 ms. `launch_app` notices a running app's new window up to 25 ms sooner. When another agent or you started an app moments ago, `launch_app` waits up to 3 seconds for its menus and then says it just started, instead of telling the agent to ask you to quit it.

#### The right place to hand over a window

Agents now tell you to hand a window over from **Windows** in the Viewer's header, where the command is; they used to point at the menu bar icon's menu, which has no Windows item. Chinese copy is more consistent: 查看窗口…, 放回原位置, 需要你帮助, and the AeroSpace hint names Agent displays. A slow or busy app no longer stalls Sidescreen while a launch checks which file its window shows.

#### Taking over and asking for help

While you answer a help request in the Viewer, its reason stays in the header. When you take over, the agent's typing stops at the next character and its drag lets go where the pointer is, short of the drop; the agent is told how far it got. A finger resting on the trackpad over the Viewer no longer takes the display over; a scroll that moves does. An agent whose help request goes unanswered hears that the request is still open, and can take it back with `withdraw: true`. If you close the display while the agent waits, it is told you closed it. A new window of an app that another agent is also using on its own display, opened while both were acting, stays where it opened. Closing a display now also closes or returns windows the agent opened while you were deciding.

`hold_key` lets go as soon as you take over, so your typing doesn't come out with the agent's Shift held. A drag stopped right after it starts lets go a step away from what it grabbed, which usually keeps it from acting as a click. Text with Windows line endings types one Return per line.

A key sequence such as Down × 40 also stops when you take over, and the agent hears how many keys went in. `launch_app` on an app that has just started and has no window yet says it is still starting, not that it has no New Window command.

## 0.1.11 (2026-10-12)

### Fixes

#### The menu names Claude Code and Codex by name

The menu bar line says "Claude Code loaded Sidescreen's tools" instead of the raw client id. When Sidescreen starts after the agent, or restarts after an update, the line comes back within 2 seconds.

#### Connect your agent from the setup window

The setup window has a row for Claude Code and Codex with a **Connect…** button, and shows which agent loaded the tools. Connect now finds a `claude` installed with npm, nvm, volta or fnm, because it uses the PATH your terminal has; if it still fails, the alert gives the command to run in Terminal. Peek with no displays says agents create their own, and that the menu opens on right-click. "Waiting for picture…" is easier to read in light mode. The Chinese name of the Screen Recording permission matches System Settings: 录屏与系统录音.

#### Setup picks up where it left off

After you press **Reopen Sidescreen**, the setup window opens again at the Connect step. The Connect row says "Connecting…" while it works, ignores a second press, and once Claude Code or Codex has Sidescreen it says so and offers **Connect Again…**. If neither client is installed, Connect gives the command and argument for any other MCP client. Connect's results are in your language. If the Screen Recording grant starts working without a reopen, agents stop being told to ask for one. Closing the setup window is no longer undone by the agent's next call.

"Added to" now counts only an entry that runs this copy of Sidescreen, so a registration left by a moved or older copy shows **Connect…** again. While the setup window asks you to reopen, it checks every 2 seconds whether the grant took effect anyway. The Chinese setup window calls displays 显示器, like the rest of the app.

#### Closing a display stops the agent

When you close a display the agent is using, the agent is told you closed it and to ask before making another, instead of being told to create a new one. When Screen Recording needs Sidescreen to reopen, the menu offers **Finish Setup: Reopen Sidescreen…**, and agents are told to ask you to reopen it. `wait` reports the time it waited, at most 30 seconds.

#### Your clicks and keys keep your windows yours

A new window of an app the agent started now goes to the agent's display only if the agent acted there in the last 3 seconds and you didn't click or type meanwhile in that app, Finder, the Dock or a launcher such as Spotlight. A window you open yourself stays on your screen. A window that opens while you both act, or with nobody acting, stays where it opened; the agent's next screenshot or window state tells it, and it can place the window with `place_window`. For an app the agent cold-starts, this holds for 30 seconds from the launch. Raycast and Alfred count like Spotlight. Clicks and keys you send through a display's viewer count as driving that display. When a session disconnects, its display stops taking new windows.

## 0.1.10 (2026-10-12)

### Features

#### See when an agent has loaded Sidescreen

The menu bar menu and the setup window name the agent once its session has loaded Sidescreen's tools, so you can tell setup worked before asking the agent for anything. Until one has, it says to start a new session after connecting.

### Fixes

#### Which windows of an agent's app are the agent's

A new window of an app the agent started goes to the agent's display only when it appears within 3 seconds of the agent acting on that display. Bringing the app to the front no longer decides it, so the agent's own late window can't get stuck on your screen. After you take back a window of the app, `place_window` says the app is yours now. A `file://` link with `#` or `?` opens its file in apps other than browsers, and a missing file is reported before anything opens.

#### A late file window is caught as it names its document

When an app you run opens a file after Sidescreen's 4-second turn, Sidescreen watches the new window until it shows the file and then moves it to the agent's display.

## 0.1.9 (2026-10-12)

### Fixes

#### Windows you make in an agent's app stay yours

A window you open in an app the agent started, while that app is in front or after you took one of its windows back, is never listed by `list_windows` with `strays: true` and never moved onto the agent's display later. A `file://` link opened in a browser gets a window of its own on the agent's display again, and `file://` links with `#` or `?` work.

## 0.1.8 (2026-10-12)

### Fixes

#### A slow file window moves as it opens

When an app you are running takes a while to open a file, Sidescreen now moves the file's window onto the agent's display the moment it appears, instead of after it showed on your screen. `launch_app` says `existing_window` only when a window the app already had shows the file; otherwise `still_opening`. `file://` URLs open like their paths.

#### An agent's app stops taking your windows when you use it

A window you open in an app the agent started stays on your screen while that app is in front, and after you take back one of its windows. `place_window` on a window you took back says so. `list_windows` marks further windows of the agent's apps as `launched`.

## 0.1.7 (2026-10-12)

### Fixes

#### create_display asks AeroSpace less

Once AeroSpace has left an Agent Display alone, Sidescreen trusts that until AeroSpace restarts or its config file changes, instead of asking AeroSpace again on each new display.

#### A file opened in a running app finds its way to the agent's display

When an agent opens a file in an app you are running, Sidescreen recognizes the file's window by the document it shows. If the app takes longer than 4 seconds, the window still moves to the agent's display when it opens, for up to 30 seconds, and `still_opening` says so. Your other windows in that app stay where they are.

#### New Window in other languages

`launch_app` finds New Window and New in German, French, Spanish, Italian, Portuguese, Dutch, Swedish, Russian, Japanese, Korean and Chinese menus. When an app's menus can't be read, it answers `menus_unreadable` instead of `no_new_window`.

#### A window you take back stays yours

A window you take back from an agent is never listed by `list_windows` with `strays: true` again, and `place_window` refuses it, even in an app the agent started. Once an agent ends its display, Sidescreen stops moving that app's new windows onto it.

## 0.1.6 (2026-10-12)

### Fixes

#### A running app gets asked for a window, not sent Command-N

For an app you are running, `launch_app` now looks in its menus for New Window, or a document app's New, and uses that item. In Notes, Music or Calendar, where Command-N makes a note, playlist or event, it creates nothing and answers `no_new_window`. A greyed-out item answers `app_busy`. A URL that can't get its own window fails instead of opening in your window.

#### Windows of an app the agent started stay on its display

A later window of an app `launch_app` started, such as the extra window TextEdit opens when two sessions start it at once, now goes to the agent's display instead of your screen. `list_windows` with `strays: true` lists such windows that still landed elsewhere, and `place_window` takes only those.

## 0.1.5 (2026-10-11)

### Fixes

#### launch_app answers at once for a single-window app

For a running app whose menus have no New Window command, such as Calculator, `launch_app` answers `no_new_window` right away instead of after 3 seconds. Its answers now include `wasRunning`.

#### place_window keeps to an agent's own windows

`place_window` refuses a window on another session's display, and its description says how an agent gets one of your windows: you hand it over.

#### Sidescreen takes a running app's windows for at most 4 seconds

When an agent opens a file in an app you are running, Sidescreen stops taking that app's windows 4 seconds after the request at most, even if the app is still opening the file; `launch_app` then answers with `still_opening`. Before, an app slow to open a file kept that window open for 6 seconds.

## 0.1.4 (2026-10-11)

### Fixes

#### AeroSpace settings count once AeroSpace has loaded them

Sidescreen treats AeroSpace as leaving its displays alone only when AeroSpace has loaded `ignored-monitors`, not as soon as the config file says so.

#### launch_app says why there is no window

When `launch_app` returns no window it now gives a `reason`: `still_starting`, `existing_window` (an app you were running opened the file in a window it had) or `no_new_window` (an app you were running made none, such as Calculator), with the next step for each. For an app you were running, Sidescreen stops taking its windows 3 seconds after asking it for a new window, or 2 seconds after it opened the file; a window you open in that app after that stays on your screen. An app that is slow to open a file could still hand Sidescreen a window you opened in it up to 6 seconds after the request.

## 0.1.3 (2026-10-11)

### Fixes

#### AeroSpace config edits keep dotfile links and comments

Adding `ignored-monitors` from the menu writes to the file a symlinked config points to, so dotfile managers keep tracking it, and reads lists with comments in them. Sidescreen also notices when AeroSpace is started, quit or reconfigured while it runs.

#### launch_app reports a failed open

An app or file that macOS can't open fails `launch_app` with "could not open" within about a second, and a URL whose `open` fails after the new window appeared is an error instead of a success.

#### Windows you open stay on your screen while an agent launches the same app

When an agent launched an app you were also using, a window you opened in that app within the next half minute could move onto the agent's display. Now Sidescreen moves only the window it asked the app for.

## 0.1.2 (2026-10-11)

Agents' windows stay off your screen, and launch_app is faster.

- A launched app's new window moves onto the agent's display the moment macOS creates it. An app that wasn't running starts hidden and shows once its window is there.
- When an agent opens a window in an app you also use, the window it asked for moves onto its display the moment macOS creates it.
- `launch_app` for an app that is already running: 612 ms median, was 1244. `create_display`: 772 ms median. Measured with AeroSpace set to ignore agent displays.

If you use AeroSpace: Sidescreen puts any workspace a new display takes back on the monitor it came from, and the menu offers to set AeroSpace's `ignored-monitors` so it leaves agent displays alone. See [docs/aerospace.md](https://github.com/Fanzzzd/sidescreen/blob/main/docs/aerospace.md).

Install or update: `brew upgrade --cask sidescreen`, or download the DMG below. Not notarized: the first time, allow it under System Settings → Privacy & Security, or run `xattr -dr com.apple.quarantine /Applications/Sidescreen.app`.
