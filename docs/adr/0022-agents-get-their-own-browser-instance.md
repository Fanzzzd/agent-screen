# A browser the user runs gets a separate instance for agents

Agents' browser windows used to open in the user's own browser process. When the user worked in that browser, it was their front app with their window focused. Agent Screen then posted nothing to the process: posted keys would reach the user's window, and a posted click would make the agent's window key and take the user's typing (#110). Clicks went in as accessibility presses only, and Dia's web elements take none. So while the user watched a video in their Dia, every agent's Dia window stopped (#222).

When `launch_app` targets a Chromium browser that the user already runs, Agent Screen starts a second process of that browser for agents. The process has its own `--user-data-dir`, and Agent Screen starts it with the private DevTools pipe (ADR 0010). It is never the user's front app, so posted clicks and keys reach it while the user works in their own process, and web drags and selects work through the pipe.

## The rule

- A Chromium browser is an app that handles web URLs and ships `chrome_100_percent.pak` (ADR 0010).
- If the agent names a profile with `--user-data-dir`, that wins and nothing below applies.
- If the agents' instance already runs, the new window opens in it. Agent Screen asks through the instance's pipe (`Target.createTarget` with `newWindow` and the display's frame), so Chromium makes the window on the display. If the instance has no pipe, for example after Agent Screen restarted, Agent Screen starts the browser again on the same profile with `--new-window`. Chromium hands that command line to the process that holds the profile, and the launch's catch moves the window onto the display. LaunchServices is never asked, because with two processes of one bundle it may pick the user's. Arguments or environment for a running agents' instance are refused, since they apply only when a process starts.
- If the agents' instance doesn't run and the user's browser does, Agent Screen starts the agents' instance.
- If neither runs, the browser starts as before, on its default profile.

The agents' profile is `~/Library/Application Support/Agent Screen/Browser Profiles/<bundle id>`. A test or development instance of Agent Screen uses `Browser Profiles (<instance>)`, so tests never touch the agents' real profile. The profile persists. A sign-in the user does there once, through a Help Request, carries over to later sessions. All agents share it, as they share one Agent Screen.

## Not yet verified

Status: proposed. Nothing here has run against Dia yet. The first e2e display creation for this work hit #220: it moved one of the user's Agent Displays, so display creation is on hold until #220's fix is installed. These still need to be shown, by `agentbrowser` and `diafocused`:

- Dia accepts a second process on its own `--user-data-dir` while the user's Dia runs, answers on the pipe and opens its first window on the display, with nothing on the user's screen. ADR 0010's Chrome measurements apply to Chrome. Dia's first run on a fresh profile may show onboarding or ask for a sign-in.
- `Target.createTarget` with `left`, `top`, `width` and `height` puts Dia's window on the display.
- With the user's Dia in front and their window focused, the agents' window takes posted clicks and keys, and the user's window keeps its focus.

If Dia refuses a second instance, the fallback is to keep #110's behavior for Dia, and to make the refusal say that the agent should ask the user to switch to another app or hand over a window.

## Consequences

- The agents' instance has its own sign-ins, history and extensions. An agent that needs the user's logged-in session in the user's own profile still asks for a Handover of a user window. A handed-over window, Safari and Finder keep today's behavior from #110 until the owner decides otherwise.
- ADR 0007's rule still decides each new window of the agents' instance. The user doesn't act on that instance, so the rule gives its new windows to the agent.
- The agents' instance keeps running when Agent Screen quits, as any app `launch_app` started does (ADR 0010). Agent Screen doesn't quit it when a display ends, because other sessions may have windows in it.
- A link the user clicks in another app goes through LaunchServices, which may pick either process of the bundle. If it picks the agents' instance, the user's link opens on an Agent Display. Not measured yet: the check is to start the agents' instance beside the user's browser and open a URL with `open -g` from a test.
- Focus borrowing for windows that can't get their own process, such as a Handover, needs its own decision and ADR.
