import React from "react";
import { AbsoluteFill, Easing, Img, interpolate, staticFile, useCurrentFrame, useVideoConfig } from "remotion";
import { copy, Lang } from "../copy";
import { OUT, theme } from "../theme";

const ease = Easing.bezier(...OUT);
const clamp = { extrapolateLeft: "clamp", extrapolateRight: "clamp" } as const;

export const End: React.FC<{ lang: Lang }> = ({ lang }) => {
  const frame = useCurrentFrame();
  const { fps } = useVideoConfig();
  const t = copy[lang];
  return (
    <AbsoluteFill
      style={{
        background: `radial-gradient(circle at 50% 45%, #1d3b7a 0%, ${theme.bg} 55%)`,
        fontFamily: theme.font,
        alignItems: "center",
        justifyContent: "center",
        gap: 30,
      }}
    >
      <div style={{ display: "flex", alignItems: "center", gap: 36 }}>
        <Img src={staticFile("icon.png")} style={{ width: 160, height: 160 }} />
        <div style={{ color: theme.text, fontSize: 120, fontWeight: 800, letterSpacing: -3 }}>Sidescreen</div>
      </div>
      <div
        style={{
          color: theme.dim,
          fontSize: 48,
          fontWeight: 500,
          opacity: interpolate(frame, [0.3 * fps, 0.7 * fps], [0, 1], { ...clamp, easing: ease }),
        }}
      >
        {t.end}
      </div>
      <div
        style={{
          marginTop: 20,
          fontFamily: theme.mono,
          fontSize: 46,
          color: theme.text,
          padding: "20px 40px",
          borderRadius: 18,
          background: "rgba(255,255,255,0.07)",
          boxShadow: "inset 0 0 0 1.5px rgba(255,255,255,0.12)",
          opacity: interpolate(frame, [0.6 * fps, 1.0 * fps], [0, 1], { ...clamp, easing: ease }),
        }}
      >
        github.com/Fanzzzd/sidescreen
      </div>
    </AbsoluteFill>
  );
};
