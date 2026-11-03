# Input reaches agent windows through SkyLight, not the real cursor

Agent Sessions and the Viewer deliver clicks and keys to windows on an Agent Display by posting events straight to the owning process with SkyLight's private `SLEventPostToPid`, plus a focus-without-raise handshake (recipe from trycua/cua, MIT). The obvious alternative, warping the real cursor onto the virtual display and clicking there, is public API and works with every app, but it takes the mouse and keyboard away from the user, which is the problem Agent Screen exists to solve.

## Consequences

- Only apps that accept per-process events work. Canvas apps and games that filter them need a brief foreground activation instead.
- The first click on an inactive window only activates it, as with a real click.
- In an app the user also uses, keys posted to the app reach its key window, which can be the user's window on their own screen. The focus record alone doesn't take key focus from another window of an app in the background. A click on an empty spot of the agent's title bar does, and so does setting the agent's window AXMain, with the app still in the background. Before any key goes out, Accessibility must report the app's focused window as the agent's or on an Agent Display. Otherwise the call fails, names that window and sends nothing (#120, `agent_screen_model::key_window`).
- A macOS update can break the private symbols. Every call falls back to the public `CGEventPostToPid`, which reaches fewer apps.
- An app on an Agent Display can still bring itself to the front, as Electron does to show a window. When the agent started the app and the user didn't just act on it, Agent Screen gives the front back to the user's app as soon as macOS reports the change, a few ms later (issue #67). The ime path (ADR 0008) holds the front on purpose and is left alone.
