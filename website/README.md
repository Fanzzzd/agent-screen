# agentscreen.app

The landing page for Agent Screen: static files in `public/`, served by a Cloudflare Worker with static assets. There is no build step. `src/index.js` sends www.agentscreen.app to agentscreen.app and serves everything else from `public/`; `wrangler.jsonc` attaches both hostnames as custom domains.

- Preview: `wrangler dev` (from this folder), then open http://localhost:8787.
- Deploy: `wrangler deploy`.
- Copy follows the root `README.md`. Change the page when the README's claims change.
- `public/og.png` is a 1200×630 screenshot of `og/og.html`. To redo it, copy `og/og.html` into `public/`, run `wrangler dev`, screenshot the page at 1200×630 (for example with headless Chrome: `--window-size=1200,630 --screenshot`), and delete the copy.
- The clips in `public/media/` come from `promo/public/fleet-*.mp4` (8× speed, 880 px wide) and `docs/media/demo.gif`, encoded with ffmpeg as H.264 without audio.
- `public/media/icon.png`, the favicons and `apple-touch-icon.png` are the installed app's icon as macOS draws it from `AppIcon.icon` (`NSWorkspace.shared.icon(forFile:)` at 1024 px, then cropped and scaled with `sips`). Redo them when the app icon changes.
- Colors in `styles.css` are the app's own, from `rust/agent-screen/src/ui/theme.rs`. The chips in "What you get" use the icons from `rust/agent-screen/assets/icons/` on the buttons that show them in the app.
