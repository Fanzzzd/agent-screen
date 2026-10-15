import React from "react";
import { theme } from "./theme";

// The Agent Pointer's shape, as the app draws it: a rounded arrow with a
// white rim, a shadow and a glow in the accent color.
export const AgentPointer: React.FC<{ size?: number; style?: React.CSSProperties }> = ({
  size = 56,
  style,
}) => (
  <svg
    width={size}
    height={size}
    viewBox="-4 -4 34 34"
    style={{
      overflow: "visible",
      filter: `drop-shadow(0 3px 4px rgba(0,0,0,0.35)) drop-shadow(0 0 8px ${theme.accent}aa)`,
      ...style,
    }}
  >
    <defs>
      <linearGradient id="ap" x1="0" y1="0" x2="1" y2="1">
        <stop offset="0.1" stopColor="#0870e0" />
        <stop offset="0.95" stopColor="#dfe8ff" />
      </linearGradient>
    </defs>
    <path
      d="M1.2 0.6 Q0 0 0.3 1.3 L9.4 22.6 Q10.4 25.6 11.6 22.8 L13.4 13.9 Q13.6 12.3 15.1 12 L24.2 9.4 Q27.3 8.3 24.4 7.0 Z"
      fill="url(#ap)"
      stroke="white"
      strokeWidth={1.9}
      strokeLinejoin="round"
    />
  </svg>
);

// The macOS pointer, standing for the user's.
export const UserPointer: React.FC<{ size?: number; style?: React.CSSProperties }> = ({
  size = 40,
  style,
}) => (
  <svg width={size} height={size * 1.5} viewBox="0 0 20 30" style={{ overflow: "visible", ...style }}>
    <path
      d="M1 1 L1 23 L6.5 17.8 L10.2 26.5 L13.6 25 L9.9 16.5 L17.2 16.5 Z"
      fill="black"
      stroke="white"
      strokeWidth={1.6}
      strokeLinejoin="round"
    />
  </svg>
);
