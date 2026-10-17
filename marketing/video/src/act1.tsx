// Act 1 — one unhurried walkthrough of the core flow, on the real app.
// It opens on the Shodh logo, name and line, which then settle into the
// brand mark top left as the app window rises. One window, one continuous
// camera that frames each part of the app in turn (the composer, the
// conversation, the answer, the citations, the passage) with even margins;
// a numbered caption under the window says what is happening at each step.
// Every step lasts three or four bars of the track.
// Footage: a 1440×810 take of the demo profile; framing and graphics come
// from the element boxes logged during the take (public/act1/boxes.json).
import React from "react";
import { AbsoluteFill, Easing, Html5Audio, Img, OffthreadVideo, Sequence, interpolate, staticFile, useCurrentFrame } from "remotion";
import boxes from "../public/act1/boxes.json";
import { barBeat, beatFrame } from "./beats";
import { font } from "./theme";

/** A look for the act: ground, type and the one accent (the deep Shodh orange family). */
export type Look = { ground: string; text: string; sub: string; accent: string; shadow: string };

/** Pastel directions under review; `apricot` is the current one. */
export const LOOKS = {
  apricot: {
    ground: "radial-gradient(85% 75% at 50% 40%, #fff3ea 0%, #fddcc6 55%, #f7c3a1 100%)",
    text: "#2a1409",
    sub: "rgba(42,20,9,0.66)",
    accent: "#c94d1c",
    shadow: "0 50px 110px rgba(150,64,22,0.20), 0 14px 34px rgba(150,64,22,0.14)",
  },
  cream: {
    ground: "radial-gradient(85% 75% at 50% 38%, #fffaf5 0%, #fbede2 58%, #f3dac8 100%)",
    text: "#2b1d16",
    sub: "rgba(43,29,22,0.64)",
    accent: "#d4582a",
    shadow: "0 50px 110px rgba(120,70,40,0.16), 0 14px 34px rgba(120,70,40,0.10)",
  },
  blush: {
    ground: "linear-gradient(135deg, #ffe9dc 0%, #fcd8cc 45%, #f5d2e2 100%)",
    text: "#2a1420",
    sub: "rgba(42,20,32,0.64)",
    accent: "#cf4f22",
    shadow: "0 50px 110px rgba(140,60,70,0.18), 0 14px 34px rgba(140,60,70,0.12)",
  },
  sorbet: {
    ground: "radial-gradient(85% 75% at 50% 38%, #ffeedd 0%, #ffd3b0 55%, #ffbb8c 100%)",
    text: "#3a1606",
    sub: "rgba(58,22,6,0.66)",
    accent: "#b9410f",
    shadow: "0 50px 110px rgba(170,70,10,0.22), 0 14px 34px rgba(170,70,10,0.15)",
  },
  sand: {
    ground: "radial-gradient(85% 75% at 50% 38%, #f8f3ed 0%, #efe5da 60%, #e5d6c6 100%)",
    text: "#1c2030",
    sub: "rgba(28,32,48,0.62)",
    accent: "#e2622b",
    shadow: "0 50px 110px rgba(60,50,40,0.18), 0 14px 34px rgba(60,50,40,0.12)",
  },
} satisfies Record<string, Look>;
export type LookName = keyof typeof LOOKS;

const clamp = { extrapolateLeft: "clamp", extrapolateRight: "clamp" } as const;
const smooth = Easing.bezier(0.45, 0, 0.2, 1);
const expo = Easing.bezier(0.16, 1, 0.3, 1);
const bar = (n: number) => beatFrame(barBeat(n));
const ramp = (f: number, a: number, b: number, e = smooth) => interpolate(f, [a, b], [0, 1], { ...clamp, easing: e });
const mix = (a: number, b: number, t: number) => a + (b - a) * t;

type Box = { x: number; y: number; w: number; h: number };
type BoxName = "composer" | "answer" | "badge1" | "badge2" | "badge3" | "badge4" | "grounded" | "sources" | "viewer" | "passage";
const box = (name: BoxName): Box => {
  const b = (boxes as Record<string, { box: Box | null }>)[name]?.box;
  if (!b) throw new Error(`public/act1/boxes.json has no box for ${name}`);
  return b;
};

/**
 * The answer's citation chips ("1 2 3 7") as drawn in the held still
 * (public/act1/hold-answer.png), measured from its pixels: the boxes logged
 * during the take predate the answer's final layout.
 */
const CHIPS: Box = { x: 0.4083, y: 0.4865, w: 0.0604, h: 0.0277 };

/** The app window, on the right; the narration column is on the left. */
const WIN = { x: 700, y: 225, w: 1120, h: 630 };

/** The narration column. */
const COLUMN = { x: 120, w: 500 };

/** A shot: what the camera frames, in footage fractions (whole app = the full frame). */
type Shot = Box;
const WHOLE: Shot = { x: 0, y: 0, w: 1, h: 1 };

/** Grow a region by `m` (fractions of the footage) on every side. */
const pad = (b: Box, mx: number, my = mx): Box => ({ x: b.x - mx, y: b.y - my, w: b.w + 2 * mx, h: b.h + 2 * my });

/**
 * The camera that frames `shot`: the zoom that fits the region in the window
 * (keeping 16:9), centred on it, never past the footage's edges.
 */
function frameShot(shot: Shot): { scale: number; tx: number; ty: number } {
  const scale = Math.max(1, Math.min(1 / shot.w, 1 / shot.h, 2.3));
  const cx = shot.x + shot.w / 2;
  const cy = shot.y + shot.h / 2;
  const spare = (scale - 1) / 2;
  const tx = Math.max(-spare, Math.min(spare, (0.5 - cx) * scale)) * WIN.w;
  const ty = Math.max(-spare, Math.min(spare, (0.5 - cy) * scale)) * WIN.h;
  return { scale, tx, ty };
}

/** Scenes, in bars of the track. */
const SCENE = { intro: 0, ask: 2, search: 6, answer: 9, cite: 13, open: 16, passage: 19, end: 23 } as const;
type SceneName = keyof typeof SCENE;

const CAPTIONS: { from: SceneName; text: string }[] = [
  { from: "ask", text: "Ask a question about your papers, in plain words." },
  { from: "search", text: "Shodh searches your library for the passages that answer it." },
  { from: "answer", text: "The answer is written from those passages." },
  { from: "cite", text: "Each number links a sentence to the paper it came from." },
  { from: "open", text: "Click a number to open that paper…" },
  { from: "passage", text: "…at the exact passage, outlined." },
];

/** A box drawn on over the footage, in window pixels from footage fractions. */
const Outline: React.FC<{ b: Box; p: number; pad: number; radius: number; width: number; color: string }> = ({ b, p, pad: m, radius, width, color }) => {
  const w = b.w * WIN.w + m * 2;
  const h = b.h * WIN.h + m * 2;
  return (
    <svg style={{ position: "absolute", left: b.x * WIN.w - m, top: b.y * WIN.h - m, overflow: "visible" }} width={w} height={h} aria-hidden="true">
      <rect x={0} y={0} width={w} height={h} rx={radius} fill="none" stroke={color} strokeWidth={width} pathLength={1} strokeDasharray={1} strokeDashoffset={1 - p} />
    </svg>
  );
};

const fill: React.CSSProperties = { position: "absolute", inset: 0, width: "100%", height: "100%" };

/** The logo and name, no tile: large and centred at the open, then settled top left for the rest. */
const Brand: React.FC<{ settle: number; look: Look }> = ({ settle, look }) => {
  const mark = mix(300, 104, settle);
  const name = mix(190, 60, settle);
  const gap = mix(30, 16, settle);
  const width = mark + gap + name * 2.95;
  const left = mix(960 - width / 2, 52, settle);
  const top = mix(250, 10, settle);
  return (
    <div style={{ position: "absolute", left, top, display: "flex", alignItems: "center", gap, fontFamily: font.sans, color: look.text }}>
      <Img src={staticFile("shodh-logo.svg")} style={{ width: mark, height: mark }} />
      <div style={{ fontSize: name, fontWeight: 790, letterSpacing: "-0.045em", lineHeight: 1 }}>Shodh</div>
    </div>
  );
};

export const Act1: React.FC<{ look: LookName }> = ({ look: lookName }) => {
  const frame = useCurrentFrame();
  const look = LOOKS[lookName];
  const at = (s: SceneName) => (SCENE[s] === 0 ? 0 : bar(SCENE[s]));
  const F = { ask: at("ask"), search: at("search"), answer: at("answer"), cite: at("cite"), open: at("open"), passage: at("passage"), end: at("end") };

  // Opening: logo, name and line hold, then settle top left as the window rises.
  const settle = ramp(frame, F.ask - 50, F.ask - 6);
  const tagline = ramp(frame, 18, 40, expo) * (1 - ramp(frame, F.ask - 62, F.ask - 40));
  const rise = ramp(frame, F.ask - 46, F.ask + 4);

  // The shots, each the part of the app in play with even margins around it.
  const composerShot = pad(box("composer"), 0.05, 0.12);
  const conversationShot: Shot = { x: 0.3, y: 0.0, w: 0.6, h: 0.44 };
  const answerShot = pad(box("answer"), 0.04, 0.03);
  // The full width of the answer column, its last lines and the chips.
  const citeShot: Shot = { x: 0.33, y: CHIPS.y - 0.24, w: 0.5, h: 0.34 };
  const passageShot = pad(box("passage"), 0.05, 0.07);

  const keys: [number, Shot][] = [
    [F.ask + 4, WHOLE],
    [F.ask + 54, composerShot],
    [F.search - 6, composerShot],
    [F.search + 44, conversationShot],
    [F.answer + 6, conversationShot],
    [F.answer + 56, answerShot],
    [F.cite + 4, answerShot],
    [F.cite + 52, citeShot],
    [F.open + 6, citeShot],
    [F.open + 56, WHOLE],
    [F.passage + 6, WHOLE],
    [F.passage + 70, passageShot],
  ];
  let cam = frameShot(keys[0][1]);
  for (let i = 1; i < keys.length; i++) {
    const [f0, s0] = keys[i - 1];
    const [f1, s1] = keys[i];
    if (frame >= f0) {
      const a = frameShot(s0);
      const b = frameShot(s1);
      const t = ramp(frame, f0, f1);
      cam = { scale: mix(a.scale, b.scale, t), tx: mix(a.tx, b.tx, t), ty: mix(a.ty, b.ty, t) };
    }
  }

  const outline = ramp(frame, F.passage + 74, F.passage + 104, expo);
  const tag = ramp(frame, F.passage + 100, F.passage + 120, expo);
  const pass = box("passage");

  const caption = [...CAPTIONS].reverse().find((c) => frame >= at(c.from));
  const captionStart = caption ? at(caption.from) : 0;
  const captionIndex = caption ? CAPTIONS.indexOf(caption) : -1;
  const cIn = ramp(frame, captionStart + 4, captionStart + 22, expo);

  return (
    <AbsoluteFill style={{ background: look.ground, overflow: "hidden", perspective: 2400 }}>
      <div
        style={{
          position: "absolute",
          left: mix(2400, WIN.x, rise),
          top: WIN.y,
          width: WIN.w,
          height: WIN.h,
          borderRadius: 18,
          overflow: "hidden",
          background: "#fff",
          boxShadow: look.shadow,
          transform: `rotateY(${(1 - rise) * -24}deg)`,
          transformOrigin: "100% 50%",
        }}
      >
        <div style={{ position: "absolute", inset: 0, transformOrigin: "50% 50%", transform: `translate(${cam.tx}px, ${cam.ty}px) scale(${cam.scale})` }}>
          <Sequence from={F.ask - 46} durationInFrames={F.search - (F.ask - 46)} layout="none">
            <OffthreadVideo src={staticFile("act1/type.mp4")} muted style={fill} />
          </Sequence>
          <Sequence from={F.search} durationInFrames={F.answer - F.search} layout="none">
            <OffthreadVideo src={staticFile("act1/search.mp4")} muted style={fill} />
          </Sequence>
          <Sequence from={F.answer} durationInFrames={F.open - F.answer} layout="none">
            <OffthreadVideo src={staticFile("act1/answer.mp4")} muted style={fill} />
            <Img src={staticFile("act1/hold-answer.png")} style={{ ...fill, opacity: ramp(frame, F.answer + 136, F.answer + 146) }} />
          </Sequence>
          <Sequence from={F.open} durationInFrames={F.end + 60 - F.open} layout="none">
            <OffthreadVideo src={staticFile("act1/click.mp4")} muted style={fill} />
            <Img src={staticFile("act1/hold-viewer.png")} style={{ ...fill, opacity: ramp(frame, F.open + 132, F.open + 142) }} />
          </Sequence>
          {frame >= F.cite + 50 && frame < F.open + 20 ? <Outline b={CHIPS} p={ramp(frame, F.cite + 50, F.cite + 76, expo)} pad={5} radius={8} width={2.5} color={look.accent} /> : null}
          {frame >= F.passage + 74 ? <Outline b={pass} p={outline} pad={8} radius={8} width={3} color={look.accent} /> : null}
          {frame >= F.passage + 100 ? (
            <div style={{ position: "absolute", left: pass.x * WIN.w, top: pass.y * WIN.h - 42, padding: "6px 14px", borderRadius: 999, background: look.accent, color: "#fff", fontFamily: font.sans, fontSize: 15, fontWeight: 700, opacity: tag, transform: `translateY(${(1 - tag) * 8}px)` }}>
              Cited passage · SPANN, page 2
            </div>
          ) : null}
        </div>
      </div>

      <Brand settle={settle} look={look} />
      <div style={{ position: "absolute", left: 0, right: 0, top: 600, textAlign: "center", fontFamily: font.sans, fontSize: 46, fontWeight: 520, letterSpacing: "-0.02em", color: look.sub, opacity: tagline, transform: `translateY(${(1 - tagline) * 14}px)` }}>
        Answers from your own papers, with the source for every claim.
      </div>

      {caption ? (
        <div style={{ position: "absolute", left: COLUMN.x, top: 0, bottom: 0, width: COLUMN.w, display: "flex", flexDirection: "column", justifyContent: "center", fontFamily: font.sans, color: look.text }}>
          <div style={{ display: "flex", alignItems: "center", gap: 18, marginBottom: 30, opacity: cIn, transform: `translateY(${(1 - cIn) * 14}px)` }}>
            <div style={{ width: 58, height: 58, borderRadius: 29, background: look.accent, color: "#fff", display: "flex", alignItems: "center", justifyContent: "center", fontSize: 28, fontWeight: 760 }}>{captionIndex + 1}</div>
            <div style={{ height: 2, width: 64, background: look.accent, opacity: 0.5 }} />
            <div style={{ fontSize: 22, fontWeight: 650, letterSpacing: "0.2em", textTransform: "uppercase", color: look.sub }}>{`Step ${captionIndex + 1} of ${CAPTIONS.length}`}</div>
          </div>
          <div style={{ overflow: "hidden", paddingBottom: 8 }}>
            <div style={{ fontSize: 54, fontWeight: 700, lineHeight: 1.14, letterSpacing: "-0.03em", transform: `translateY(${(1 - cIn) * 40}px)`, opacity: cIn }}>{caption.text}</div>
          </div>
        </div>
      ) : null}

      <Html5Audio src={staticFile("music/track.mp3")} />
    </AbsoluteFill>
  );
};
