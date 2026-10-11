# Changelog

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
