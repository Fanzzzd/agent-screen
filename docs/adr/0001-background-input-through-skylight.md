# Input reaches agent windows through SkyLight, not the real cursor

Agent Sessions and the Viewer deliver clicks and keys to windows on an Agent Display by posting events straight to the owning process with SkyLight's private `SLEventPostToPid`, plus a focus-without-raise handshake (recipe from trycua/cua, MIT). The obvious alternative, warping the real cursor onto the virtual display and clicking there, is public API and works with every app, but it takes the mouse and keyboard away from the user, which is the problem Agent Screen exists to solve.

## Consequences

- Only apps that accept per-process events work. Canvas apps and games that filter them need a brief foreground activation instead.
- The first click on an inactive window only activates it, as with a real click.
- A macOS update can break the private symbols. Every call falls back to the public `CGEventPostToPid`, which reaches fewer apps.
