import React from "react";
import { Video } from "@remotion/media";
import { staticFile } from "remotion";

// The recorded Viewer (1920 × 1312) in a window card, zoomed about
// `origin` by `zoom`.
export const Clip: React.FC<{
  trimBefore: number;
  zoom: number;
  origin: string;
  width?: number;
  children?: React.ReactNode;
}> = ({ trimBefore, zoom, origin, width = 1300, children }) => {
  const height = Math.round((width * 1312) / 1920);
  return (
    <div
      style={{
        position: "absolute",
        left: (1920 - width) / 2,
        top: 36,
        width,
        height,
        borderRadius: 22,
        overflow: "hidden",
        boxShadow: "0 50px 140px rgba(0,0,0,0.6), 0 0 0 1.5px rgba(255,255,255,0.10)",
      }}
    >
      <div style={{ width: "100%", height: "100%", scale: String(zoom), transformOrigin: origin }}>
        <Video src={staticFile("agent.mp4")} trimBefore={trimBefore} muted style={{ width: "100%", height: "100%" }} />
      </div>
      {children}
    </div>
  );
};
