import React from "react";
import { AbsoluteFill, Easing, interpolate, useCurrentFrame, useVideoConfig } from "remotion";
import { Caption } from "../Caption";
import { Clip } from "../Clip";
import { copy, Lang } from "../copy";
import { UserPointer } from "../Pointers";
import { OUT, theme } from "../theme";

const ease = Easing.bezier(...OUT);

// The agent fills a checkout in Chrome in the Viewer; the user's pointer rests.
export const Live: React.FC<{ lang: Lang }> = ({ lang }) => {
  const frame = useCurrentFrame();
  const { fps } = useVideoConfig();
  const t = copy[lang];
  return (
    <AbsoluteFill style={{ background: `radial-gradient(circle at 50% 30%, #23194a, ${theme.bg} 70%)` }}>
      <Clip
        trimBefore={1 * fps}
        zoom={interpolate(frame, [0.8 * fps, 2 * fps], [1, 1.4], {
          extrapolateLeft: "clamp",
          extrapolateRight: "clamp",
          easing: ease,
        })}
        origin="38% 52%"
      />
      {/* The user's pointer, still, beside the agent's display. */}
      <div
        style={{
          position: "absolute",
          right: 130,
          top: 470,
          display: "flex",
          flexDirection: "column",
          alignItems: "center",
          gap: 10,
          opacity: interpolate(frame, [3 * fps, 3.4 * fps], [0, 1], { extrapolateLeft: "clamp", extrapolateRight: "clamp" }),
        }}
      >
        <UserPointer size={44} />
        <div style={{ color: theme.dim, fontFamily: theme.font, fontSize: 28, fontWeight: 600 }}>
          {lang === "zh" ? "你" : "You"}
        </div>
      </div>
      <Caption from={0.4 * fps} to={3 * fps}>{t.live1}</Caption>
      <Caption from={3 * fps} to={6 * fps}>{t.live2}</Caption>
      <Caption from={6 * fps} to={9.6 * fps}>{t.live3}</Caption>
    </AbsoluteFill>
  );
};
