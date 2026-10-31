# Chromium apps launched with a private DevTools pipe

Chromium and WebKit take a mouse event's `buttons` from `[NSEvent pressedMouseButtons]`, which reads WindowServer's button state for the whole login session. Events Agent Screen posts to one process (ADR 0001) never set that state. So every move of an agent's drag reached a web page with no button held. Splitters, sliders and sortable lists took the moves for hover, and Chromium ended an HTML5 drag as soon as it started (#37). The only way to set the state is to press the session's button, which is the user's button. Closed PR #89 held it for up to 0.6 s per drag, and we rejected that: any chance of a conflict with the user is too much.

`launch_app` starts a Chromium or Electron app itself, with `--remote-debugging-pipe`, in place of `open`. Chromium then reads DevTools commands from fd 3 and writes answers to fd 4. Only Agent Screen holds the other ends. No TCP port opens, so no other process can connect. A drag that starts in a web page of such an app goes through the pipe: `Input.dispatchMouseEvent` with the left button in `buttons`, and `Input.setInterceptDrags` with `Input.dispatchDragEvent` for an HTML5 drag. The browser makes the events itself. Nothing reaches WindowServer, so the session's button state and the user's pointer stay theirs.

## Which apps get a pipe

- An app is Chromium when a framework in its bundle ships `chrome_100_percent.pak`. That covers Chrome, Edge, Brave, Arc, CEF apps and every Electron app.
- Only an app that starts here gets one. A running app opens a new window through its menus, as before, and gets none. For a browser, the agent starts a new instance with its own `--user-data-dir`, since Chrome ignores the switch on its default profile. A fresh profile has separate sign-ins. It does not preserve the user's current login or add a pipe to a window already running. When that login session is required, the agent keeps the window and asks the user to do the drag through `request_help`.
- An app that strips the switch, such as Claude.app or Figma, never answers on the pipe. After 30 s without an answer, Agent Screen closes its end and the app counts as one without a pipe.
- A non-browser app given a file or URL with `open` starts through `open` without a pipe, since only a browser takes a target as an argument.

## How the app starts

`posix_spawn` with `--remote-debugging-pipe` before the agent's arguments, and otherwise what `open -n` gives an app: its own session, the working directory `/`, `/dev/null` on stdin and stdout, and no other descriptor of Agent Screen's. Its TCC responsibility is its own (`responsibility_spawnattrs_setdisclaim`), so its permission prompts name it and it gets none of Agent Screen's grants. The environment is Agent Screen's minus what marks a process as Agent Screen (`__CFBundleIdentifier`, `XPC_SERVICE_NAME`, `AGENTSCREEN_*`), plus the agent's. The pid is known at once, so the pid choice from #35 isn't needed. The single-instance handling from #29 still applies. `open -j` started an app hidden. Agent Screen hides a spawned app as soon as it can, then shows it once its windows are on the display. The pipe adds no infobar or other UI, and the app looks and behaves as it would after `open`. Measured in `webdrag` on one boot, both ways. The Electron fixture held the front for 214 to 240 ms with the pipe and 213 to 229 ms through `open`, and its window showed on the main screen for a moment both ways, before Agent Screen gave the front back (#67) and moved the window. Chrome on a throwaway profile took the front for 220 ms and showed its window on the main screen through `open`. Started with the pipe, it did neither.

When Agent Screen quits, the pipe closes and the app keeps running. An app adopted after a restart has no pipe.

## Where a drag goes

For `left_click_drag` and `left_mouse_down`, Agent Screen finds the window under the start point, then the outermost `AXWebArea` there. That web area's frame is the page's viewport. Every web area Chromium and WebKit publish has this role.

- **In a web area, with a pipe.** The drag goes through the pipe. The page is the visible page target whose `screenX`, `screenY`, `outerWidth` and `outerHeight` match the window. When several do, as in an Electron window with more than one web view, the one whose viewport matches the web area wins. A point maps to CSS pixels by the web area's origin and its width over `innerWidth`, which covers page zoom and `devicePixelRatio`. Without accessibility's tree, the viewport fills the window's width at its bottom, as in Chrome and Electron. The moves, the release and a takeover's release go to the same page.
- **In a Chromium or WebKit web area, without a pipe.** A complete drag first tries a supported accessibility slider value change. Chromium reports a written value as accepted and ignores it, so its slider moves by AXIncrement or AXDecrement, one confirmed step at a time. Otherwise the action creates an approval intent in the shared Input Queue (ADR 0013), then returns `input_pending`. A `left_mouse_down` there also queues an intent without sending a press. There is no half-click and no subsequent move to continue. A complete drag waits for approval, then can run with the user's pointer. A split press only asks; it cannot hold the user's button while the agent reasons between calls. `input_queue` can inspect, wait for or withdraw the request; `request_help` remains available when the user needs to perform the drag.
- **Anywhere else.** Title bars use window placement. A complete drag on a slider with sufficient accessibility information sets and confirms its value; ambiguous controls are left to the other routes. Other native controls, window edges, toolbars, tab strips and Firefox's pages use posted events as before. Firefox's page drag behavior was not tested.

A text-selection drag in a page with no supported background route also queues an unexecuted intent. The error offers click and shift-click as a background alternative.

## Consequences

- General page drags work through the private pipe of Chromium apps `launch_app` started. Some sliders can use accessibility instead. For other page drags in WebKit, a default browser profile, user-started apps or apps that strip the switch, queue approval does not add a working execution route in this build.
- A drag through the pipe can't leave the browser. An HTML5 drag drops inside the app's own pages, never into another app.
- `Input.insertText` could type whole strings, CJK included, but it sends no key events, and pages that act on keys would behave differently. Typing keeps the posted keys.
- Agent Screen now parents and reaps these apps. `open` gave them launchd as parent.
