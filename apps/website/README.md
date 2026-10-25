# agentscreen.app

The landing page for Agent Screen: static files in `public/`, served by a Cloudflare Worker with static assets. There is no build step. `src/index.js` sends www.agentscreen.app to agentscreen.app and serves everything else from `public/`; `wrangler.jsonc` attaches both hostnames as custom domains.

- Preview: `pnpm dev` (from this folder, after `pnpm install` at the root), then open http://localhost:8787. The Worker answers byte ranges for `/media/`, which Safari needs to play MP4, and keeps revalidation headers so repeat visits get a 304.
- Deploy: `pnpm run deploy`.
- Copy follows the root `README.md`. Change the page when the README's claims change.
- `public/og.png` is a 1200×630 screenshot of `og/og.html`. To redo it, copy `og/og.html` into `public/`, run `pnpm dev`, screenshot the page at 1200×630 (for example with headless Chrome: `--window-size=1200,630 --screenshot`), and delete the copy.
- The films in `public/media/` are rendered in the private promo repo, `Fanzzzd/agent-screen-promo`.
- `logo.svg` (also `docs/media/logo.svg` for the README) is the icon's two screens and pointer without the tile. The favicons and `apple-touch-icon.png` are the installed app's icon as macOS draws it from `AppIcon.icon` (`NSWorkspace.shared.icon(forFile:)` at 1024 px, then cropped and scaled with `sips`). Redo them when the app icon changes.
- Colors in `styles.css` are the app's own, from `apps/macos/src/ui/theme.rs`.
