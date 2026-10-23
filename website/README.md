# agentscreen.app

The landing page for Agent Screen: static files in `public/`, served by a Cloudflare Worker with static assets. There is no build step. `src/index.js` sends www.agentscreen.app to agentscreen.app and serves everything else from `public/`; `wrangler.jsonc` attaches both hostnames as custom domains.

- Preview: `wrangler dev` (from this folder), then open http://localhost:8787.
- Deploy: `wrangler deploy`.
- Copy follows the root `README.md`. Change the page when the README's claims change.
- `public/og.png` is a 1200×630 screenshot of `og/og.html`. To redo it, copy `og/og.html` into `public/`, run `wrangler dev`, screenshot the page at 1200×630 (for example with headless Chrome: `--window-size=1200,630 --screenshot`), and delete the copy.
- The fleet clips in `public/media/` come from `promo/public/fleet-*.mp4` (8× speed, 880 px wide), encoded with ffmpeg as H.264 without audio. `demo.mp4` is the first 18.4 s of `rust/target/demo.mov`, the full-size recording `cargo e2e demo` leaves behind, scaled to 1600 px. The three `state-*.png` strips are the Viewer header cropped from that recording at 3 s, 9 s and 12.5 s, and `state-*-buttons.png` are their right ends for narrow screens.
- `logo.svg` (also `docs/media/logo.svg` for the README) is the icon's two screens and pointer without the tile. The favicons and `apple-touch-icon.png` are the installed app's icon as macOS draws it from `AppIcon.icon` (`NSWorkspace.shared.icon(forFile:)` at 1024 px, then cropped and scaled with `sips`). Redo them when the app icon changes.
- Colors in `styles.css` are the app's own, from `rust/agent-screen/src/ui/theme.rs`.
