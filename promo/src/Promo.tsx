import React from "react";
import { linearTiming, TransitionSeries } from "@remotion/transitions";
import { fade } from "@remotion/transitions/fade";
import { Lang } from "./copy";
import { End } from "./scenes/End";
import { Help } from "./scenes/Help";
import { Hook } from "./scenes/Hook";
import { Live } from "./scenes/Live";
import { Reveal } from "./scenes/Reveal";
import { Stats } from "./scenes/Stats";
import { Steps } from "./scenes/Steps";

export const FADE = 10;

export const Promo: React.FC<{ lang: Lang }> = ({ lang }) => {
  return (
    <TransitionSeries>
      <TransitionSeries.Sequence name="Hook" durationInFrames={105}>
        <Hook lang={lang} />
      </TransitionSeries.Sequence>
      <TransitionSeries.Transition presentation={fade()} timing={linearTiming({ durationInFrames: FADE })} />
      <TransitionSeries.Sequence name="Reveal" durationInFrames={75}>
        <Reveal lang={lang} />
      </TransitionSeries.Sequence>
      <TransitionSeries.Transition presentation={fade()} timing={linearTiming({ durationInFrames: FADE })} />
      <TransitionSeries.Sequence name="Live" durationInFrames={288}>
        <Live lang={lang} />
      </TransitionSeries.Sequence>
      <TransitionSeries.Transition presentation={fade()} timing={linearTiming({ durationInFrames: FADE })} />
      <TransitionSeries.Sequence name="Help" durationInFrames={120}>
        <Help lang={lang} />
      </TransitionSeries.Sequence>
      <TransitionSeries.Transition presentation={fade()} timing={linearTiming({ durationInFrames: FADE })} />
      <TransitionSeries.Sequence name="Steps" durationInFrames={135}>
        <Steps lang={lang} />
      </TransitionSeries.Sequence>
      <TransitionSeries.Transition presentation={fade()} timing={linearTiming({ durationInFrames: FADE })} />
      <TransitionSeries.Sequence name="Stats" durationInFrames={120}>
        <Stats lang={lang} />
      </TransitionSeries.Sequence>
      <TransitionSeries.Transition presentation={fade()} timing={linearTiming({ durationInFrames: FADE })} />
      <TransitionSeries.Sequence name="End" durationInFrames={105}>
        <End lang={lang} />
      </TransitionSeries.Sequence>
    </TransitionSeries>
  );
};

// 105 + 75 + 288 + 120 + 135 + 120 + 105, less six fades.
export const PROMO_FRAMES = 948 - 6 * FADE;
