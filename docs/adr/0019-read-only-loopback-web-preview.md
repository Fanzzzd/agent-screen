# Web Preview is read-only, loopback-only, and addressed by a display token

Accepted, 2026-11-15.

Users chatting with an agent in a terminal or desktop client want to watch the agent's Agent Display in a local browser without opening the Viewer. A handed-over window on an Agent Display can show sensitive content such as a sign-in page, and macOS Screen Sharing exposes the user's own screens rather than one Agent Display.

Agent Screen runs an HTTP/1.1 GET server bound to `127.0.0.1:0` on a background runtime (`apps/macos/src/web`). The operating system picks a free loopback port, and the app writes `{"port": <n>}` with mode `0600` under `~/Library/Application Support/Agent Screen/` (`web.json`, or `web-<instance>.json` when `AGENTSCREEN_INSTANCE` is set) and removes the file on exit. No network interface outside this Mac can connect.

Each adopted Agent Display gets a fresh 128-bit random token (`crates/model/src/web_preview.rs`, filled from `arc4random_buf`). Only live Agent Displays have tokens; the user's own screens and Spares never get one. When the display ends, or becomes a Review Display after `end_display` or disconnect, Agent Screen expires its token immediately. Every unknown, malformed, or expired token, and every unknown path, returns the same `404 Not Found` response so callers cannot tell a wrong token from an ended display.

Three GET routes serve a live token:

- `/d/<token>` returns a static HTML page with the Display Label, a localized read-only notice (`Read-only preview` or `只读预览`), and an `<img>` pointing at `/d/<token>/stream`, sent with the HTTP response header `Content-Security-Policy: default-src 'none'; img-src 'self'; style-src 'unsafe-inline'`.
- `/d/<token>/stream` streams `multipart/x-mixed-replace` JPEG frames at up to 8 fps, cuts the connection immediately when the display ends, and drops stalled clients after a 10-second write timeout.
- `/d/<token>/frame.jpg` returns one JPEG frame, serving the running feed's latest encoded frame when a feed is open and otherwise holding a 5-second grace lease so polling does not cycle the capture stream.

Web Preview is read-only. The page executes no script, opens no return channel, accepts only `GET`, and never sends pointer or keyboard events, triggers a Takeover, or activates an app, keeping ADR 0001 and ADR 0004 intact.

Web Preview shares the ScreenCaptureKit stream that Peek and the Viewer use (`mac/capture.rs` `Capture::start` and `Latest`). When a browser connects and no stream runs for that display, `Streams` starts a low-rate `"web"` subscription at 8 fps and stops it when the last browser lease ends. JPEG encoding runs on worker threads, downscales frames wider than 1280 pixels proportionally, gates new encodes on `Latest.delivered` (which advances only when ScreenCaptureKit delivers a `Complete` changed frame, without turning on `Latest.changes` CPU luma sampling), and never runs on the main thread or when no lease is active.

`create_display` returns `preview_url` alongside `display_id` so the Agent Session can share the link in chat, and each live display's menu in Peek and the menu bar offers Copy Web Link (`复制网页链接`).
