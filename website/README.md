# agentscreen.app

The landing page for Agent Screen: static files in `public/`, served by a Cloudflare Worker with static assets. There is no build step. `src/index.js` sends www.agentscreen.app to agentscreen.app and serves everything else from `public/`; `wrangler.jsonc` attaches both hostnames as custom domains.

- Preview: `wrangler dev` (from this folder), then open http://localhost:8787. The Worker answers byte ranges for `/media/`, which Safari needs to play MP4, and keeps revalidation headers so repeat visits get a 304.
- Deploy: `wrangler deploy`.
- Copy follows the root `README.md`. Change the page when the README's claims change.
- `public/og.png` is a 1200×630 screenshot of `og/og.html`. To redo it, copy `og/og.html` into `public/`, run `wrangler dev`, screenshot the page at 1200×630 (for example with headless Chrome: `--window-size=1200,630 --screenshot`), and delete the copy.
- Films in `public/media/`:
  - `hero.mp4` (and the 4:3 `hero-m.mp4` for phones) is `films/hero.html` rendered without sound: the teaser's opening and help beat with its captions. The file starts at 13.4 s, on the twelve displays, and crossfades back to 0 s, so the poster is the first frame and no seek is needed. `hero-m.mp4` crops 1440×1080 from x = 45 until the Viewer opens at 24 s, then an ffmpeg `zoompan` pulls out to the whole frame on black, because the Viewer is 1640 px wide. `docs/media/demo.gif` is the same render in story order at 720 px, 8 fps.
  - `peek.mp4` (`peek-m.mp4` from `films/peek-m.html` for phones), `help.mp4` (16:9 on phones too, since the Viewer is wider than a 4:3 crop) and `handover.mp4` (`handover-m.mp4` for phones is a 1440×1080 crop that pans with the window: x = 58, 480, 240, then back to 58) are rendered from `films/peek.html`, `films/help.html` and `films/handover.html` with HyperFrames (`npx hyperframes@0.8.180 render -c peek.html -o renders/peek.mp4 --quality high --video-frame-format png` inside `films/`), then scaled to 1440 px at 30 fps with ffmpeg.
  - `films/` is a copy of the teaser project. Only the three website compositions and `06-help.html` are tracked; copy `assets/` and `node_modules/` from `videos/agent-screen-teaser` before rendering.
  - `fleet-*.mp4` are the active seconds of the teaser's `assets/fleet2` recordings (address bar shows acme.cloud), at 880 px.
- `logo.svg` (also `docs/media/logo.svg` for the README) is the icon's two screens and pointer without the tile. The favicons and `apple-touch-icon.png` are the installed app's icon as macOS draws it from `AppIcon.icon` (`NSWorkspace.shared.icon(forFile:)` at 1024 px, then cropped and scaled with `sips`). Redo them when the app icon changes.
- Colors in `styles.css` are the app's own, from `rust/agent-screen/src/ui/theme.rs`.
