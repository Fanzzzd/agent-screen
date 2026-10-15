import React from "react";
import { AbsoluteFill, Easing, interpolate, useCurrentFrame, useVideoConfig } from "remotion";
import { copy, Lang } from "../copy";
import { AgentPointer, UserPointer } from "../Pointers";
import { OUT, theme } from "../theme";

const ease = Easing.bezier(...OUT);
const clamp = { extrapolateLeft: "clamp", extrapolateRight: "clamp" } as const;

// A screen where the user works; an agent's pointer arrives and drags the
// user's pointer away.
export const Hook: React.FC<{ lang: Lang }> = ({ lang }) => {
  const frame = useCurrentFrame();
  const { fps } = useVideoConfig();
  const t = copy[lang];
  const grab = 1.4 * fps;
  return (
    <AbsoluteFill style={{ backgroundColor: theme.bg, fontFamily: theme.font, alignItems: "center" }}>
      <div
        style={{
          position: "absolute",
          top: 120,
          color: theme.text,
          fontSize: 96,
          fontWeight: 700,
          letterSpacing: -2,
          opacity: interpolate(frame, [0.1 * fps, 0.6 * fps], [0, 1], { ...clamp, easing: ease }),
          translate: interpolate(frame, [0.1 * fps, 0.6 * fps], ["0px 30px", "0px 0px"], { ...clamp, easing: ease }),
        }}
      >
        {t.hook1}
      </div>
      <div
        style={{
          position: "absolute",
          top: 260,
          color: theme.dim,
          fontSize: 52,
          fontWeight: 500,
          opacity: interpolate(frame, [grab + 0.2 * fps, grab + 0.7 * fps], [0, 1], { ...clamp, easing: ease }),
        }}
      >
        {t.hook2}
      </div>
      {/* The user's screen. */}
      <div
        style={{
          position: "absolute",
          top: 390,
          width: 1300,
          height: 600,
          borderRadius: 28,
          background: `linear-gradient(160deg, ${theme.bgLift}, #2a1f45)`,
          boxShadow: "0 40px 120px rgba(0,0,0,0.5), inset 0 0 0 2px rgba(255,255,255,0.08)",
          overflow: "hidden",
          opacity: interpolate(frame, [0, 0.4 * fps], [0, 1], clamp),
        }}
      >
        {[0, 1, 2].map((i) => (
          <div
            key={i}
            style={{
              position: "absolute",
              left: 80,
              top: 90 + i * 70,
              height: 26,
              width: [620, 760, 480][i],
              borderRadius: 13,
              background: "rgba(255,255,255,0.10)",
            }}
          />
        ))}
        <UserPointer
          style={{
            position: "absolute",
            left: interpolate(frame, [0, grab, grab + 0.35 * fps, 3 * fps], [520, 560, 960, 1000], {
              ...clamp,
              easing: [Easing.inOut(Easing.sin), ease, Easing.linear],
            }),
            top: interpolate(frame, [0, grab, grab + 0.35 * fps, 3 * fps], [330, 300, 160, 150], {
              ...clamp,
              easing: [Easing.inOut(Easing.sin), ease, Easing.linear],
            }),
            rotate: interpolate(frame, [grab, grab + 0.15 * fps, grab + 0.5 * fps], ["0deg", "-14deg", "0deg"], clamp),
          }}
        />
        <AgentPointer
          size={64}
          style={{
            position: "absolute",
            left: interpolate(frame, [0.6 * fps, grab, grab + 0.35 * fps, 3 * fps], [1200, 575, 975, 1015], {
              ...clamp,
              easing: [ease, ease, Easing.linear],
            }),
            top: interpolate(frame, [0.6 * fps, grab, grab + 0.35 * fps, 3 * fps], [620, 290, 150, 140], {
              ...clamp,
              easing: [ease, ease, Easing.linear],
            }),
            rotate: interpolate(frame, [0.6 * fps, 1.0 * fps, grab, grab + 0.2 * fps, grab + 0.5 * fps], ["0deg", "-16deg", "0deg", "12deg", "0deg"], clamp),
          }}
        />
      </div>
    </AbsoluteFill>
  );
};
