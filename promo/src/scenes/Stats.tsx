import React from "react";
import { AbsoluteFill, Easing, interpolate, useCurrentFrame, useVideoConfig } from "remotion";
import { copy, Lang } from "../copy";
import { OUT, theme } from "../theme";

const ease = Easing.bezier(...OUT);
const clamp = { extrapolateLeft: "clamp", extrapolateRight: "clamp" } as const;

const Stat: React.FC<{ value: string; label: string; delay: number }> = ({ value, label, delay }) => {
  const frame = useCurrentFrame();
  return (
    <div
      style={{
        display: "flex",
        alignItems: "baseline",
        gap: 36,
        opacity: interpolate(frame, [delay, delay + 10], [0, 1], clamp),
        translate: interpolate(frame, [delay, delay + 16], ["-60px 0px", "0px 0px"], { ...clamp, easing: ease }),
      }}
    >
      <div
        style={{
          width: 860,
          whiteSpace: "nowrap",
          textAlign: "right",
          fontSize: 110,
          fontWeight: 800,
          letterSpacing: -3,
          background: `linear-gradient(90deg, ${theme.accentSoft}, ${theme.accent})`,
          WebkitBackgroundClip: "text",
          color: "transparent",
        }}
      >
        {value}
      </div>
      <div style={{ color: theme.text, fontSize: 50, fontWeight: 600, width: 820 }}>{label}</div>
    </div>
  );
};

export const Stats: React.FC<{ lang: Lang }> = ({ lang }) => {
  const { fps } = useVideoConfig();
  const t = copy[lang];
  return (
    <AbsoluteFill
      style={{ background: theme.bg, fontFamily: theme.font, justifyContent: "center", gap: 70, paddingLeft: 60 }}
    >
      <Stat value={t.stat1} label={t.stat1Sub} delay={0} />
      <Stat value={t.stat2} label={t.stat2Sub} delay={0.4 * fps} />
      <Stat value={t.stat3} label={t.stat3Sub} delay={0.8 * fps} />
    </AbsoluteFill>
  );
};
