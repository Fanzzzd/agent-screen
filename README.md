<div align="center">

<picture>
  <source media="(prefers-color-scheme: dark)" srcset="docs/media/logo-dark.svg">
  <img src="docs/media/logo.svg" alt="" width="72">
</picture>

<h1>Agent Screen</h1>

<p><strong>Give each AI agent its own Mac screen.</strong><br>
Claude Code or Codex opens apps, clicks and types on a virtual display, while your cursor, keyboard and windows stay yours.</p>

<p>
<a href="https://agentscreen.app">Website</a> ·
<a href="#install">Install</a> ·
<a href="https://github.com/Fanzzzd/agent-screen/releases/latest">Download</a> ·
<a href="docs/DEVELOPMENT.md">Develop</a>
</p>

[![Latest release](https://img.shields.io/github/v/release/Fanzzzd/agent-screen?label=release)](https://github.com/Fanzzzd/agent-screen/releases/latest)
[![CI](https://github.com/Fanzzzd/agent-screen/actions/workflows/ci.yml/badge.svg)](https://github.com/Fanzzzd/agent-screen/actions/workflows/ci.yml)
[![macOS 15 or later](https://img.shields.io/badge/macOS-15%2B-lightgrey)](#install)
[![MIT license](https://img.shields.io/github/license/Fanzzzd/agent-screen)](LICENSE)

</div>

![You work in your editor and spreadsheet while agents grab your pointer. Agent Screen moves each agent onto a display of its own, and your pointer stays put. One agent asks for help with a sign-in; you take over, sign in and hand it back.](docs/media/demo.gif)

<sub>An animation of Agent Screen's behavior, built from its interface and recordings of the repo's test sessions. More at [agentscreen.app](https://agentscreen.app).</sub>

Built-in computer use drives your real mouse and keyboard, so you stop working while the agent works. Agent Screen moves the agent onto a display of its own. You watch it from the menu bar, step in when it asks, and hand the screen back with one click.

## What you get

- **A display per agent session.** Run three agents at once and each gets its own screen. Subagents of one session share theirs. Each display can hold several apps and windows. Your windows stay put unless you give them to an agent.
- **Peek.** Hover the menu bar icon to see every agent's screen live.
- **Takeover.** Click into an agent's screen to take the wheel. The agent pauses until you press Let … Continue or explicitly tell it to continue in the conversation.
- **Help Request.** An agent stuck on a login or a captcha asks for you with a card under the menu bar icon and a notification, then waits.
- **Menus and controls through accessibility.** Agents press buttons and pick menu items by name, so a button still gets pressed after its window moves or resizes.
- **Handover.** Drag one of your windows onto an agent's screen, or tell the agent to take it in the conversation. Take it back with Take Back or ask the agent to return it.
- **Window authorization.** Tell the agent it may take windows for this session, from all apps or selected apps. It can then take matching windows without another confirmation. You can revoke that access from the conversation, Viewer or Peek.

|                                      | Agent Screen            | Built-in computer use | A virtual machine     |
| ------------------------------------ | --------------------- | --------------------- | --------------------- |
| You keep your mouse and keyboard     | Yes                   | No                    | Yes                   |
| Works with your real apps and logins | Yes                   | Yes                   | No, set them up again |
| Several agents at once               | Yes, one display each | No                    | One VM each           |
| Watch and step in                    | Menu bar, one click   | Watch your own screen | VM window             |

## Install

1. Install with Homebrew. It needs macOS 15 or later.

   ```sh
   brew tap fanzzzd/agent-screen
   brew trust fanzzzd/agent-screen
   brew install --cask agent-screen
   ```

   Homebrew loads casks from a tap outside its own only once you trust it, which `brew trust` does.

   Or download the DMG from the [latest release](https://github.com/Fanzzzd/agent-screen/releases/latest) and drag Agent Screen into Applications. Agent Screen is signed with a Developer ID and notarized by Apple, so it opens like any downloaded app.

   Agent Screen updates itself. It downloads a new version in the background and installs it once no Agent Display is open, so it never ends an agent's work. **Restart to Update…** in the menu bar icon's menu installs it sooner, and **Check for Updates…** checks now.

2. Open Agent Screen. Its Settings window asks for Screen & System Audio Recording and Accessibility. Both stay on this Mac.
3. Connect your agents. Press **Connect…** in Settings (right-click the menu bar icon and choose **Settings…**), or run:

   ```sh
   "/Applications/Agent Screen.app/Contents/MacOS/AgentScreen" mcp install
   ```

   This registers the MCP server with Claude Code for all projects and adds it to `~/.codex/config.toml`. Running it again is safe. For any other MCP client, such as Gemini CLI or OpenCode, add a stdio server with the command `/Applications/Agent Screen.app/Contents/MacOS/AgentScreen` and the argument `mcp`.

Then ask your agent something like "Use Agent Screen to open Calculator on your own display and work out 12 × 34." Start a new session after connecting, so the agent loads the tools. Once it has, Settings and the menu name the agent. If Agent Screen isn't running, the first tool call starts it.

## Agent tools

| Tool                                         | What it does                                                                                                                                                                               |
| -------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------ |
| `create_display`, `end_display`              | Get the session's display, one per session, or finish with it.                                                                                                                             |
| `launch_app`, `place_window`, `list_windows` | Open apps on the display and list its windows. `launch_app` takes an app name, bundle id or `.app` path, and `arguments` and `environment` for a new instance, as a development build needs. `list_windows` with `strays: true` lists windows of the agent's apps that opened elsewhere, and its windows that left the display, for `place_window`. `place_window` answers once the window has stayed on the display, and fails when it keeps going back. `place_window` with `x`, `y`, `width` or `height` sets a window's frame on the display, in screenshot pixels from its top-left corner, and returns the frame the app kept. |
| `computer`                                   | Anthropic-style actions on the whole display. Every action except `wait` and `cursor_position` returns a screenshot. `type` and `key` with `via_input_method` go through your input method, and use your keyboard for that call ([ADR 0008](docs/adr/0008-keys-through-the-input-method.md)). |
| `record_display`                             | Record the display to an MP4 in ~/Movies/Agent Screen, with the sound of its apps and a timeline of the agent's actions and Takeovers beside it.                                              |
| `get_window_state`, `act_on_element`         | One window's screenshot and accessibility tree, and actions on its numbered elements: press, click, set or insert text, read text. Numbers stay put while the element stays in the window. `max_depth` leaves out deeper elements, and `act_on_element` with `result: "changes"` returns only the elements that changed, or with `"none"` just that it's done. |
| `set_disposition`                            | Keep or close a window when the user closes the display.                                                                                                                                   |
| `wait_for_control`, `request_help`           | Wait out a Takeover, or ask the user to step in.                                                                                                                                           |
| `get_diagnostics` | Collect this MCP process's compiled build identity and the responding app's identity, without starting the app or creating a display. |
| `request_control`                            | During a Takeover, ask the user to hand the display back. With `user_instruction`, relay their explicit chat instruction to hand it back, or with `control: "user"` to pause. |
| `window_access` | Grant, inspect or revoke session window authorization, or return handed-over windows. Grant and return require the user's explicit `user_instruction`. |

A session can only see and drive its own display. `place_window` with `user_instruction` relays your instruction to hand over that window without another confirmation. Agent Screen trusts the agent's relay and records it; it does not read or independently verify your conversation. [docs/agents/AGENTS.md](docs/agents/AGENTS.md) is a convention you can paste into a project's `AGENTS.md` or `CLAUDE.md`.

## Your side

- **Peek.** Hover the menu bar icon for 300 ms to start opening Peek. Click the icon to pin it, including when hover already showed it. A later click closes pinned Peek; a click on the closed icon opens it pinned without the hover wait. Unpinned Peek waits 500 ms outside both the icon and Peek before starting to close. Moving between them keeps it open. Click outside or press Esc to dismiss. Right-click the icon for the menu: New Display, Recordings…, Settings…, Report a Problem… and Quit, and anything that needs fixing. Report a Problem… asks first, then copies this build's diagnostics and opens the bug report form; you paste and submit them. Displays in use come first, then those waiting for review. The narrow + creates a display of your own.
- **Takeover.** Your first click, scroll or key press in the Viewer, or Take Over, pauses the agent, including automatic window acquisition. Its input tools fail until you press Let … Continue or explicitly tell the agent to continue in the conversation. Collapsing the Viewer keeps the Takeover. The agent can ask you to hand control back; a request or timeout never resumes it by itself.
- **Help Request.** `request_help` drops a card from the menu bar icon with the display's picture and the agent's reason, and posts a notification. Take Over, on the card or the notification, opens the display in Takeover. Clicking the picture or the notification only opens it. Later puts the card away; the icon stays orange. When the agent gives steps, the Viewer lists them under its header while you are in control.
- **Notifications.** Besides Help Requests, Agent Screen tells you when an agent leaves windows on its display, by finishing or by disconnecting. Allow them in Settings.
- **Where an app's windows went.** When you bring an app to the front, from the Dock or a launcher, and all its windows are on Agent Displays, Peek comes down from the menu bar icon. That display's tile glows orange, with the app's windows outlined on it. Click the tile to open it in the Viewer, or Take Back to bring the windows to your screen. Peek goes after 5 seconds unless the pointer is on it. A click on the app's Dock icon does this too, also when the app is already the front app. Before setup is done, a notification names the displays instead.
- **Recording.** While an agent records its display, the Viewer header and Peek show a red Recording mark. The video has the sound of the apps whose every window is on that display. macOS separates sound by app, not by window, so an app you also have open elsewhere, such as your browser, stays silent in it, and the agent is told which apps were left out.
- **Recording playback.** Recordings… in the menu bar icon's menu opens the local MP4 library in `~/Movies/Agent Screen`, newest first, each with its display's label and start time. Select a video, play or pause it, or drag its progress bar to seek. While you drag, the video pauses and keeps showing its last frame, and it resumes on release if it was playing. Space plays or pauses, ← and → move 5 s, ↑ and ↓ pick a recording, and ⌘W closes the window. Show in Finder reveals the file. Show Pointer toggles a visual overlay without changing the video, sound or playhead; it reads Pointer Unavailable when the video has no usable pointer timeline. New recordings include pointer timing and coordinate metadata. Older videos still play; missing, damaged, incomplete or older timelines cannot accurately replay a pointer. The overlay hides during Takeover because user pointer movements were not recorded. Playback never sends input to the recorded apps.
- **Recording format.** New recordings prefer HEVC/MP4 when macOS advertises it and use H.264/MP4 when HEVC is unavailable. The agent receives the selected codec. HEVC may need a newer video player outside Agent Screen. Hardware encoder use and same-quality space savings have not been verified for ScreenCaptureKit.
- **System prompts.** A macOS prompt that opens on an Agent Display, such as a keychain password request, moves to your main display.
- **Agent Pointer.** The Viewer and Peek share an original rounded arrow with an even light rim and dark edge. It keeps the theme accent, glide and click ripple. The pointer is clipped to the display image, not drawn on the display itself, so agent screenshots don't contain it. Your Mac's system cursor is unchanged.
- **Wallpaper.** Choose… on the Wallpaper row in Settings sets the picture agent displays show in the Viewer and in recordings. Use Default puts back Agent Screen's own, which names each display in System Settings. Agents' screenshots leave the wallpaper out either way.
- **Handover.** Drag one of your windows onto the Viewer, or pick it under Give in the Viewer's header. Take Back brings any window back. A handed-over window returns to where it was. The × beside them closes the display.
- **Window Requests.** An agent can ask for one of your windows, such as an app it opened with a script. A card drops from the menu bar icon, with a notification, naming the window. Hand Over gives it the window as a Handover; Keep leaves it with you. If you already explicitly told the agent to take that window, or gave it matching continuing authorization, it can take the window without asking again.
- **Window authorization.** A continuing authorization covers this session's display and the apps you named, or all apps. Viewer and Peek show its scope, whether Takeover has paused it, and a way to revoke it. Revoking prevents future automatic acquisition; existing windows stay where they are. Ask the agent to return one or all handed-over windows separately. A window you take back, including one returned through the conversation, stays excluded until you explicitly authorize that window again. Unknown app names fail without granting access. Authorization ends when the display ends or the agent disconnects. It never permits taking another agent's windows, borrowing your keyboard or approving unrelated actions such as purchases.
- **Review.** When the agent stops using a display, the display closes by itself if it is empty, and otherwise shows Needs Review until you press Review Windows…. See [ADR 0004](docs/adr/0004-display-lifecycle.md).
- **Closing a display** lists its windows with what the agent suggested for each, keep or close, and you can change any of them. Esc cancels, ⌘Return confirms. Quitting Agent Screen applies the same plan without asking.

The app follows your Mac's language: Chinese when it comes first in your preferred languages, English otherwise.

## Privacy

Agent Screen runs on your Mac and sends nothing anywhere. Screen & System Audio Recording lets it show agent displays and take the screenshots agents ask for. Accessibility lets it click, type and place windows on them. Agents reach it through a socket only your user account can open.

## Known limits

- It uses private macOS APIs, so it can't be in the App Store, and a future macOS may break it.
- The menu bar strip in the Viewer mirrors your own front app. A click on it opens the menus of the app in front on the agent's display; status items there take no clicks. A window's zoom control can put its toolbar in that strip. For agent tools, recover with `place_window` on that window and an explicit frame. Agent Screen constrains that request below the strip and returns the actual frame. For a window already on its Agent Display, omitting frame arguments keeps its frame. Zoom itself is not automatically corrected.
- The first click on a window only activates it, the same as a real click on an inactive window.
- Clicks on an empty part of a display reach nothing.
- `launch_app` on a running single-window app such as Calculator gets no new window, and says so at once. Quit it first, or hand its window over.
- When an agent opens a new window in an app that is already running, the window shows on your screen for about 30 ms before it moves, 140 ms at most in tests.
- A new window of an app the agent started goes to the agent's display if the display was driven in the last 3 seconds, by the agent or by you through its Viewer, and you didn't click or type in that app, Finder, the Dock or a launcher such as Spotlight meanwhile. For an app the agent starts, this holds for 30 seconds from the launch, so a slow app's windows still arrive. A window you open stays on your screen. One the app opens by itself later, with nobody acting, goes to the agent's display too while none of the app's other windows is on your screen, as a VM viewer's new window after its guest reboots. One that opens while you both act, or with nobody acting while the app has a window on your screen, stays where it opened; the agent's next screenshot or window state names it, and the agent can move it with `place_window`. Take back one of the app's windows and the whole app is yours; hand a window over again and the agent can use it.
- A window that refuses to close, such as one with unsaved changes, is left to macOS, which moves it to your main display.
- Canvas apps and games that filter per-process events may ignore forwarded input.
- A drag in a web page works only in a Chromium or Electron app that `launch_app` started, which gets a private DevTools pipe (ADR 0010). In Safari, WebKit apps, a browser on its default profile, or an app you started, it fails and says so. Agent Screen won't press your mouse button for it. A fresh browser profile has separate sign-ins. It does not preserve your existing login or add a pipe to an already running window. Keep that window and use `request_help` if its login session is required.
- With the menu bar icon hidden, for example behind the notch, Peek and the Viewer drop from the top center of the screen under the pointer.
- Copy, paste and undo shortcuts need a focused app, so they do nothing in an app on an agent's display. Agents copy and paste with `act_on_element`'s `read_text` and `insert_text` instead; Command-A still selects all.
- Window managers move windows too. With AeroSpace, see [docs/aerospace.md](docs/aerospace.md). Others, such as yabai, are untested.
- The Viewer shows a display at one pixel per point, scaled up, so small text in it looks soft. Agents' screenshots are unaffected.

## Troubleshooting

For a bug, collect diagnostics in the failing session and follow [Reporting bugs](docs/REPORTING_BUGS.md). The guide covers stale MCP clients and older builds too.

- **The agent has no Agent Screen tools.** Start a new session after connecting; a running session doesn't load new MCP servers. `claude mcp get agent-screen` shows whether Claude Code has it.
- **Tools fail with a permissions error after an update.** macOS ties the grants to the app's signature. The menu, which opens when you right-click the menu bar icon, then shows **Finish Setup: Allow …**. Choose it, and in each list it opens switch Agent Screen off and on.
- **Tools say Agent Screen isn't running.** The first tool call starts it. If that fails, open `/Applications/Agent Screen.app` once yourself.
- **The agent says you closed its display.** Closing a display from Peek or the Viewer tells its agent to stop and ask you before it makes another. Tell it to go on, and it creates a new one.
- **The Mac stutters with many agent displays.** On every display change macOS reads its color records of every display ever connected, once per display online, and retries every 5 s until a read finishes within 50 ms. Turn on **Smooth Display Changes** in Agent Screen's Settings (or run `AgentScreen colorsync setup`). It asks for your administrator password once and installs a small tool that Agent Screen runs as root, through `sudo`, after displays change and every 30 minutes. The tool removes the records of displays that aren't connected and hold only what macOS made for them, and keeps display names in English and your languages, which makes the reads about five times faster. Profiles you chose for a display stay, and macOS makes a record again when you plug that display in. `AgentScreen colorsync remove` takes the tool away. Without it, Agent Screen refuses a new display when the reads would take too long.
- **Keys fail in a shared browser while you are idle.** The safety check uses the foreground app and its focused window, not recent user activity. If that foreground window differs from the agent's target, or Accessibility cannot verify it, keys stay refused. Use a supported accessibility action, or switch your foreground app when convenient before the agent retries. Agent Screen does not switch it for you.
- **Tools fail with "locked".** macOS blocks input and screenshots on every display while the screen is locked. Unlock and retry.

## Develop

To build and install from source instead of the DMG, with Rust installed: `apps/macos/scripts/install.sh`. See [docs/DEVELOPMENT.md](docs/DEVELOPMENT.md) for testing and how it works. To send a change, read [CONTRIBUTING.md](CONTRIBUTING.md). Agent Screen is MIT licensed. Its background input recipe is ported from [trycua/cua](https://github.com/trycua/cua); see [THIRD_PARTY_NOTICES.md](THIRD_PARTY_NOTICES.md).
