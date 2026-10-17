// The Shodh film — the problem, what Shodh is and who it is for, then one
// unhurried walkthrough on the real app: the core flow (ask, search, answer,
// citations, the exact passage), trust (flagged statements, the verified
// log), a short tour (paper graph, workspaces, models), and the close. Two problem beats (too many
// papers to read; AI answers that cannot be traced to a source), then the
// Shodh logo, name, line and audience, which settle into the brand mark top
// left as the app window rises. One window, one continuous
// camera that frames each part of the app in turn (the composer, the
// conversation, the answer, the citations, the passage) with even margins;
// a numbered caption under the window says what is happening at each step.
// Every step lasts three or four bars of the track.
// Footage: a 1440×810 take of the demo profile; framing and graphics come
// from the element boxes logged during the take (public/act1/boxes.json).
import React from "react";
import { AbsoluteFill, Easing, Html5Audio, Img, OffthreadVideo, Sequence, interpolate, staticFile, useCurrentFrame } from "remotion";
import boxes from "../public/act1/boxes.json";
import activityBoxes from "../public/act2/activity-boxes.json";
import graphBoxes from "../public/act2/graph-boxes.json";
import groundBoxes from "../public/act2/ground-boxes.json";
import modelsBoxes from "../public/act2/models-boxes.json";
import workspaceBoxes from "../public/act2/workspace-boxes.json";
import { barBeat, beatFrame } from "./beats";
import { font } from "./theme";

/** A look for the act: ground, type and the one accent (the deep Shodh orange family). */
export type Look = { ground: string; text: string; sub: string; accent: string; shadow: string };

/** Anthropic beige: an ivory-to-oat ground, slate type, Shodh orange as the one accent. */
export const LOOKS = {
  beige: {
    ground: "radial-gradient(90% 80% at 50% 40%, #faf9f5 0%, #f3f0e8 60%, #ebe6da 100%)",
    text: "#141413",
    sub: "rgba(20,20,19,0.6)",
    accent: "#c94d1c",
    shadow: "0 40px 100px rgba(60,50,30,0.14), 0 12px 30px rgba(60,50,30,0.10)",
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
const box = (name: BoxName): Box => boxFrom(boxes, "act1/boxes.json", name);

/** An element box logged during a take; a missing one is a broken take, so it fails the render. */
function boxFrom(log: Record<string, { box: Box | null }>, file: string, name: string): Box {
  const b = log[name]?.box;
  if (!b) throw new Error(`public/${file} has no box for ${name}`);
  return b;
}

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
const SCENE = { papers: 0, trust: 4, brand: 8, ask: 12, search: 16, answer: 19, cite: 23, open: 26, passage: 29, ground: 33, activity: 37, graph: 41, workspace: 44, models: 47, close: 51 } as const;
type SceneName = keyof typeof SCENE;

const CAPTIONS: { from: SceneName; text: string }[] = [
  { from: "ask", text: "Ask a question about your papers, in plain words." },
  { from: "search", text: "Shodh searches your library for the passages that answer it." },
  { from: "answer", text: "The answer is written from those passages." },
  { from: "cite", text: "Each number links a sentence to the paper it came from." },
  { from: "open", text: "Click a number to open that paper…" },
  { from: "passage", text: "…at the exact passage, outlined." },
  { from: "ground", text: "When a sentence is not backed by its source, Shodh flags it." },
  { from: "activity", text: "Every question, search and answer is kept in a tamper-evident log on your computer." },
  { from: "graph", text: "See how your papers cite each other." },
  { from: "workspace", text: "Keep each project in its own workspace." },
  { from: "models", text: "Use the model you choose, or run entirely on your computer." },
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

/** The library: twelve real paper covers, rendered from their first pages. */
const COVERS = Array.from({ length: 12 }, (_, i) => `act1/cover-${String(i).padStart(2, "0")}.png`);

/** Problem one: the twelve papers falling into a pile, with their real page count. */
const PaperPile: React.FC<{ frame: number; look: Look; out: number }> = ({ frame, look, out }) => {
  const cx = WIN.x + WIN.w / 2;
  const cy = WIN.y + WIN.h / 2 - 20;
  const angles = [-9, 6, -4, 11, -13, 3, 8, -6, 14, -2, 5, -10];
  const dxs = [-210, 150, -60, 230, -260, 40, 190, -150, 260, -30, 90, -200];
  const dys = [-40, -80, 30, 10, 60, -110, 80, -10, -60, 70, -30, 100];
  return (
    <>
      {COVERS.map((src, i) => {
        const t = ramp(frame, 10 + i * 7, 34 + i * 7, expo);
        const w = 250;
        return (
          <div
            key={src}
            style={{
              position: "absolute",
              left: cx + dxs[i] * t - w / 2,
              top: mix(-460, cy + dys[i] - 162, t) - out * 60,
              width: w,
              height: 324,
              borderRadius: 6,
              overflow: "hidden",
              background: "#fff",
              boxShadow: "0 18px 40px rgba(60,50,30,0.18)",
              transform: `rotate(${angles[i] * t}deg)`,
              opacity: Math.min(1, t * 2) * (1 - out),
            }}
          >
            <Img src={staticFile(src)} style={{ width: "100%", height: "100%", objectFit: "cover" }} />
          </div>
        );
      })}
      <div style={{ position: "absolute", left: WIN.x, width: WIN.w, top: WIN.y + WIN.h - 10, textAlign: "center", fontFamily: font.sans, fontSize: 30, fontWeight: 600, color: look.sub, opacity: ramp(frame, 100, 120, expo) * (1 - out) }}>
        12 papers · 224 pages
      </div>
    </>
  );
};

/** Problem two: an answer whose claim has an empty source slot. */
const Untraced: React.FC<{ frame: number; look: Look; out: number }> = ({ frame, look, out }) => {
  const card = ramp(frame, 0, 26, expo);
  const lines = [470, 520, 430, 500, 380];
  const mark = ramp(frame, 40, 64, expo);
  const slot = ramp(frame, 66, 86, expo);
  return (
    <div style={{ position: "absolute", left: WIN.x + 150, top: WIN.y + 90, width: 820, opacity: card * (1 - out), transform: `translateY(${(1 - card) * 30 - out * 30}px)` }}>
      <div style={{ borderRadius: 20, background: "#fff", boxShadow: look.shadow, padding: "40px 44px" }}>
        {lines.map((w, i) => (
          <div key={i} style={{ position: "relative", height: 16, width: w, borderRadius: 8, background: "#e7e4dc", marginBottom: 26 }}>
            {i === 2 ? (
              <>
                <div style={{ position: "absolute", left: -6, top: -8, height: 32, width: (w + 12) * mark, borderRadius: 8, background: "rgba(201,77,28,0.16)" }} />
                <div style={{ position: "absolute", left: w + 18, top: -12, padding: "6px 14px", borderRadius: 10, border: `2.5px dashed ${look.accent}`, color: look.accent, fontFamily: font.sans, fontSize: 20, fontWeight: 700, opacity: slot, transform: `scale(${0.8 + 0.2 * slot})`, transformOrigin: "left center", whiteSpace: "nowrap" }}>
                  source?
                </div>
              </>
            ) : null}
          </div>
        ))}
      </div>
      <div style={{ marginTop: 34, fontFamily: font.sans, fontSize: 34, fontWeight: 600, color: look.sub, opacity: ramp(frame, 80, 100, expo) }}>Where did this sentence come from?</div>
    </div>
  );
};

/** The left column for the problem beats: a small label and the line. */
const ProblemText: React.FC<{ text: string; inn: number; out: number; look: Look }> = ({ text, inn, out, look }) => (
  <div style={{ position: "absolute", left: COLUMN.x, top: 0, bottom: 0, width: COLUMN.w, display: "flex", flexDirection: "column", justifyContent: "center", fontFamily: font.sans, color: look.text, opacity: inn * (1 - out) }}>
    <div style={{ fontSize: 22, fontWeight: 650, letterSpacing: "0.2em", textTransform: "uppercase", color: look.accent, marginBottom: 26 }}>The problem</div>
    <div style={{ overflow: "hidden", paddingBottom: 8 }}>
      <div style={{ fontSize: 58, fontWeight: 700, lineHeight: 1.12, letterSpacing: "-0.03em", transform: `translateY(${(1 - inn) * 40}px)` }}>{text}</div>
    </div>
  </div>
);

/** The close, under the brand back at the centre: the line, what it costs and where to get it. */
const Close: React.FC<{ frame: number; look: Look }> = ({ frame, look }) => {
  const line = ramp(frame, 50, 72, expo);
  const offer = ramp(frame, 80, 102, expo);
  const where = ramp(frame, 104, 126, expo);
  return (
    <>
      <div style={{ position: "absolute", left: 0, right: 0, top: 590, textAlign: "center", fontFamily: font.sans, fontSize: 58, fontWeight: 700, letterSpacing: "-0.03em", color: look.text, opacity: line, transform: `translateY(${(1 - line) * 16}px)` }}>
        Research you can verify.
      </div>
      <div style={{ position: "absolute", left: 0, right: 0, top: 690, textAlign: "center", fontFamily: font.sans, fontSize: 34, fontWeight: 500, color: look.sub, opacity: offer, transform: `translateY(${(1 - offer) * 12}px)` }}>
        Free and open source · Windows and Mac
      </div>
      <div style={{ position: "absolute", left: 0, right: 0, top: 760, display: "flex", justifyContent: "center", opacity: where, transform: `translateY(${(1 - where) * 12}px)` }}>
        <div style={{ padding: "14px 30px", borderRadius: 999, background: look.accent, color: "#fff", fontFamily: font.sans, fontSize: 30, fontWeight: 650 }}>github.com/varun29ankuS/shodhRAG</div>
      </div>
    </>
  );
};

export const ShodhFilm: React.FC = () => {
  const frame = useCurrentFrame();
  const look = LOOKS.beige;
  const at = (s: SceneName) => (SCENE[s] === 0 ? 0 : bar(SCENE[s]));
  const F = {
    trust: at("trust"),
    brand: at("brand"),
    ask: at("ask"),
    search: at("search"),
    answer: at("answer"),
    cite: at("cite"),
    open: at("open"),
    passage: at("passage"),
    ground: at("ground"),
    activity: at("activity"),
    graph: at("graph"),
    workspace: at("workspace"),
    models: at("models"),
    close: at("close"),
  };

  // The problem, in two beats; then the brand, which settles top left as the window rises.
  const papersIn = ramp(frame, 4, 26, expo);
  const papersOut = ramp(frame, F.trust - 16, F.trust + 4);
  const trustIn = ramp(frame, F.trust + 6, F.trust + 28, expo);
  const trustOut = ramp(frame, F.brand - 16, F.brand + 2);
  const brandIn = ramp(frame, F.brand, F.brand + 26, expo);
  // The brand settles top left for the walkthrough and returns to the centre for the close.
  const settle = ramp(frame, F.ask - 50, F.ask - 6) * (1 - ramp(frame, F.close + 6, F.close + 50));
  const leave = ramp(frame, F.close - 14, F.close + 24);
  const fadeCopy = 1 - ramp(frame, F.ask - 62, F.ask - 40);
  const tagline = ramp(frame, F.brand + 18, F.brand + 40, expo) * fadeCopy;
  const audience = ramp(frame, F.brand + 46, F.brand + 68, expo) * fadeCopy;
  const rise = ramp(frame, F.ask - 46, F.ask + 4);

  // The shots, each the part of the app in play with even margins around it.
  const composerShot = pad(box("composer"), 0.05, 0.12);
  const conversationShot: Shot = { x: 0.3, y: 0.0, w: 0.6, h: 0.44 };
  const answerShot = pad(box("answer"), 0.04, 0.03);
  // The full width of the answer column, its last lines and the chips.
  const citeShot: Shot = { x: 0.33, y: CHIPS.y - 0.24, w: 0.5, h: 0.34 };
  const passageShot = pad(box("passage"), 0.05, 0.07);
  // Trust and the tour, from the boxes each take logged.
  const flagShot = pad(boxFrom(groundBoxes, "act2/ground-boxes.json", "flag"), 0.16, 0.14);
  const chipAfter = boxFrom(groundBoxes, "act2/ground-boxes.json", "chipAfter");
  const panelShot: Shot = { x: 0.33, y: chipAfter.y - 0.06, w: 0.5, h: 0.24 };
  const verify = boxFrom(activityBoxes, "act2/activity-boxes.json", "verify");
  const activityShot: Shot = { x: 0.2, y: 0.02, w: verify.x + verify.w - 0.2 + 0.06, h: 0.44 };
  const graphShot = pad(boxFrom(graphBoxes, "act2/graph-boxes.json", "graph"), 0.02, 0.0);
  const header = boxFrom(workspaceBoxes, "act2/workspace-boxes.json", "header");
  const workspaceShot: Shot = { x: 0.23, y: header.y - 0.06, w: 0.7, h: 0.42 };
  const tiers = boxFrom(modelsBoxes, "act2/models-boxes.json", "tiers");
  const modelsShot: Shot = { x: 0.33, y: 0.0, w: 0.56, h: tiers.y + tiers.h + 0.04 };

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
    [F.ground - 8, passageShot],
    [F.ground + 30, WHOLE],
    [F.ground + 44, flagShot],
    [F.ground + 112, flagShot],
    [F.ground + 150, panelShot],
    [F.activity - 4, panelShot],
    [F.activity + 30, WHOLE],
    [F.activity + 110, WHOLE],
    [F.activity + 150, activityShot],
    [F.graph - 4, activityShot],
    [F.graph + 30, WHOLE],
    [F.graph + 70, WHOLE],
    [F.graph + 110, graphShot],
    [F.workspace - 4, graphShot],
    [F.workspace + 30, WHOLE],
    [F.workspace + 90, WHOLE],
    [F.workspace + 130, workspaceShot],
    [F.models - 4, workspaceShot],
    [F.models + 30, WHOLE],
    [F.models + 90, WHOLE],
    [F.models + 140, modelsShot],
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

  const caption = frame < F.close ? [...CAPTIONS].reverse().find((c) => frame >= at(c.from)) : undefined;
  const captionStart = caption ? at(caption.from) : 0;
  const captionIndex = caption ? CAPTIONS.indexOf(caption) : -1;
  const cIn = ramp(frame, captionStart + 4, captionStart + 22, expo);

  return (
    <AbsoluteFill style={{ background: look.ground, overflow: "hidden", perspective: 2400 }}>
      <div
        style={{
          position: "absolute",
          left: mix(mix(2400, WIN.x, rise), 2400, leave),
          top: WIN.y,
          width: WIN.w,
          height: WIN.h,
          borderRadius: 18,
          overflow: "hidden",
          background: "#fff",
          boxShadow: look.shadow,
          transform: `rotateY(${(1 - rise) * -24 + leave * -24}deg)`,
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
          <Sequence from={F.open} durationInFrames={F.ground - F.open} layout="none">
            <OffthreadVideo src={staticFile("act1/click.mp4")} muted style={fill} />
            <Img src={staticFile("act1/hold-viewer.png")} style={{ ...fill, opacity: ramp(frame, F.open + 132, F.open + 142) }} />
          </Sequence>
          {(
            [
              ["ground", F.ground, F.activity],
              ["activity", F.activity, F.graph],
              ["graph", F.graph, F.workspace],
              ["workspace", F.workspace, F.models],
              ["models", F.models, F.close + 30],
            ] as const
          ).map(([name, from, to]) => (
            <Sequence key={name} from={from} durationInFrames={to - from} layout="none">
              <OffthreadVideo src={staticFile(`act2/${name}.mp4`)} muted style={fill} />
              <Img src={staticFile(`act2/${name}-hold.png`)} style={{ ...fill, opacity: ramp(frame, to - 26, to - 16) }} />
            </Sequence>
          ))}
          {frame >= F.cite + 50 && frame < F.open + 20 ? <Outline b={CHIPS} p={ramp(frame, F.cite + 50, F.cite + 76, expo)} pad={5} radius={8} width={2.5} color={look.accent} /> : null}
          {frame >= F.passage + 74 && frame < F.ground + 16 ? <Outline b={pass} p={outline} pad={8} radius={8} width={3} color={look.accent} /> : null}
          {frame >= F.passage + 100 && frame < F.ground + 16 ? (
            <div style={{ position: "absolute", left: pass.x * WIN.w, top: pass.y * WIN.h - 42, padding: "6px 14px", borderRadius: 999, background: look.accent, color: "#fff", fontFamily: font.sans, fontSize: 15, fontWeight: 700, opacity: tag, transform: `translateY(${(1 - tag) * 8}px)` }}>
              Cited passage · SPANN, page 2
            </div>
          ) : null}
        </div>
      </div>

      {frame < F.trust + 4 ? <PaperPile frame={frame} look={look} out={papersOut} /> : null}
      {frame < F.trust + 4 ? <ProblemText text="You have more papers than time to read them." inn={papersIn} out={papersOut} look={look} /> : null}
      {frame >= F.trust && frame < F.brand + 2 ? <Untraced frame={frame - F.trust} look={look} out={trustOut} /> : null}
      {frame >= F.trust && frame < F.brand + 2 ? <ProblemText text="AI answers you cannot check are a risk in research." inn={trustIn} out={trustOut} look={look} /> : null}
      {frame >= F.brand ? (
        <div style={{ position: "absolute", inset: 0, opacity: Math.min(1, brandIn * 1.4), transform: `scale(${0.94 + 0.06 * brandIn})`, transformOrigin: "50% 40%" }}>
          <Brand settle={settle} look={look} />
        </div>
      ) : null}
      {frame >= F.close ? <Close frame={frame - F.close} look={look} /> : null}
      <div style={{ position: "absolute", left: 0, right: 0, top: 590, textAlign: "center", fontFamily: font.sans, fontSize: 50, fontWeight: 600, letterSpacing: "-0.02em", color: look.text, opacity: tagline, transform: `translateY(${(1 - tagline) * 14}px)` }}>
        A research assistant for your own papers.
      </div>
      <div style={{ position: "absolute", left: 0, right: 0, top: 672, textAlign: "center", fontFamily: font.sans, fontSize: 34, fontWeight: 500, color: look.sub, opacity: audience, transform: `translateY(${(1 - audience) * 12}px)` }}>
        For researchers, students and analysts. Every answer cites the exact page.
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
