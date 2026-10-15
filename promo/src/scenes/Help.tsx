import React from "react";
import { Video } from "@remotion/media";
import { AbsoluteFill, Easing, interpolate, staticFile, useCurrentFrame, useVideoConfig } from "remotion";
import { Caption } from "../Caption";
import { Clip } from "../Clip";
import { copy, Lang } from "../copy";
import { OUT, theme } from "../theme";

const ease = Easing.bezier(...OUT);

// The agent asks for help: the Viewer's header turns orange.
export const Help: React.FC<{ lang: Lang }> = ({ lang }) => {
  const frame = useCurrentFrame();
  const { fps } = useVideoConfig();
  const t = copy[lang];
  return (
    <AbsoluteFill style={{ background: `radial-gradient(circle at 50% 20%, #3a2410, ${theme.bg} 70%)` }}>
      <Clip
        trimBefore={10.2 * fps}
        zoom={1}
        origin="50% 0%"
      >
        {/* The Viewer's header, which turns orange when the agent asks. */}
        <div
          style={{
            position: "absolute",
            left: 0,
            right: 0,
            top: 0,
            height: 44,
            borderRadius: "22px 22px 0 0",
            boxShadow: `inset 0 0 0 3px ${theme.warm}, 0 0 50px ${theme.warm}`,
            opacity: interpolate(frame, [0.6 * fps, 0.9 * fps, 3.6 * fps, 4 * fps], [0, 1, 1, 0], {
              extrapolateLeft: "clamp",
              extrapolateRight: "clamp",
              easing: ease,
            }),
          }}
        />
      </Clip>
      {/* The header again, large enough to read: the request and Take Over. */}
      <div
        style={{
          position: "absolute",
          left: 180,
          top: 330,
          width: 1560,
          height: 112,
          borderRadius: 20,
          overflow: "hidden",
          boxShadow: `0 30px 90px rgba(0,0,0,0.7), 0 0 0 3px ${theme.warm}, 0 0 60px ${theme.warm}88`,
          opacity: interpolate(frame, [0.8 * fps, 1.1 * fps], [0, 1], { extrapolateLeft: "clamp", extrapolateRight: "clamp" }),
          scale: interpolate(frame, [0.8 * fps, 1.3 * fps], [0.92, 1], {
            extrapolateLeft: "clamp",
            extrapolateRight: "clamp",
            easing: ease,
            output: "perceptual-scale",
          }),
        }}
      >
        <Video
          src={staticFile("agent.mp4")}
          trimBefore={10.2 * fps}
          muted
          style={{ position: "absolute", left: 0, top: 0, width: 2600, height: Math.round((2600 * 1312) / 1920) }}
        />
      </div>
      <Caption from={0.3 * fps} to={2 * fps} accent={theme.warm}>{t.help1}</Caption>
      <Caption from={2 * fps} to={4 * fps} accent={theme.warm}>{t.help2}</Caption>
    </AbsoluteFill>
  );
};
