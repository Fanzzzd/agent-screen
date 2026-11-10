# agentscreen.app

The landing page for Agent Screen: static files in `public/`, served by a Cloudflare Worker with static assets. There is no build step. `src/index.js` sends www.agentscreen.app to agentscreen.app and serves everything else from `public/`; `wrangler.jsonc` attaches both hostnames as custom domains.

- Preview: `pnpm dev` (from this folder, after `pnpm install` at the root), then open http://localhost:8787. The Worker answers byte ranges for `/media/`, which Safari needs to play MP4. In `wrangler dev` the assets binding ignores `Range` and returns the whole file, so the Worker's range code stays until production shows otherwise.
- Caching: every `/media/` URL in `index.html` ends in `?v=<hash of the file>`. The Worker tells browsers to keep a versioned `/media/` file for a year. Everything else, and `/media/` without `?v=`, is revalidated on each visit and gets a 304. After any file in `public/media/` changes, run `node scripts/version-media.mjs`. It rewrites the versions in `index.html`. With `--check` it changes nothing and fails if a version is stale. A stale version leaves returning visitors with the old file for up to a year, so `pnpm run deploy` and CI both run the check first.
- Deploy: `pnpm run deploy`.
- Copy follows the root `README.md`. Change the page when the README's claims change.
- `public/og.png` is a 1200×630 screenshot of `og/og.html`. To redo it, copy `og/og.html` into `public/`, run `pnpm dev`, screenshot the page at 1200×630 (for example with headless Chrome: `--window-size=1200,630 --screenshot`), and delete the copy.
- The films in `public/media/` are rendered in the private promo repo, `Fanzzzd/agent-screen-promo`. Each film ships twice: `name.mp4` (H.264) and `name.av1.mp4` (AV1, about half the size). The page's script gives the AV1 file to browsers that report smooth AV1 decoding and H.264 to the rest, such as Safari on a Mac without an AV1 decoder. The script asks about 8-bit AV1 Main at level 4.0, 30 fps and up to 1920x1080 (`av01.0.08M.08`); a larger or 10-bit film needs that string and size changed. A browser that doesn't answer within 300 ms, or whose AV1 file fails, plays H.264. The three Peek tiles are H.264 only, because AV1 saves nothing at 12 KB. Posters are WebP.
- To add or replace a film:
  1. Put the H.264 file in `public/media/` and, if phones get their own cut, a `-m` file.
  2. Encode AV1 from the best source you have, the render master if there is one:
     `ffmpeg -i master.mp4 -an -c:v libsvtav1 -preset 2 -crf 42 -svtav1-params keyint=10s:scm=1:tune=0 -pix_fmt yuv420p -movflags +faststart name.av1.mp4`
     Use CRF 42 when the source is the site's own H.264 file. `hero.av1.mp4` uses CRF 52 because it comes from the 4.6 MB master. Compare 100% crops of small text with the H.264 file before shipping a higher CRF.
  3. Make the poster from the first frame: `cwebp -q 88 -m 6 -sharp_yuv frame.png -o name.webp`.
  4. In `index.html` give the `<video>` `data-src`, `data-av1` and `data-poster`, plus `data-src-m`, `data-av1-m` and `data-poster-m` for a phone cut. The hero's two `<link rel="preload">` lines must name the same poster files as its `data-poster` attributes.
  5. Run `node scripts/version-media.mjs`. When you replace `name.mp4`, encode `name.av1.mp4` again too; nothing checks that the pair match.
- `logo.svg` (also `docs/media/logo.svg` for the README) is the icon's two screens and pointer without the tile. The favicons and `apple-touch-icon.png` are the installed app's icon as macOS draws it from `AppIcon.icon` (`NSWorkspace.shared.icon(forFile:)` at 1024 px, then cropped and scaled with `sips`). Redo them when the app icon changes.
- Colors in `styles.css` are the app's own, from `apps/macos/src/ui/theme.rs`.
