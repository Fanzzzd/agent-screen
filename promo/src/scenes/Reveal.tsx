import React from "react";
import { AbsoluteFill, Easing, Img, interpolate, staticFile, useCurrentFrame, useVideoConfig } from "remotion";
import { copy, Lang } from "../copy";
import { OUT, theme } from "../theme";

const ease = Easing.bezier(...OUT);
const clamp = { extrapolateLeft: "clamp", extrapolateRight: "clamp" } as const;

export const Reveal: React.FC<{ lang: Lang }> = ({ lang }) => {
  const frame = useCurrentFrame();
  const { fps } = useVideoConfig();
  const t = copy[lang];
  return (
    <AbsoluteFill
      style={{
        background: `radial-gradient(circle at 50% 42%, #1d3b7a 0%, ${theme.bg} 55%)`,
        fontFamily: theme.font,
        alignItems: "center",
        justifyContent: "center",
        gap: 28,
      }}
    >
      <Img
        src={staticFile("icon.png")}
        style={{
          width: 220,
          height: 220,
          scale: interpolate(frame, [0, 0.6 * fps], [0.6, 1], {
            ...clamp,
            easing: Easing.spring({ damping: 12 }),
            output: "perceptual-scale",
          }),
          opacity: interpolate(frame, [0, 0.25 * fps], [0, 1], clamp),
        }}
      />
      <div
        style={{
          color: theme.text,
          fontSize: 132,
          fontWeight: 800,
          letterSpacing: -3,
          opacity: interpolate(frame, [0.2 * fps, 0.6 * fps], [0, 1], { ...clamp, easing: ease }),
          translate: interpolate(frame, [0.2 * fps, 0.6 * fps], ["0px 24px", "0px 0px"], { ...clamp, easing: ease }),
        }}
      >
        {t.reveal}
      </div>
      <div
        style={{
          color: theme.dim,
          fontSize: 54,
          fontWeight: 500,
          opacity: interpolate(frame, [0.6 * fps, 1.0 * fps], [0, 1], { ...clamp, easing: ease }),
        }}
      >
        {t.revealSub}
      </div>
    </AbsoluteFill>
  );
};
