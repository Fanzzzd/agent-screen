# GUI apps go on your Agent Display

The user works on the same Mac. Operate GUI apps on your own Agent Display from the `agent-screen` MCP server so their screen, cursor and keyboard stay theirs.

## Before any GUI work

1. `create_display` with a short task title. It returns a `display_id`; calling it again returns the same one, also from your subagents. Leave the GUI work to one of them.
2. Open every app you will operate with `launch_app`, never `open -a`. Its window opens on your display. You can't see the user's windows: for one of theirs, or for an app you already opened another way, call `place_window` with the app's name. Agent Screen asks the user, and the call returns once they answer.

The step is done when `list_windows` shows each window you will touch.

## Operating the windows

Use either toolset on windows that are on your display:

- **Agent Screen**: `computer` for the whole display, or `get_window_state` and `act_on_element` for one window. Coordinates are screenshot pixels. `create_display` returns width and height in points and `scale` (screenshot pixels per point). The default 1280×800 display has scale 1.
- A large window's tree costs thousands of tokens. Pass `max_depth` to `get_window_state` to see the top levels first, and `result: "changes"` to `act_on_element` to get back only the elements that changed.
- **Built-in computer use** (Codex `get_app_state`, `click`, `type_text`, ...): works on the app's window wherever it is, so place the window first.

## While you work

- After each window's purpose is clear, call `set_disposition`: `keep` for results the user should see, `close` for scratch windows. Your session can end without warning.
- A `takeover` error means the user is operating the display: call `wait_for_control`. When you need the display back to go on, call `request_control` with a short reason; the user answers Let … Continue or Keep Control.
- An error saying the user closed your display means they want you to stop working on screen. Ask them before calling `create_display` again.
- For a login or two-factor code, call `request_help` with the reason, and `steps` when it takes more than one thing. If you get past it yourself, call `request_help` with `withdraw: true`.
- Before you solve a captcha, ask the user to confirm that challenge. A general instruction to finish the site is not that confirmation. After they confirm, solve and fill it when you can. Offer `request_help` if they prefer to do it, the challenge needs them, or you cannot complete it. Captcha consent is not approval to create an account. Ask because filling a captcha acts as the user on a site that is checking they are present.
- To resize or move a window on your display, call `place_window` with its `window_id` and `x`, `y`, `width` or `height` in screenshot pixels. Don't use System Events or AppleScript: their coordinates are global, and a window sent there lands on the user's screen.
- To show the user what you did, `record_display` with `start` before and `stop` after. It returns the video and a timeline of your actions.
- To choose in a dropdown such as a web page's select (a `popupbutton` in the window state), call `act_on_element` with `set_value` and the option's text. Don't open its menu and click an item: macOS closes an open menu at the user's next click or key, and sends their keys to it while it is open.
- `type` inserts finished text and skips the input method. Use `via_input_method` only to test input-method behavior: it uses the user's keyboard for the call.
- When the GUI part of the task is finished, call `end_display`.
