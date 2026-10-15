# Sidescreen promo video

A 30-second promo in English and Chinese, made with [Remotion](https://www.remotion.dev) and following its agent skills (`remotion-best-practices`).

```sh
npm i
npm run dev                                                # Remotion Studio
npx remotion render PromoEN out/sidescreen-promo-en.mp4    # or PromoZH
```

Scenes are in `src/scenes`, the words in `src/copy.ts`. `public/agent.mp4` is real footage: the Viewer while an agent works Calculator on its own display, recorded by `SIDESCREEN_INSTANCE=e2e cargo e2e promo` from `rust/`. `public/setup.png` is the setup window.
