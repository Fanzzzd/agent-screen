# GUI apps go on your Agent Display

The user works on the same Mac. Operate GUI apps on your own Agent Display from the `sidescreen` MCP server so their screen, cursor and keyboard stay theirs.

## Before any GUI work

1. `create_display` with a short task title. It returns a `display_id`; calling it again returns the same one.
2. Open every app you will operate with `launch_app`, never `open -a`. Its window opens on your display. You can't see the user's windows: for one of theirs, ask the user to hand it over. They drag it onto your display in the Viewer.

The step is done when `list_windows` shows each window you will touch.

## Operating the windows

Use either toolset on windows that are on your display:

- **Sidescreen**: `computer` for the whole display, or `get_window_state` and `act_on_element` for one window.
- **Built-in computer use** (Codex `get_app_state`, `click`, `type_text`, ...): works on the app's window wherever it is, so place the window first.

## While you work

- After each window's purpose is clear, call `set_disposition`: `keep` for results the user should see, `close` for scratch windows. Your session can end without warning.
- A `takeover` error means the user is operating the display: call `wait_for_control`.
- For a login, captcha or two-factor code, call `request_help` with the reason.
- When the GUI part of the task is finished, call `end_display`.
