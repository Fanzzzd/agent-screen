# A new window is the agent's when the agent just acted and the user didn't

An app the agent started keeps running after `launch_app` returns. The user may use it too, so Sidescreen has to decide, for each new window the app makes, whether it goes to the agent's display. A catch on the app's AXWindowCreated notification decides as the window appears. Deciding later shows the window on the user's screen, and AeroSpace lays out the user's workspace around it.

The catch takes the window when the display was driven in the last 3 s and the user didn't act on the app meanwhile.

- **The display was driven.** The agent's input, element actions and launches on the display count, and so does the user's input through the display's Viewer. A cold start counts for 30 s, the time a slow app may take to show its first window.
- **The user acted.** A key press with the app frontmost, or a click on one of its windows or in Finder, the Dock or Spotlight, which open files and apps. Global NSEvent monitors report these. Events Sidescreen posts to a pid don't reach those monitors. They do reset the HID idle timer, so that timer can't tell the two apart.

When only the user acted, the window is theirs, and nothing moves it later. When both acted, or neither did, the window stays where it opened, and the agent can move it with `place_window` from `list_windows` with `strays: true`. Taking back any window of the app makes all its windows the user's.

We tried two rules before this one. Under the first, the catch left alone any window the app made while it was frontmost. That left the agent's own windows on the user's screen whenever the app came to the front. Under the second, any window within 3 s of an agent action went to the agent. That took windows the user opened while the agent worked, and it marked a slow agent window as the user's for good.

The rule still guesses wrong when the user clicks a link in another app within 3 s of an agent action and the link opens in a browser the agent started. That window goes to the agent's display, and the user takes it back with Windows ▸ Take Back Window.
