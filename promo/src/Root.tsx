import React from "react";
import { Composition, Folder } from "remotion";
import { Promo, PROMO_FRAMES } from "./Promo";
import { End } from "./scenes/End";
import { Help } from "./scenes/Help";
import { Hook } from "./scenes/Hook";
import { Live } from "./scenes/Live";
import { Reveal } from "./scenes/Reveal";
import { Stats } from "./scenes/Stats";
import { Steps } from "./scenes/Steps";

const size = { width: 1920, height: 1080, fps: 30 } as const;

export const RemotionRoot: React.FC = () => (
  <>
    <Composition id="PromoEN" component={Promo} durationInFrames={PROMO_FRAMES} {...size} defaultProps={{ lang: "en" as const }} />
    <Composition id="PromoZH" component={Promo} durationInFrames={PROMO_FRAMES} {...size} defaultProps={{ lang: "zh" as const }} />
    <Folder name="Scenes">
      <Composition id="Hook" component={Hook} durationInFrames={105} {...size} defaultProps={{ lang: "en" as const }} />
      <Composition id="Reveal" component={Reveal} durationInFrames={75} {...size} defaultProps={{ lang: "en" as const }} />
      <Composition id="Live" component={Live} durationInFrames={288} {...size} defaultProps={{ lang: "en" as const }} />
      <Composition id="Help" component={Help} durationInFrames={120} {...size} defaultProps={{ lang: "en" as const }} />
      <Composition id="Steps" component={Steps} durationInFrames={135} {...size} defaultProps={{ lang: "en" as const }} />
      <Composition id="Stats" component={Stats} durationInFrames={120} {...size} defaultProps={{ lang: "en" as const }} />
      <Composition id="End" component={End} durationInFrames={105} {...size} defaultProps={{ lang: "en" as const }} />
    </Folder>
  </>
);
