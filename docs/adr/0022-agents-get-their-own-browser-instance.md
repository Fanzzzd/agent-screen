# Agents get their own browser instance

Agents' browser windows used to open in the user's own browser process. When the user worked in that browser, it was their front app with their window focused. Agent Screen then posted nothing to the process: posted keys would reach the user's window, and a posted click would make the agent's window key and take the user's typing (#110). Clicks went in as accessibility presses only, and Dia's web elements take none. So while the user watched a video in their Dia, every agent's Dia window stopped (#222).

When `launch_app` targets a Chromium browser verified for it, Agent Screen runs a separate process of that browser for agents. The process has its own `--user-data-dir`, and Agent Screen starts it with the private DevTools pipe (ADR 0010). It is never the user's front app, so posted clicks and keys reach it while the user works in their own process, and web drags and selects work through the pipe.

Status: proposed.

## The rule

- A Chromium browser is an app that handles web URLs and ships `chrome_100_percent.pak` (ADR 0010).
- If the agent names a profile with `--user-data-dir`, that wins and nothing below applies.
- Only a browser on `agent_browser::VERIFIED` gets the agents' instance: one whose instance passed `agentbrowser` on a display, starting beside the user's without taking the front, with its windows coming through the pipe onto the display and nothing on the user's screen as it quits. Google Chrome is the only one so far. Dia and any other Chromium browser keep the behavior from before this ADR: while the user runs it, the agents' windows open in the user's process, and the result says `shared: true` with a note that the window stops while the user works in that browser and that Chrome works unattended. When no one runs it, it starts as asked. An agents' instance left from earlier is not used.
- If the agents' instance runs with Agent Screen's pipe, the new page opens in it through the pipe: `Target.createTarget` with `newWindow`, `background` and the display's frame. Only the window that shows that page answers the launch. Agent Screen reads the page's window frame from Chromium (`Browser.getWindowForTarget`) and takes the system window with that frame, so a window another session's page opens meanwhile is never answered to this agent. A browser that puts the page in a window it had open, as a tab, answers with that window when it is on the launch's display. When it is on another display, the tab is closed and the launch fails. A browser that keeps its pages in windows of its own, which Chromium gives no frame, answers `newWindow` with a window of no size that never shows; Agent Screen closes it and loads the page in the visible page of the agents' window on the launch's display. This path is for verifying Dia and has not passed a display run. Arguments or environment for a running agents' instance are refused, since they apply only when a process starts. LaunchServices and the browser's menus are never used, because with two processes of one bundle LaunchServices may pick the user's.
- If the agents' instance runs without Agent Screen's pipe, as one left from an earlier run of Agent Screen, Agent Screen asks it to quit and starts a new one. Only the pipe opens a window without activating the browser.
- Otherwise Agent Screen starts the agents' instance, whether or not the user runs the browser. It starts with `--no-startup-window`, and its first window comes through the pipe like any other: Chromium shows its startup window with activation, which would take the user's front app, and a window opened through the pipe never activates it. A browser that keeps its pages in windows of its own, as Dia does, starts with its startup window, which Dia opens without taking the front. Agents then never use the user's default profile. The one exception is a browser recorded as refusing a second instance (below), which starts as before when no one runs it, since an agents' instance would make it refuse the user's own launch later.
- The agent's `--remote-debugging-port` and `--remote-debugging-address` are dropped for the agents' instance. Any local process could drive the signed-in agents' profile through such a port. Agent Screen's own channel is a pipe.

The agents' profile is `~/Library/Application Support/Agent Screen/Browser Profiles/<bundle id>`. A test or development instance of Agent Screen uses `Browser Profiles (<instance>)`, so tests never touch the agents' real profile. The profile persists. A sign-in the user does there once, through a Help Request, carries over to later sessions. All agent sessions share it and its sign-ins, and the `launch_app` description says so.

The agents' instance belongs to no display. No display records it among the apps it started, keeps a catch on it, or hands it to a session's next display, because every session uses it. A browser window that a page in it opens, such as a sign-in popup, goes where Chromium places it, next to the window that opened it.

When a display's windows are dismissed, the agents' windows close through the pipe (`Target.closeTarget`). A window the pipe can't close, as Dia's, gets no accessibility press: Dia moves a window it closes to the middle of the main screen. Instead the instance quits when it has no other window. The agents' instance quits once its pipe counts no page, and when Agent Screen quits. It quits hidden first, so no window it closes shows, through the pipe (`Browser.close`), then with SIGTERM after 1 second and SIGKILL after 2 more, since Dia takes longer to quit than a display takes to go. A window left on a display that goes would land on the user's screen, and an instance left running would take the user's Dock click and links (below).

## Refusing browsers

A browser may allow only one instance. Then the started process quits at once while the user's keeps running. Only that signal counts as a refusal: a timeout, a crash or a missing window is not remembered. On a refusal, `launch_app` opens the window in the user's process as before this ADR, the result says `shared: true` with a note that the window stops while the user works in that browser, and Agent Screen records the browser's version in the file `refuses own instance` beside the profiles, so later launches don't try again until the browser updates. A profile folder the refused launch created goes to the Trash. No browser measured so far refuses: Dia and Chrome both accept a second instance.

## Measured

On 2026-11-19, with the user's Dia running, `agentbrowser` with `E2E_BROWSER=Dia` started Dia on the agents' profile with the pipe. The process kept running beside the user's, answered on its pipe, and its first window was caught onto the display. A first run on 2026-11-18 had reported that Dia quit at once. That report was false: Agent Screen took a process it had just started for gone because the bundle's list of running apps didn't show it yet. A started process now counts as quit only once it ends.

`agentbrowser` on Chrome, with a stand-in for the user's Chrome, passed every check on 2026-11-19 (run chrome4):

- Chrome started a second process on the agents' profile with the pipe beside the stand-in, with no startup window, and its first window came through the pipe onto the display.
- The front app, sampled every 50 ms through every launch, never changed. An earlier run (chrome3), before `--no-startup-window`, found the agents' instance front three times while it started.
- Posted clicks, typing and a submit reached its page.
- `Target.createTarget` opened another window in the same instance at the display's frame.
- With only the stand-in running, and with both running, a link through LaunchServices (`open -g -b`) reached the stand-in. It carried no cookie of the agents' profile.
- As the test app quit, Agent Screen closed the agents' windows and quit the instance, with nothing on the user's screen.

The stand-in itself, a Chrome an agent starts on a profile it names, still came to the front while it started, and Agent Screen gave the front back. That path predates this ADR and applies to any Chromium app an agent starts.

On Dia (runs dia5 to dia8), the first launch passes: the agents' Dia starts beside the user's, without taking the front, and its window is caught onto the display. Two faults remain open:

- Dia's answer to `newWindow` through the pipe is a window of no size that never shows. Loading the page in the visible page of the agents' window on the display found none of its 5 pages visible there. The second launch with a URL fails.
- Dia moves a window it closes or quits with to the middle of the main screen. As the test app quit, the agents' Dia window showed there for 48 to 700 ms, and for 85 ms with the instance hidden first.

The test's cleanup found the agents' instance with `pkill -f <profile>`. That pattern is a regular expression, and the parentheses in `Browser Profiles (e2e)` made it match nothing. The cleanup now matches the exact `--user-data-dir=` argument.

Dia's first window comes 1728 by 1084 points, the main screen's size, ignoring `--window-size`. When the agents' window reaches past the display, Agent Screen sets that one window's frame through Chromium (`Browser.setWindowBounds`).

## Not yet verified

- `agentbrowser` on Dia: the two faults above keep Dia off `VERIFIED`. The follow-up is its own issue.
- `diafocused`, with the user at the Mac, for Dia and for Chrome (`E2E_BROWSER="Google Chrome"`): with the user's browser in front and their window focused, the agents' window takes posted clicks and keys, and the user's window keeps its focus.

## Consequences

- The agents' instance has its own sign-ins, history and extensions. An agent that needs the user's logged-in session in the user's own profile still asks for a Handover of a user window. A handed-over window, Safari and Finder keep today's behavior from #110 until the owner decides otherwise.
- ADR 0007's rule decides each new window of the agents' instance only through the launch that asked for it, as above.
- LaunchServices hands a link, a Dock click or `open -a` to a running instance of the bundle, whatever its profile. While an agent display holds an agents' window and the user's own browser isn't running, those reach the agents' instance. When the agents' instance comes to the front by something other than Agent Screen, Agent Screen decides by the user's latest input (`agent_browser::agents_activated`). If the user just used one of the agents' windows, it leaves it. Otherwise it gives the front back, so the agents' profile never takes the user's typing. If the user's input just before went to an app that opens others (the Dock with Launchpad, Spotlight, Finder, Raycast, Alfred) and the user's own browser isn't running, the user meant to open their browser: Agent Screen also starts it on its default profile, as a new instance through LaunchServices (`NSWorkspace` with `createsNewApplicationInstance`), since a plain launch would reach the agents' instance again. LaunchServices starts it, so it inherits none of Agent Screen's privacy grants. With no such input, as when the agents' instance brings itself forward for a popup, only the front comes back. A link's URL is not forwarded yet: it opens in the agents' instance, on an Agent Display. Telling a link's page from one the agent opened needs Chromium's target events with their opener; that is a follow-up. The instance quits once its pipe counts no page, checked every 2 seconds after it has run 30 seconds and whenever a display goes, so the state lasts only while an agent uses the browser. Before any signal, Agent Screen checks that the pid still runs with the agents' `--user-data-dir`, since a pid can be reused.
- Focus borrowing for windows that can't get their own process, such as a Handover, needs its own decision and ADR.
