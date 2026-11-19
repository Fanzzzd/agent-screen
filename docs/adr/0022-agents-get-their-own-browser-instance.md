# Agents get their own browser instance

Agents' browser windows used to open in the user's own browser process. When the user worked in that browser, it was their front app with their window focused. Agent Screen then posted nothing to the process: posted keys would reach the user's window, and a posted click would make the agent's window key and take the user's typing (#110). Clicks went in as accessibility presses only, and Dia's web elements take none. So while the user watched a video in their Dia, every agent's Dia window stopped (#222).

When `launch_app` targets a Chromium browser, Agent Screen runs a separate process of that browser for agents. The process has its own `--user-data-dir`, and Agent Screen starts it with the private DevTools pipe (ADR 0010). It is never the user's front app, so posted clicks and keys reach it while the user works in their own process, and web drags and selects work through the pipe.

Status: proposed.

## The rule

- A Chromium browser is an app that handles web URLs and ships `chrome_100_percent.pak` (ADR 0010).
- If the agent names a profile with `--user-data-dir`, that wins and nothing below applies.
- If the agents' instance runs with Agent Screen's pipe, the new page opens in it through the pipe: `Target.createTarget` with `newWindow`, `background` and the display's frame. Only the window that shows that page answers the launch. Agent Screen reads the page's window frame from Chromium (`Browser.getWindowForTarget`) and takes the system window with that frame, so a window another session's page opens meanwhile is never answered to this agent. A browser that puts the page in a window it had open, as a tab, answers with that window when it is on the launch's display. When it is on another display, the tab is closed and the launch fails. Dia keeps its pages in its own windows, which Chromium gives no frame: its answer to `newWindow` is a window of no size that never shows. Agent Screen closes that one and loads the page in the visible page of the agents' Dia window on the launch's display, which it finds by asking each page through the pipe whether it is visible and where its window is. Arguments or environment for a running agents' instance are refused, since they apply only when a process starts. LaunchServices and the browser's menus are never used, because with two processes of one bundle LaunchServices may pick the user's.
- If the agents' instance runs without Agent Screen's pipe, as one left from an earlier run of Agent Screen, Agent Screen asks it to quit and starts a new one. Only the pipe opens a window without activating the browser.
- Otherwise Agent Screen starts the agents' instance, whether or not the user runs the browser. Agents then never use the user's default profile. The one exception is a browser recorded as refusing a second instance (below), which starts as before when no one runs it, since an agents' instance would make it refuse the user's own launch later.
- The agent's `--remote-debugging-port` and `--remote-debugging-address` are dropped for the agents' instance. Any local process could drive the signed-in agents' profile through such a port. Agent Screen's own channel is a pipe.

The agents' profile is `~/Library/Application Support/Agent Screen/Browser Profiles/<bundle id>`. A test or development instance of Agent Screen uses `Browser Profiles (<instance>)`, so tests never touch the agents' real profile. The profile persists. A sign-in the user does there once, through a Help Request, carries over to later sessions. All agent sessions share it and its sign-ins, and the `launch_app` description says so.

The agents' instance belongs to no display. No display records it among the apps it started, keeps a catch on it, or hands it to a session's next display, because every session uses it. A browser window that a page in it opens, such as a sign-in popup, goes where Chromium places it, next to the window that opened it.

When a display's windows are dismissed, the agents' windows close through the pipe (`Target.closeTarget`). A window the pipe can't close, as Dia's, gets no accessibility press: Dia moves a window it closes to the middle of the main screen. Instead the instance quits when it has no other window. The agents' instance quits once no window of it is left, after a display goes, and when Agent Screen quits. It quits hidden first, so no window it closes shows, through the pipe (`Browser.close`), then with SIGTERM after 1 second and SIGKILL after 2 more, since Dia takes longer to quit than a display takes to go. A window left on a display that goes would land on the user's screen, and an instance left running would take the user's Dock click and links (below).

## Refusing browsers

A browser may allow only one instance. Then the started process quits at once while the user's keeps running. Only that signal counts as a refusal: a timeout, a crash or a missing window is not remembered. On a refusal, `launch_app` opens the window in the user's process as before this ADR, the result says `shared: true` with a note that the window stops while the user works in that browser, and Agent Screen records the browser's version in the file `refuses own instance` beside the profiles, so later launches don't try again until the browser updates. A profile folder the refused launch created goes to the Trash. No browser measured so far refuses: Dia and Chrome both accept a second instance.

## Measured

On 2026-11-19, with the user's Dia running, `agentbrowser` with `E2E_BROWSER=Dia` started Dia on the agents' profile with the pipe. The process kept running beside the user's, answered on its pipe, and its first window was caught onto the display. A first run on 2026-11-18 had reported that Dia quit at once. That report was false: Agent Screen took a process it had just started for gone because the bundle's list of running apps didn't show it yet. A started process now counts as quit only once it ends.

`agentbrowser` on Chrome, with a stand-in for the user's Chrome, on 2026-11-19 (run chrome3):

- Chrome started a second process on the agents' profile with the pipe beside the stand-in, and its first window opened on the display.
- Posted clicks, typing and a submit reached its page.
- `Target.createTarget` opened another window in the same instance at the display's frame. The front app didn't change while it did.
- With only the stand-in running, and with both running, a link through LaunchServices (`open -g -b`) reached the stand-in. It carried no cookie of the agents' profile.
- The agents' instance did become the front app three times while it started, and Agent Screen gave the front back each time. That run didn't time how long each lasted; the test now records it. Agent Screen does the same for any app an agent starts. A process Agent Screen starts with `posix_spawn` activates itself at launch. Only `posix_spawn` can hand it the private pipe, since LaunchServices passes no file descriptors. A window opened through the pipe never activated it.

On Dia (runs dia5 to dia7), Dia's answer to `newWindow` was a window of no size that never showed. Its windows' close button takes no accessibility press, and Dia moves a window it closes to the middle of the main screen: as the test app quit, the agents' Dia window showed there for 48 to 700 ms. Both led to the rules above, not yet run on a display.

The test's cleanup found the agents' instance with `pkill -f <profile>`. That pattern is a regular expression, and the parentheses in `Browser Profiles (e2e)` made it match nothing. The cleanup now matches the exact `--user-data-dir=` argument.

Dia's first window comes 1728 by 1084 points, the main screen's size, ignoring `--window-size`. When the agents' window reaches past the display, Agent Screen sets that one window's frame through Chromium (`Browser.setWindowBounds`).

## Not yet verified

- `agentbrowser` on Dia with the rules above: the second launch's page in the agents' window on the display, and nothing on the user's screen as the instance quits.
- `diafocused`, with the user at the Mac, for Dia and for Chrome (`E2E_BROWSER="Google Chrome"`): with the user's browser in front and their window focused, the agents' window takes posted clicks and keys, and the user's window keeps its focus.

## Consequences

- The agents' instance has its own sign-ins, history and extensions. An agent that needs the user's logged-in session in the user's own profile still asks for a Handover of a user window. A handed-over window, Safari and Finder keep today's behavior from #110 until the owner decides otherwise.
- ADR 0007's rule decides each new window of the agents' instance only through the launch that asked for it, as above.
- LaunchServices hands a link, a Dock click or `open -a` to a running instance of the bundle, whatever its profile. While an agent display holds an agents' window and the user's own browser isn't running, those reach the agents' instance. Agent Screen gives the front back when the agents' instance comes to the front, as for any app an agent started, so the agents' profile doesn't take the user's typing. The instance quits with its last window, so the state lasts only while an agent uses the browser.
- Focus borrowing for windows that can't get their own process, such as a Handover, needs its own decision and ADR.
