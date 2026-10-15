import React from "react";
import { AbsoluteFill, Easing, Img, interpolate, staticFile, useCurrentFrame, useVideoConfig } from "remotion";
import { copy, Lang } from "../copy";
import { OUT, theme } from "../theme";

const ease = Easing.bezier(...OUT);
const clamp = { extrapolateLeft: "clamp", extrapolateRight: "clamp" } as const;

const Card: React.FC<{
  n: number;
  title: string;
  sub: string;
  delay: number;
  children: React.ReactNode;
}> = ({ n, title, sub, delay, children }) => {
  const frame = useCurrentFrame();
  return (
    <div
      style={{
        width: 520,
        height: 640,
        borderRadius: 32,
        background: "linear-gradient(170deg, rgba(255,255,255,0.08), rgba(255,255,255,0.03))",
        boxShadow: "inset 0 0 0 1.5px rgba(255,255,255,0.10), 0 30px 80px rgba(0,0,0,0.4)",
        padding: 40,
        display: "flex",
        flexDirection: "column",
        gap: 18,
        opacity: interpolate(frame, [delay, delay + 10], [0, 1], clamp),
        translate: interpolate(frame, [delay, delay + 16], ["0px 60px", "0px 0px"], { ...clamp, easing: ease }),
      }}
    >
      <div
        style={{
          width: 64,
          height: 64,
          borderRadius: 32,
          background: theme.accent,
          color: "white",
          fontSize: 36,
          fontWeight: 800,
          display: "flex",
          alignItems: "center",
          justifyContent: "center",
          boxShadow: `0 0 30px ${theme.accent}88`,
        }}
      >
        {n}
      </div>
      <div style={{ color: theme.text, fontSize: 50, fontWeight: 700 }}>{title}</div>
      <div style={{ color: theme.dim, fontSize: 32, fontWeight: 500, lineHeight: 1.3 }}>{sub}</div>
      <div style={{ flex: 1, display: "flex", alignItems: "center", justifyContent: "center" }}>{children}</div>
    </div>
  );
};

export const Steps: React.FC<{ lang: Lang }> = ({ lang }) => {
  const { fps } = useVideoConfig();
  const t = copy[lang];
  return (
    <AbsoluteFill
      style={{
        background: theme.bg,
        fontFamily: theme.font,
        alignItems: "center",
        justifyContent: "center",
        flexDirection: "row",
        gap: 50,
      }}
    >
      <Card n={1} title={t.step1} sub={t.step1Sub} delay={0}>
        <Img src={staticFile("setup.png")} style={{ width: 300, borderRadius: 14 }} />
      </Card>
      <Card n={2} title={t.step2} sub={t.step2Sub} delay={0.35 * fps}>
        <div
          style={{
            padding: "22px 48px",
            borderRadius: 999,
            background: theme.accent,
            color: "white",
            fontSize: 40,
            fontWeight: 700,
            boxShadow: `0 0 40px ${theme.accent}77`,
          }}
        >
          {lang === "zh" ? "接入…" : "Connect…"}
        </div>
      </Card>
      <Card n={3} title={t.step3} sub={t.step3Sub} delay={0.7 * fps}>
        <div
          style={{
            fontFamily: theme.mono,
            fontSize: 30,
            color: theme.accentSoft,
            background: "rgba(0,0,0,0.35)",
            padding: "20px 24px",
            borderRadius: 16,
          }}
        >
          create_display ✓
        </div>
      </Card>
    </AbsoluteFill>
  );
};
