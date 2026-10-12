# Changelog

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
