# The app and its MCP server are one Rust binary

Sidescreen is being rewritten in Rust so it can later run on Windows and Linux. The UI uses GPUI (Zed's UI framework, pinned to a main-branch commit for AccessKit support and accessory launch), and the MCP server uses rmcp. `Sidescreen mcp` runs the MCP server from the app binary itself, so a client registers one path and the server and the app it talks to always come from the same build. The server still only talks to the running app through the control socket, whose NDJSON protocol is the contract every platform implements.

This supersedes [ADR 0003](0003-upstream-effect-mcpserver.md): the TypeScript server in `packages/mcp` is gone. Its test scripts live on in `rust/tests/mcp` and drive the Rust server by default.

GPUI redraws the whole window from a display link on every frame, where the Swift app swapped a layer's contents. With the Viewer open on a display running at 60 fps, the Rust app uses 5.0% CPU against the Swift app's 2.0%; we accept that for one UI codebase across platforms. GPUI also clips live surfaces to rectangles only, so rounded corners are painted over them, and it ignores layer transforms, so the Viewer's open and close animate the native window frame instead of scaling a snapshot.
