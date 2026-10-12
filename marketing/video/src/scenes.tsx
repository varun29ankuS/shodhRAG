import React from "react";
import { AbsoluteFill, interpolate, useCurrentFrame, useVideoConfig } from "remotion";
import { Caption, FootageFrame, Logo, footageBox, isPortrait, useFade } from "./components";
import { CAPTIONS, type SceneId } from "./timeline";
import { calm, color, font } from "./theme";

const clamp = { extrapolateLeft: "clamp", extrapolateRight: "clamp" } as const;

/** 1–8 — footage of the app only. */
const FootageScene: React.FC<{ scene: SceneId }> = ({ scene }) => {
  const { width, height } = useVideoConfig();
  return <FootageFrame scene={scene} box={footageBox(width, height)} />;
};

/** Line icons for scene 9. */
const stroke = { fill: "none", stroke: color.light.text, strokeWidth: 2.2, strokeLinecap: "round", strokeLinejoin: "round" } as const;

const LaptopIcon: React.FC<{ size: number }> = ({ size }) => (
  <svg width={size} height={size} viewBox="0 0 64 64">
    <rect x="12" y="14" width="40" height="27" rx="3" {...stroke} />
    <path d="M6 48h52l-4 5H10z" {...stroke} />
    <path d="M20 23h18M20 29h24M20 35h14" {...stroke} stroke={color.light.faint} />
  </svg>
);

const ModelIcon: React.FC<{ size: number }> = ({ size }) => (
  <svg width={size} height={size} viewBox="0 0 64 64">
    <rect x="16" y="16" width="32" height="32" rx="6" {...stroke} />
    <rect x="25" y="25" width="14" height="14" rx="2" {...stroke} stroke={color.accent} />
    <path d="M24 10v6M32 10v6M40 10v6M24 48v6M32 48v6M40 48v6M10 24h6M10 32h6M10 40h6M48 24h6M48 32h6M48 40h6" {...stroke} />
  </svg>
);

/** 9 — Settings → Model footage beside the data path: laptop → passages → model. */
const LocalFirst: React.FC = () => {
  const frame = useCurrentFrame();
  const { width, height, fps } = useVideoConfig();
  const portrait = isPortrait(width, height);
  const graphic = interpolate(frame, [0.8 * fps, 2 * fps], [0, 1], { ...clamp, easing: calm });
  const box = portrait
    ? { left: width * 0.05, top: height * 0.06, w: width * 0.9, h: (width * 0.9 * 9) / 16 }
    : { left: width * 0.06, top: height * 0.1, w: width * 0.52, h: (width * 0.52 * 9) / 16 };
  const area = portrait
    ? { left: width * 0.05, top: box.top + box.h + height * 0.05, w: width * 0.9, h: height * 0.3 }
    : { left: width * 0.62, top: height * 0.1, w: width * 0.32, h: box.h };
  const icon = portrait ? 130 : 110;
  const label: React.CSSProperties = {
    fontFamily: font.sans,
    fontSize: portrait ? 26 : 21,
    color: color.light.muted,
    marginTop: 10,
    textAlign: "center",
    whiteSpace: "nowrap",
  };
  // The path runs along the area's long axis.
  const vertical = !portrait;
  const chips = [0, 1, 2].map((i) => {
    const t = ((frame - 1.6 * fps - i * 0.5 * fps) / (2.4 * fps)) % 1;
    return frame < 1.6 * fps + i * 0.5 * fps ? -1 : t;
  });
  const pathLength = vertical ? area.h - 2 * (icon + 50) : area.w - 2 * (icon + 50);
  return (
    <AbsoluteFill>
      <FootageFrame scene={9} box={box} />
      <div
        style={{
          position: "absolute",
          left: area.left,
          top: area.top,
          width: area.w,
          height: area.h,
          display: "flex",
          flexDirection: vertical ? "column" : "row",
          alignItems: "center",
          justifyContent: "space-between",
          opacity: graphic * useFade(18),
        }}
      >
        <div style={{ display: "flex", flexDirection: "column", alignItems: "center" }}>
          <LaptopIcon size={icon} />
          <div style={label}>Your library</div>
        </div>
        <div
          style={{
            position: "relative",
            [vertical ? "height" : "width"]: pathLength,
            [vertical ? "width" : "height"]: 2,
            borderLeft: vertical ? `2px dashed ${color.light.borderStrong}` : undefined,
            borderTop: vertical ? undefined : `2px dashed ${color.light.borderStrong}`,
          }}
        >
          {chips.map((t, i) =>
            t < 0 ? null : (
              <div
                key={i}
                style={{
                  position: "absolute",
                  [vertical ? "top" : "left"]: `${calm(t) * 100}%`,
                  [vertical ? "left" : "top"]: -9,
                  transform: vertical ? "translate(-50%, -50%)" : "translate(-50%, -50%)",
                  width: 44,
                  height: 16,
                  borderRadius: 5,
                  background: color.accentSoft,
                  border: `1.5px solid ${color.accent}`,
                  opacity: Math.sin(Math.PI * t),
                }}
              />
            ),
          )}
          <div
            style={{
              ...label,
              position: "absolute",
              whiteSpace: "nowrap",
              marginTop: 0,
              color: color.accent,
              [vertical ? "left" : "top"]: 26,
              [vertical ? "top" : "left"]: "50%",
              transform: vertical ? "translateY(-50%)" : "translateX(-50%)",
            }}
          >
            passages only
          </div>
        </div>
        <div style={{ display: "flex", flexDirection: "column", alignItems: "center" }}>
          <ModelIcon size={icon} />
          <div style={label}>The model you choose</div>
        </div>
      </div>
    </AbsoluteFill>
  );
};

/** 10 — close. */
const Close: React.FC = () => {
  const frame = useCurrentFrame();
  const { width, height } = useVideoConfig();
  const portrait = isPortrait(width, height);
  const line = (delay: number) => interpolate(frame, [delay, delay + 24], [0, 1], { ...clamp, easing: calm });
  return (
    <AbsoluteFill
      style={{
        justifyContent: "center",
        alignItems: "center",
        flexDirection: "column",
        paddingBottom: height * 0.12,
        fontFamily: font.sans,
        color: color.light.text,
        opacity: useFade(18),
      }}
    >
      <div style={{ opacity: line(0) }}>
        <Logo size={portrait ? 220 : 170} />
      </div>
      <div style={{ opacity: line(10), fontSize: portrait ? 56 : 48, fontWeight: 550, letterSpacing: "-0.025em", marginTop: 18 }}>
        Free and open source
      </div>
      <div style={{ opacity: line(20), fontSize: portrait ? 32 : 28, color: color.light.muted, marginTop: 14 }}>
        Download for Windows and Mac
      </div>
      <div
        style={{
          opacity: line(30),
          fontSize: portrait ? 28 : 24,
          color: color.accent,
          marginTop: 26,
          padding: "10px 22px",
          borderRadius: 999,
          background: color.accentSoft,
        }}
      >
        github.com/varun29ankuS/shodhRAG
      </div>
    </AbsoluteFill>
  );
};

export const Scene: React.FC<{ scene: SceneId }> = ({ scene }) => {
  const body = (() => {
    switch (scene) {
      case 9:
        return <LocalFirst />;
      case 10:
        return <Close />;
      default:
        return <FootageScene scene={scene} />;
    }
  })();
  return (
    <AbsoluteFill>
      {body}
      <Caption text={CAPTIONS[scene]} />
    </AbsoluteFill>
  );
};
