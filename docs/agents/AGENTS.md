# GUI apps go on your Agent Display

The user works on the same Mac. Operate GUI apps on your own Agent Display from the `agent-screen` MCP server so their screen, cursor and keyboard stay theirs.

## Before any GUI work

1. `create_display` with a short task title. It returns a `display_id`; calling it again returns the same one, also from your subagents. Leave the GUI work to one of them.
2. Open every app you will operate with `launch_app`, never `open -a`. Its window opens on your display. You can't see the user's windows: for one of theirs, ask the user to hand it over. They drag it onto your display in the Viewer, or click Windows in the Viewer's header and choose Hand Over.

The step is done when `list_windows` shows each window you will touch.

## Operating the windows

Use either toolset on windows that are on your display:

- **Agent Screen**: `computer` for the whole display, or `get_window_state` and `act_on_element` for one window.
- **Built-in computer use** (Codex `get_app_state`, `click`, `type_text`, ...): works on the app's window wherever it is, so place the window first.

## While you work

- After each window's purpose is clear, call `set_disposition`: `keep` for results the user should see, `close` for scratch windows. Your session can end without warning.
- A `takeover` error means the user is operating the display: call `wait_for_control`.
- An error saying the user closed your display means they want you to stop working on screen. Ask them before calling `create_display` again.
- For a login, captcha or two-factor code, call `request_help` with the reason, and `steps` when it takes more than one thing. If you get past it yourself, call `request_help` with `withdraw: true`.
- To show the user what you did, `record_display` with `start` before and `stop` after. It returns the video and a timeline of your actions.
- `type` inserts finished text and skips the input method. Use `via_input_method` only to test input-method behavior: it uses the user's keyboard for the call.
- When the GUI part of the task is finished, call `end_display`.
