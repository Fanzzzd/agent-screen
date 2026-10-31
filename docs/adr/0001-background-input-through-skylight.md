# Input reaches agent windows through SkyLight, not the real cursor

Agent Sessions and the Viewer deliver clicks and keys to windows on an Agent Display by posting events straight to the owning process with SkyLight's private `SLEventPostToPid`, plus a focus-without-raise handshake (recipe from trycua/cua, MIT). The obvious alternative, warping the real cursor onto the virtual display and clicking there, is public API and works with every app, but it takes the mouse and keyboard away from the user, which is the problem Agent Screen exists to solve.

[ADR 0013](0013-user-approved-input-queue.md) records the accepted exception for operations that cannot run in the background: a visible, user-approved Input Queue shared by sessions. Background routes remain first. Complete drags without one can run with the real pointer only after the user allows them, under the visible grant in ADR 0013.

## Consequences

- Only apps that accept per-process events work. Canvas apps and games that filter them need a brief foreground activation instead.
- The first click on an inactive window only activates it, as with a real click.
- A macOS update can break the private symbols. Every call falls back to the public `CGEventPostToPid`, which reaches fewer apps.
- An app on an Agent Display can still bring itself to the front, as Electron does to show a window. When the agent started the app and the user didn't just act on it, Agent Screen gives the front back to the user's app as soon as macOS reports the change, a few ms later (issue #67). The ime path (ADR 0008) holds the front on purpose and is left alone.
