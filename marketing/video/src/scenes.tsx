import React from "react";
import { AbsoluteFill, interpolate, useCurrentFrame, useVideoConfig } from "remotion";
import { Caption, FootageFrame, Logo, footageBox, isPortrait, useFade } from "./components";
import { CAPTIONS, type SceneId } from "./timeline";
import { calm, color, font } from "./theme";

const clamp = { extrapolateLeft: "clamp", extrapolateRight: "clamp" } as const;

// Real, public arXiv titles.
const PAPERS = [
  "RoboTTT: Context Scaling for Robot Policies",
  "KAN: Kolmogorov–Arnold Networks",
  "Parallelizing Linear Transformers with the Delta Rule over Sequence Length",
  "Unlocking State-Tracking in Linear RNNs Through Negative Eigenvalues",
  "Memory as a Markov Matrix",
];

/** 1 — paper titles drifting slowly in the dark. */
const PaperDrift: React.FC = () => {
  const frame = useCurrentFrame();
  const { width, height, durationInFrames } = useVideoConfig();
  const portrait = isPortrait(width, height);
  const progress = frame / durationInFrames;
  const near = PAPERS.map((title, i) => ({ title, row: i, near: true }));
  // A dimmer, blurred copy between the rows (the last one would meet the caption).
  const far = PAPERS.slice(0, 4).map((_, i) => ({ title: PAPERS[(i + 3) % PAPERS.length], row: i + 0.5, near: false }));
  return (
    <AbsoluteFill style={{ overflow: "hidden", opacity: useFade(24) }}>
      <AbsoluteFill style={{ transform: `scale(${1 + 0.05 * calm(progress)})` }}>
        {[...far, ...near].map(({ title, row, near: isNear }) => {
          const i = Math.floor(row);
          const top = (portrait ? 0.1 : 0.09) + row * (portrait ? 0.13 : 0.145);
          const direction = i % 2 === 0 ? 1 : -1;
          const drift = (isNear ? direction : -direction) * (isNear ? 60 : 30) * progress;
          const left = isNear ? (i % 2 === 0 ? 0.07 : 0.2) : i % 2 === 0 ? 0.42 : 0.12;
          return (
            <div
              key={`${isNear ? "n" : "f"}-${row}`}
              style={{
                position: "absolute",
                left: left * width + drift,
                top: top * height,
                maxWidth: width * (0.94 - left),
                fontFamily: font.sans,
                fontSize: isNear ? (portrait ? 44 : 44) - (i % 3) * 4 : 24,
                fontWeight: isNear ? 450 : 400,
                letterSpacing: "-0.015em",
                lineHeight: 1.2,
                color: color.dark.text,
                opacity: isNear ? 0.66 - (i % 3) * 0.13 : 0.14,
                filter: isNear ? undefined : "blur(1.5px)",
              }}
            >
              {title}
            </div>
          );
        })}
      </AbsoluteFill>
    </AbsoluteFill>
  );
};

/** 2 — a generic AI answer whose second citation turns out not to exist. */
const UnsureAnswer: React.FC = () => {
  const frame = useCurrentFrame();
  const { width, height, fps } = useVideoConfig();
  const portrait = isPortrait(width, height);
  const flickerStart = 2.2 * fps;
  const dissolveStart = 3.1 * fps;
  const flicker =
    frame < flickerStart || frame > dissolveStart
      ? 1
      : 0.55 + 0.45 * Math.cos(((frame - flickerStart) / fps) * Math.PI * 5);
  const gone = interpolate(frame, [dissolveStart, dissolveStart + 18], [0, 1], { ...clamp, easing: calm });
  const notFound = interpolate(frame, [dissolveStart + 10, dissolveStart + 30], [0, 1], { ...clamp, easing: calm });
  const cite = (n: number, extra?: React.CSSProperties) => (
    <span
      style={{
        display: "inline-block",
        fontSize: "0.62em",
        verticalAlign: "super",
        padding: "0 0.35em",
        marginLeft: 3,
        borderRadius: 6,
        background: color.dark.raised,
        color: color.dark.muted,
        ...extra,
      }}
    >
      {n}
    </span>
  );
  return (
    <AbsoluteFill
      style={{ justifyContent: "center", alignItems: "center", opacity: useFade(18), paddingBottom: height * 0.12 }}
    >
      <div style={{ width: portrait ? width * 0.86 : width * 0.52, fontFamily: font.sans }}>
        <div
          style={{
            marginLeft: "auto",
            width: "fit-content",
            maxWidth: "80%",
            padding: "18px 24px",
            borderRadius: 18,
            background: color.dark.raised,
            color: color.dark.text,
            fontSize: portrait ? 34 : 28,
            marginBottom: 34,
          }}
        >
          Does linear attention close the gap on long context?
        </div>
        <div
          style={{
            padding: "28px 32px",
            borderRadius: 18,
            border: `1px solid ${color.dark.border}`,
            background: color.dark.surface,
            color: color.dark.text,
            fontSize: portrait ? 34 : 29,
            lineHeight: 1.55,
          }}
        >
          Yes. Linear-time variants now match full attention on every long-context benchmark
          {cite(1)}, and the gap disappears entirely beyond a million tokens.
          <span style={{ position: "relative", display: "inline-block" }}>
            {cite(2, { opacity: flicker * (1 - gone), filter: `blur(${gone * 6}px)` })}
            <span
              style={{
                position: "absolute",
                left: 0,
                top: "-0.25em",
                whiteSpace: "nowrap",
                fontSize: "0.55em",
                padding: "2px 10px",
                borderRadius: 6,
                border: `1px dashed ${color.dark.faint}`,
                color: color.dark.muted,
                opacity: notFound,
              }}
            >
              source not found
            </span>
          </span>
        </div>
      </div>
    </AbsoluteFill>
  );
};

/** 3 — the mark, in the light. */
const Reveal: React.FC = () => {
  const frame = useCurrentFrame();
  const { width, height } = useVideoConfig();
  const portrait = isPortrait(width, height);
  const appear = interpolate(frame, [14, 50], [0, 1], { ...clamp, easing: calm });
  const word = interpolate(frame, [30, 64], [0, 1], { ...clamp, easing: calm });
  return (
    <AbsoluteFill
      style={{
        justifyContent: "center",
        alignItems: "center",
        flexDirection: "column",
        paddingBottom: height * 0.1,
        opacity: useFade(1),
      }}
    >
      <div style={{ opacity: appear, transform: `scale(${0.96 + 0.04 * appear})` }}>
        <Logo size={portrait ? 300 : 240} />
      </div>
      <div
        style={{
          marginTop: 18,
          opacity: word,
          display: "flex",
          alignItems: "baseline",
          gap: 26,
          color: color.light.text,
        }}
      >
        <span style={{ fontFamily: font.sans, fontSize: portrait ? 76 : 64, fontWeight: 550, letterSpacing: "-0.03em" }}>
          Shodh
        </span>
        <span style={{ fontFamily: font.devanagari, fontSize: portrait ? 64 : 54, fontWeight: 500, color: color.accent }}>
          शोध
        </span>
      </div>
    </AbsoluteFill>
  );
};

/** 4–8 — footage only. */
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

/** Scenes 1 and 2 sit on the dark ground; scene 3 onwards on the light. */
export const toneOf = (scene: SceneId): "dark" | "light" => (scene <= 2 ? "dark" : "light");

export const Scene: React.FC<{ scene: SceneId }> = ({ scene }) => {
  const body = (() => {
    switch (scene) {
      case 1:
        return <PaperDrift />;
      case 2:
        return <UnsureAnswer />;
      case 3:
        return <Reveal />;
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
      {/* Scene 3's caption waits for the ground to turn light. */}
      <Caption text={CAPTIONS[scene]} tone={toneOf(scene)} delay={scene === 3 ? 34 : 8} />
    </AbsoluteFill>
  );
};
