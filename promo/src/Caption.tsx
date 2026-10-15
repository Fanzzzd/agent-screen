import React from "react";
import { Easing, interpolate, useCurrentFrame } from "remotion";
import { OUT, theme } from "./theme";

const ease = Easing.bezier(...OUT);

// A caption in a pill at the bottom, shown from `from` to `to` (frames).
export const Caption: React.FC<{ from: number; to: number; children: React.ReactNode; accent?: string }> = ({
  from,
  to,
  children,
  accent = theme.accent,
}) => {
  const frame = useCurrentFrame();
  const opacity = interpolate(frame, [from, from + 8, to - 6, to], [0, 1, 1, 0], {
    extrapolateLeft: "clamp",
    extrapolateRight: "clamp",
  });
  if (opacity <= 0) return null;
  return (
    <div
      style={{
        position: "absolute",
        bottom: 64,
        left: 0,
        right: 0,
        display: "flex",
        justifyContent: "center",
        opacity,
        translate: interpolate(frame, [from, from + 12], ["0px 20px", "0px 0px"], {
          extrapolateLeft: "clamp",
          extrapolateRight: "clamp",
          easing: ease,
        }),
      }}
    >
      <div
        style={{
          display: "flex",
          alignItems: "center",
          gap: 20,
          padding: "22px 40px",
          borderRadius: 999,
          background: "rgba(18,12,31,0.82)",
          backdropFilter: "blur(20px)",
          boxShadow: `0 20px 60px rgba(0,0,0,0.5), inset 0 0 0 1.5px rgba(255,255,255,0.12)`,
          color: theme.text,
          fontFamily: theme.font,
          fontSize: 48,
          fontWeight: 600,
        }}
      >
        <div style={{ width: 14, height: 14, borderRadius: 7, background: accent, boxShadow: `0 0 16px ${accent}` }} />
        {children}
      </div>
    </div>
  );
};
