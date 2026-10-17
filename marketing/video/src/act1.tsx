// Act 1 — the core story, on the real app: "Ask your papers." over a soft
// apricot ground as the app window rises; the question is typed (camera on
// the composer); the answer streams and its citations are ringed where the
// app draws them; "Every claim, cited."; citation 2 opens the viewer and the
// camera moves into the outlined passage, crisp from a full-resolution still.
// Footage: a 1440×810 take of the demo profile; graphics are placed from the
// element boxes the take logged (public/act1/boxes.json).
import React from "react";
import { AbsoluteFill, Easing, Html5Audio, Img, OffthreadVideo, Sequence, interpolate, staticFile, useCurrentFrame } from "remotion";
import boxes from "../public/act1/boxes.json";
import { barBeat, beatFrame } from "./beats";
import { font } from "./theme";

/** Pastel Shodh: apricot ground, dark warm text, the deep Shodh orange as the only accent. */
export const PASTEL = {
  ground: "radial-gradient(85% 75% at 50% 40%, #fff3ea 0%, #fddcc6 55%, #f7c3a1 100%)",
  text: "#2a1409",
  sub: "rgba(42,20,9,0.62)",
  accent: "#c94d1c",
  shadow: "0 60px 120px rgba(150,64,22,0.22), 0 18px 40px rgba(150,64,22,0.16)",
} as const;

const clamp = { extrapolateLeft: "clamp", extrapolateRight: "clamp" } as const;
const expo = Easing.bezier(0.16, 1, 0.3, 1);
const inOut = Easing.bezier(0.65, 0, 0.35, 1);
const outIn = Easing.bezier(0.7, 0, 0.84, 0);
const bar = (n: number) => beatFrame(barBeat(n));
const ramp = (f: number, a: number, b: number, e = expo) => interpolate(f, [a, b], [0, 1], { ...clamp, easing: e });
const mix = (a: number, b: number, t: number) => a + (b - a) * t;

type Box = { x: number; y: number; w: number; h: number };
type BoxName = "composer" | "answer" | "badge1" | "badge2" | "badge3" | "badge4" | "grounded" | "sources" | "viewer" | "passage";
const box = (name: BoxName): Box => {
  const b = (boxes as Record<string, { box: Box | null }>)[name]?.box;
  if (!b) throw new Error(`public/act1/boxes.json has no box for ${name}`);
  return b;
};
const centre = (b: Box) => ({ x: b.x + b.w / 2, y: b.y + b.h / 2 });

/** The app window's place in the frame: large, centred, 16:9 like the footage. */
const WIN = { x: 140, y: 79, w: 1640, h: 922.5 };

/**
 * The camera inside the window: `scale` about the window centre, shifted so
 * `focus` (footage fractions) comes toward the middle without uncovering an edge.
 */
function cameraTransform(focus: { x: number; y: number }, scale: number): string {
  const pan = (f: number, size: number) => {
    const spare = ((scale - 1) * size) / 2;
    return Math.max(-spare, Math.min(spare, (0.5 - f) * size * scale));
  };
  return `translate(${pan(focus.x, WIN.w)}px, ${pan(focus.y, WIN.h)}px) scale(${scale})`;
}

/** Words rising out of masks, one after another. */
const Headline: React.FC<{ words: string[]; at: number; out: number; size: number; x: number; y: number; maxWidth?: number }> = ({ words, at, out, size, x, y, maxWidth = 1500 }) => {
  const frame = useCurrentFrame();
  return (
    <div style={{ position: "absolute", left: x, top: y, maxWidth, display: "flex", flexWrap: "wrap", gap: `0 ${size * 0.24}px`, fontFamily: font.sans, fontWeight: 760, fontSize: size, lineHeight: 1.0, letterSpacing: "-0.045em" }}>
      {words.map((w, i) => {
        const inn = ramp(frame, at + i * 4, at + i * 4 + 18);
        const leave = ramp(frame, out + i * 2, out + i * 2 + 10, outIn);
        return (
          <span key={`${w}${i}`} style={{ display: "inline-block", overflow: "hidden", paddingBottom: "0.1em", marginBottom: "-0.1em" }}>
            <span style={{ display: "inline-block", transform: `translateY(${(1 - inn) * 110 - leave * 110}%)`, color: i === words.length - 1 ? PASTEL.accent : PASTEL.text }}>{w}</span>
          </span>
        );
      })}
    </div>
  );
};

/** A box drawn on over the footage, in window pixels from footage fractions. */
const Outline: React.FC<{ b: Box; p: number; pad: number; radius: number; width: number }> = ({ b, p, pad, radius, width }) => {
  const w = b.w * WIN.w + pad * 2;
  const h = b.h * WIN.h + pad * 2;
  return (
    <svg style={{ position: "absolute", left: b.x * WIN.w - pad, top: b.y * WIN.h - pad, overflow: "visible" }} width={w} height={h} aria-hidden="true">
      <rect x={0} y={0} width={w} height={h} rx={radius} fill="none" stroke={PASTEL.accent} strokeWidth={width} pathLength={1} strokeDasharray={1} strokeDashoffset={1 - p} />
    </svg>
  );
};

const fill: React.CSSProperties = { position: "absolute", inset: 0, width: "100%", height: "100%" };

export const Act1: React.FC = () => {
  const frame = useCurrentFrame();
  const B1 = bar(1);
  const B3 = bar(3);
  const B5 = bar(5);
  const B6 = bar(6);
  const B8 = bar(8);

  // The window rises in on a tilt under the first headline.
  const rise = ramp(frame, B1 - 30, B1 + 10, inOut);
  // It recedes behind the second headline, then makes room for the last one.
  const recede = ramp(frame, B5 - 2, B5 + 14, inOut) * (1 - ramp(frame, B6 - 8, B6 + 8, inOut));
  const aside = ramp(frame, B6 + 26, B6 + 52, inOut);

  // Camera inside the window, from the logged element boxes.
  const composer = centre(box("composer"));
  const badges = centre(box("badge2"));
  const passage = centre(box("passage"));
  let focus = { x: 0.5, y: 0.5 };
  let scale = 1;
  const column = { x: 0.56, y: 0.36 };
  if (frame < B3) {
    const t = ramp(frame, B1 + 4, B1 + 30, inOut);
    const back = ramp(frame, B3 - 22, B3 - 4, inOut);
    focus = { x: mix(mix(0.5, composer.x, t), column.x, back), y: mix(mix(0.5, composer.y, t), column.y, back) };
    scale = mix(mix(1, 1.6, t), 1.3, back);
  } else if (frame < B5) {
    const u = ramp(frame, B5 - 40, B5 - 6, inOut);
    focus = { x: mix(column.x, badges.x, u), y: mix(column.y, badges.y, u) };
    scale = mix(1.3, 2.2, u);
  } else if (frame < B6) {
    const t = ramp(frame, B5, B6 - 6, inOut);
    focus = { x: mix(badges.x, 0.5, t), y: mix(badges.y, 0.5, t) };
    scale = mix(2.2, 1, t);
  } else {
    const t = ramp(frame, B6 + 58, B8 + 10, inOut);
    focus = { x: mix(0.5, passage.x, t), y: mix(0.5, passage.y, t) };
    scale = mix(1, 2.1, t);
  }

  const toHold = ramp(frame, B6 + 50, B6 + 60);
  const outline = ramp(frame, B6 + 70, B6 + 88);
  const label = ramp(frame, B6 + 84, B6 + 100);

  const winScale = (1 - 0.12 * recede) * (1 - 0.3 * aside);
  const winX = WIN.x + aside * 560;
  const winY = mix(1120, WIN.y, rise) + aside * 60;
  const pass = box("passage");

  return (
    <AbsoluteFill style={{ background: PASTEL.ground, overflow: "hidden", perspective: 2200 }}>
      <div
        style={{
          position: "absolute",
          left: winX,
          top: winY,
          width: WIN.w,
          height: WIN.h,
          borderRadius: 20,
          overflow: "hidden",
          background: "#fff",
          boxShadow: PASTEL.shadow,
          transform: `rotateX(${(1 - rise) * 26}deg) scale(${winScale})`,
          transformOrigin: "0% 0%",
          filter: recede > 0.02 ? `blur(${recede * 8}px)` : undefined,
          opacity: 1 - 0.35 * recede,
        }}
      >
        <div style={{ position: "absolute", inset: 0, transformOrigin: "50% 50%", transform: cameraTransform(focus, scale) }}>
          <Sequence from={B1 - 16} durationInFrames={B3 - (B1 - 16)} layout="none">
            <OffthreadVideo src={staticFile("act1/type.mp4")} muted style={fill} />
          </Sequence>
          <Sequence from={B3} durationInFrames={B5 - B3} layout="none">
            <OffthreadVideo src={staticFile("act1/stream.mp4")} muted style={fill} />
          </Sequence>
          {frame >= B5 - 8 && frame < B6 ? <Img src={staticFile("act1/hold-answer.png")} style={{ ...fill, opacity: ramp(frame, B5 - 8, B5 - 2) }} /> : null}
          <Sequence from={B6} durationInFrames={B8 + 90 - B6} layout="none">
            <OffthreadVideo src={staticFile("act1/click.mp4")} muted style={fill} />
          </Sequence>
          {frame >= B6 + 50 ? <Img src={staticFile("act1/hold-viewer.png")} style={{ ...fill, opacity: toHold }} /> : null}
          {frame >= B5 - 34 && frame < B6
            ? (["badge1", "badge2", "badge3", "badge4"] as const).map((k, i) => (
                <Outline key={k} b={box(k)} p={ramp(frame, B5 - 34 + i * 3, B5 - 18 + i * 3)} pad={3} radius={5} width={2.5} />
              ))
            : null}
          {frame >= B6 + 70 ? <Outline b={pass} p={outline} pad={8} radius={8} width={3} /> : null}
          {frame >= B6 + 84 ? (
            <div
              style={{
                position: "absolute",
                left: pass.x * WIN.w,
                top: pass.y * WIN.h - 44,
                padding: "6px 14px",
                borderRadius: 999,
                background: PASTEL.accent,
                color: "#fff",
                fontFamily: font.sans,
                fontSize: 15,
                fontWeight: 700,
                opacity: label,
                transform: `translateY(${(1 - label) * 8}px)`,
              }}
            >
              Cited passage · SPANN, page 2
            </div>
          ) : null}
        </div>
      </div>

      <Headline words={["Ask", "your", "papers."]} at={6} out={B1 - 40} size={150} x={160} y={420} />
      <Headline words={["Every", "claim,", "cited."]} at={B5 + 2} out={B6 - 8} size={150} x={160} y={420} />
      <Headline words={["The", "exact", "page."]} at={B6 + 38} out={B8 + 80} size={112} x={120} y={200} maxWidth={640} />
      {frame >= B6 + 58 ? (
        <div style={{ position: "absolute", left: 124, top: 452, maxWidth: 440, fontFamily: font.sans, fontSize: 30, lineHeight: 1.4, fontWeight: 480, color: PASTEL.sub, opacity: ramp(frame, B6 + 58, B6 + 78) }}>
          One click on a citation opens the paper at the passage it came from.
        </div>
      ) : null}
      <Html5Audio src={staticFile("music/track.mp3")} />
    </AbsoluteFill>
  );
};
