// Act 1 — the core story, in rebuilt UI at full scale: the question types
// into the composer, the library fans out and the three sources that answer
// it come forward, the answer streams in with its citations flying in from
// those papers, and citation 2 opens the real page with the real passage.
// Every word, title and page is the product's own (the SPANN answer from
// the recorded take, the library's covers, SPANN page 2).
import React from "react";
import { AbsoluteFill, Easing, Html5Audio, Img, interpolate, spring, staticFile, useCurrentFrame, useVideoConfig } from "remotion";
import assets from "../public/act1/assets.json";
import { barBeat, beatFrame } from "./beats";
import { font } from "./theme";

export type Palette = "night" | "paper" | "ember";

type Look = {
  ground: string;
  glow: string;
  text: string;
  sub: string;
  accent: string;
  card: string;
  cardText: string;
  shadow: string;
  grain: number;
};

const LOOKS: Record<Palette, Look> = {
  night: {
    ground: "radial-gradient(90% 80% at 50% 38%, #221813 0%, #120d0b 55%, #070505 100%)",
    glow: "rgba(255,106,43,0.30)",
    text: "#fbf7f3",
    sub: "rgba(251,247,243,0.62)",
    accent: "#ff6a2b",
    card: "#ffffff",
    cardText: "#18181b",
    shadow: "0 60px 140px rgba(0,0,0,0.65), 0 18px 40px rgba(0,0,0,0.45)",
    grain: 0.1,
  },
  paper: {
    ground: "radial-gradient(90% 80% at 50% 35%, #fbf8f3 0%, #f3ede4 60%, #e9e0d3 100%)",
    glow: "rgba(201,77,28,0.10)",
    text: "#17120e",
    sub: "rgba(23,18,14,0.58)",
    accent: "#c94d1c",
    card: "#ffffff",
    cardText: "#18181b",
    shadow: "0 50px 110px rgba(80,50,20,0.18), 0 12px 30px rgba(80,50,20,0.12)",
    grain: 0.06,
  },
  ember: {
    ground: "radial-gradient(95% 85% at 40% 30%, #b8471a 0%, #7c2c0d 52%, #3a1305 100%)",
    glow: "rgba(255,170,120,0.22)",
    text: "#fff4ea",
    sub: "rgba(255,244,234,0.66)",
    accent: "#ffb183",
    card: "#ffffff",
    cardText: "#18181b",
    shadow: "0 60px 130px rgba(30,6,0,0.55), 0 16px 36px rgba(30,6,0,0.4)",
    grain: 0.12,
  },
};

const clamp = { extrapolateLeft: "clamp", extrapolateRight: "clamp" } as const;
const expo = Easing.bezier(0.16, 1, 0.3, 1);
const inOut = Easing.bezier(0.65, 0, 0.35, 1);
const bar = (n: number) => beatFrame(barBeat(n));
const ramp = (f: number, a: number, b: number, e = expo) => interpolate(f, [a, b], [0, 1], { ...clamp, easing: e });
const mix = (a: number, b: number, t: number) => a + (b - a) * t;

const QUESTION = "In my papers, how does SPANN keep search fast when the index no longer fits in RAM?";

/** The answer as recorded, split where its citations sit. */
const ANSWER: { text: string; cite?: number[] }[] = [
  { text: "SPANN keeps search fast when the index no longer fits in RAM by using a memory-disk hybrid approach: it stores only the centroid points of posting lists in memory, as a fast coarse-grained index, while keeping the large posting lists on disk.", cite: [1, 2] },
  { text: " To keep latency low and recall high, it prunes unnecessary posting lists for each query and improves them with hierarchical balanced clustering.", cite: [3] },
];

/** Library order (sorted file names); the answer cites SPANN, Faiss and ANN-Benchmarks. */
const CITED: Record<number, number> = { 1: 9, 2: 11, 3: 0 };
const SOURCE_NAMES: Record<number, string> = { 9: "SPANN - Billion-scale ANN Search", 11: "The Faiss Library", 0: "ANN-Benchmarks" };

const Grain: React.FC<{ strength: number }> = ({ strength }) => {
  const frame = useCurrentFrame();
  const { width, height } = useVideoConfig();
  return (
    <svg width={width} height={height} style={{ position: "absolute", inset: 0, mixBlendMode: "overlay", opacity: strength, pointerEvents: "none" }}>
      <filter id="act1grain">
        <feTurbulence type="fractalNoise" baseFrequency="0.9" numOctaves={2} seed={Math.floor(frame / 2) % 8} stitchTiles="stitch" />
        <feColorMatrix type="saturate" values="0" />
      </filter>
      <rect width="100%" height="100%" filter="url(#act1grain)" />
    </svg>
  );
};

/** A headline whose words rise out of a mask, one after another, from `at`. */
const Headline: React.FC<{ words: string[]; at: number; out: number; look: Look; size: number; x: number; y: number; accentLast?: boolean; maxWidth?: number }> = ({
  words,
  at,
  out,
  look,
  size,
  x,
  y,
  accentLast = false,
  maxWidth = 760,
}) => {
  const frame = useCurrentFrame();
  return (
    <div style={{ position: "absolute", left: x, top: y, display: "flex", flexWrap: "wrap", gap: `0 ${size * 0.24}px`, maxWidth, fontFamily: font.sans, fontWeight: 760, fontSize: size, lineHeight: 1.0, letterSpacing: "-0.045em" }}>
      {words.map((w, i) => {
        const inn = ramp(frame, at + i * 4, at + i * 4 + 18);
        const leave = ramp(frame, out + i * 2, out + i * 2 + 10, Easing.bezier(0.7, 0, 0.84, 0));
        return (
          <span key={`${w}${i}`} style={{ display: "inline-block", overflow: "hidden", paddingBottom: "0.1em", marginBottom: "-0.1em" }}>
            <span style={{ display: "inline-block", transform: `translateY(${(1 - inn) * 110 - leave * 110}%)`, color: accentLast && i === words.length - 1 ? look.accent : look.text }}>{w}</span>
          </span>
        );
      })}
    </div>
  );
};

/** The composer, as the app draws it, at film scale. */
const Composer: React.FC<{ typed: string; caret: boolean; sent: number; look: Look }> = ({ typed, caret, sent, look }) => (
  <div style={{ width: 1240, borderRadius: 30, background: look.card, boxShadow: look.shadow, padding: "34px 40px 26px", boxSizing: "border-box", fontFamily: font.sans, color: look.cardText }}>
    <div style={{ fontSize: 40, fontWeight: 450, letterSpacing: "-0.015em", lineHeight: 1.3, minHeight: 104 }}>
      {typed}
      <span style={{ display: "inline-block", width: 3, height: 44, marginLeft: 3, verticalAlign: "-6px", background: "#c94d1c", opacity: caret ? 1 : 0 }} />
    </div>
    <div style={{ display: "flex", alignItems: "center", gap: 14, marginTop: 18 }}>
      {["NVIDIA: Nemotron 3 Super (free)", "Research", "All sources · 12 files"].map((t, i) => (
        <div key={t} style={{ fontSize: 20, fontWeight: 520, padding: "9px 16px", borderRadius: 999, background: i === 1 ? "#f4f4f3" : "transparent", border: "1.5px solid #e6e6e3", color: "#3f3f46" }}>
          {t}
        </div>
      ))}
      <div style={{ flex: 1 }} />
      <div style={{ width: 56, height: 56, borderRadius: 28, background: "#c94d1c", display: "flex", alignItems: "center", justifyContent: "center", transform: `scale(${1 - 0.12 * sent})` }}>
        <svg width="26" height="26" viewBox="0 0 24 24" aria-hidden="true">
          <path d="M12 19V5M6 11l6-6 6 6" fill="none" stroke="#fff" strokeWidth="2.6" strokeLinecap="round" strokeLinejoin="round" />
        </svg>
      </div>
    </div>
  </div>
);

/** A paper as its first page, with the library's own cover. */
const Cover: React.FC<{ index: number; w: number; style: React.CSSProperties; ring: number; accent: string }> = ({ index, w, style, ring, accent }) => {
  const c = assets.covers[index];
  const h = (w * c.h) / c.w;
  return (
    <div style={{ position: "absolute", width: w, height: h, borderRadius: 10, overflow: "hidden", background: "#fff", boxShadow: `0 30px 70px rgba(0,0,0,0.35), 0 0 0 ${ring * 5}px ${accent}`, ...style }}>
      <Img src={staticFile(`act1/${c.file}`)} style={{ width: "100%", height: "100%", objectFit: "cover" }} />
    </div>
  );
};

/** A citation chip in the app's style; `hot` turns it to the accent as it lands. */
const Chip: React.FC<{ n: number; hot: number }> = ({ n, hot }) => (
  <span
    style={{
      display: "inline-flex",
      alignItems: "center",
      justifyContent: "center",
      width: 34,
      height: 34,
      margin: "0 3px",
      verticalAlign: "3px",
      borderRadius: 9,
      fontSize: 19,
      fontWeight: 750,
      background: hot > 0.5 ? "#c94d1c" : "#f0efed",
      color: hot > 0.5 ? "#fff" : "#52525b",
      boxShadow: hot > 0.5 ? "0 0 0 6px rgba(201,77,28,0.18)" : "none",
    }}
  >
    {n}
  </span>
);

export const Act1: React.FC<{ palette: Palette }> = ({ palette }) => {
  const frame = useCurrentFrame();
  const { fps, width, height } = useVideoConfig();
  const look = LOOKS[palette];
  const B = [0, bar(1), bar(2), bar(4), bar(6), bar(8)];

  // A — the question types into the composer.
  const typed = QUESTION.slice(0, Math.floor(interpolate(frame, [10, B[2] - 14], [0, QUESTION.length], clamp)));
  const caret = Math.floor(frame / 9) % 2 === 0;
  const sent = frame >= B[2] - 8 && frame < B[2] - 2 ? 1 : 0;
  const lift = ramp(frame, B[2], B[2] + 26, inOut);
  const composerGone = ramp(frame, B[2] + 8, B[2] + 26);

  // B — the library fans out; the three cited papers come forward.
  const fanIn = ramp(frame, B[2] + 4, B[2] + 40);
  const pick = ramp(frame, B[2] + 52, B[3] - 6);
  const stackLeft = ramp(frame, B[3] - 4, B[3] + 24, inOut);

  // C — the answer streams; chips fly from the cited papers.
  const answerIn = spring({ frame: frame - B[3], fps, config: { damping: 20, stiffness: 120 } });
  const total = ANSWER.reduce((n, p) => n + p.text.length, 0);
  const shown = Math.floor(interpolate(frame, [B[3] + 10, B[4] - 16], [0, total], clamp));

  // D — citation 2 opens its page; the passage is outlined.
  const press = frame >= B[4] + 6 && frame < B[4] + 12 ? 1 : 0;
  const pageIn = ramp(frame, B[4] + 12, B[4] + 40, inOut);
  const sweep = ramp(frame, B[4] + 40, B[4] + 64, inOut);
  const outline = ramp(frame, B[4] + 58, B[4] + 76);
  const dolly = ramp(frame, B[4] + 64, B[5], inOut);
  const answerOut = ramp(frame, B[4] + 10, B[4] + 34, inOut);

  const camera = `scale(${1 + 0.035 * ramp(frame, 0, B[2], inOut)})`;

  // Paper fan geometry (12 papers on an arc, cited ones step forward).
  const fan = assets.covers.map((_, i) => {
    const cited = Object.values(CITED).indexOf(i);
    const a = (i - 5.5) / 5.5;
    const base = { x: width / 2 + a * 780, y: 600 + Math.abs(a) * 70, rot: a * 14, z: -Math.abs(a) * 260, s: 1 };
    const fwdX = [width / 2 - 330, width / 2, width / 2 + 330][cited] ?? base.x;
    const stackX = 250;
    const stackY = [330, 560, 790][cited] ?? 0;
    let x = mix(width / 2, base.x, fanIn);
    let y = mix(height + 300, base.y, fanIn);
    let rot = base.rot * fanIn;
    let z = base.z;
    let o = 1;
    let s = 1;
    if (cited >= 0) {
      x = mix(x, fwdX, pick);
      y = mix(y, 560, pick);
      rot = mix(rot, 0, pick);
      z = mix(z, 160, pick);
      x = mix(x, stackX, stackLeft);
      y = mix(y, stackY, stackLeft);
      s = mix(1, 0.62, stackLeft);
      z = mix(z, 0, stackLeft);
    } else {
      o = 1 - 0.75 * pick - 0.25 * stackLeft;
      y = y + pick * 80;
    }
    return { i, x, y, rot, z, o, s, cited };
  });
  const coverW = 230;
  const stackFade = 1 - ramp(frame, B[4] + 10, B[4] + 30);

  // Chip flights: when the stream reaches a citation, a chip flies from its paper.
  let cursor = 0;
  const flights: { n: number; at: number }[] = [];
  for (const part of ANSWER) {
    cursor += part.text.length;
    for (const n of part.cite ?? []) flights.push({ n, at: interpolate(cursor, [0, total], [B[3] + 10, B[4] - 16]) });
  }

  return (
    <AbsoluteFill style={{ background: look.ground, overflow: "hidden" }}>
      <AbsoluteFill style={{ background: `radial-gradient(40% 40% at 50% ${mix(70, 45, lift)}%, ${look.glow} 0%, rgba(0,0,0,0) 70%)` }} />
      <AbsoluteFill style={{ transform: camera, perspective: 1800 }}>
        {/* B — library fan */}
        {frame >= B[2] && stackFade > 0
          ? fan.map((p) => (
              <Cover
                key={p.i}
                index={p.i}
                w={coverW}
                accent={look.accent}
                ring={p.cited >= 0 ? pick : 0}
                style={{
                  left: p.x - coverW / 2,
                  top: p.y - 150,
                  opacity: p.o * stackFade,
                  transform: `translateZ(${p.z}px) rotate(${p.rot}deg) scale(${p.s})`,
                  zIndex: p.cited >= 0 ? 10 : 1,
                }}
              />
            ))
          : null}

        {/* A — composer */}
        <div
          style={{
            position: "absolute",
            left: width / 2 - 620,
            top: mix(470, -60, lift),
            transform: `scale(${mix(1, 0.8, lift)})`,
            transformOrigin: "center top",
            opacity: 1 - composerGone,
          }}
        >
          <Composer typed={typed} caret={caret} sent={sent} look={look} />
        </div>

        {/* C — answer */}
        {frame >= B[3] && answerOut < 1 ? (
          <div
            style={{
              position: "absolute",
              left: 560,
              top: 300,
              width: 1240,
              borderRadius: 26,
              background: look.card,
              boxShadow: look.shadow,
              padding: "44px 52px",
              boxSizing: "border-box",
              fontFamily: font.sans,
              color: look.cardText,
              fontSize: 34,
              lineHeight: 1.5,
              letterSpacing: "-0.01em",
              opacity: Math.min(1, answerIn * 1.4) * (1 - answerOut),
              transform: `translateY(${(1 - answerIn) * 60}px) scale(${1 - 0.08 * answerOut})`,
            }}
          >
            <div style={{ fontSize: 20, color: "#71717a", marginBottom: 18 }}>Worked for 18s · 2 steps · 3 sources</div>
            {(() => {
              let used = 0;
              return ANSWER.map((part, k) => {
                const start = used;
                used += part.text.length;
                const visible = part.text.slice(0, Math.max(0, Math.min(part.text.length, shown - start)));
                const done = shown >= used;
                return (
                  <span key={k}>
                    {visible}
                    {done
                      ? (part.cite ?? []).map((n) => {
                          const fl = flights.find((f) => f.n === n);
                          const landed = fl ? ramp(frame, fl.at, fl.at + 14) : 1;
                          const hot = n === 2 ? Math.max(1 - landed, press ? 1 : ramp(frame, B[4], B[4] + 6)) : 1 - landed;
                          return <Chip key={n} n={n} hot={hot} />;
                        })
                      : null}
                  </span>
                );
              });
            })()}
          </div>
        ) : null}

        {/* C — chips in flight from the cited papers */}
        {flights.map((fl) => {
          const t = ramp(frame, fl.at - 14, fl.at, inOut);
          if (t <= 0 || t >= 1) return null;
          const src = fan.find((p) => p.i === CITED[fl.n]);
          if (!src) return null;
          const x = mix(src.x + 40, 1500, t);
          const y = mix(src.y - 40, 520, t) - Math.sin(t * Math.PI) * 120;
          return (
            <div key={fl.n} style={{ position: "absolute", left: x, top: y, width: 48, height: 48, borderRadius: 12, background: "#c94d1c", color: "#fff", fontFamily: font.sans, fontSize: 26, fontWeight: 800, display: "flex", alignItems: "center", justifyContent: "center", boxShadow: "0 10px 30px rgba(201,77,28,0.5)" }}>
              {fl.n}
            </div>
          );
        })}

        {/* D — the page */}
        {frame >= B[4] + 12 ? (
          (() => {
            const pw = 900;
            const ph = (pw * assets.page.h) / assets.page.w;
            const [x0, y0, x1, y1] = assets.passage;
            const focusY = ((y0 + y1) / 2) * ph;
            const zoom = mix(1, 1.55, dolly);
            return (
              <div
                style={{
                  position: "absolute",
                  left: 930,
                  top: mix(height + 200, 90, pageIn) - focusY * (zoom - 1) * 0.9,
                  width: pw,
                  height: ph,
                  transform: `rotateX(${(1 - pageIn) * 24}deg) scale(${zoom})`,
                  transformOrigin: `50% ${((y0 + y1) / 2) * 100}%`,
                  borderRadius: 8,
                  overflow: "hidden",
                  background: "#fff",
                  boxShadow: look.shadow,
                }}
              >
                <Img src={staticFile("act1/spann-p2.png")} style={{ width: "100%", height: "100%" }} />
                <div style={{ position: "absolute", left: x0 * pw - 10, top: y0 * ph - 8, width: (x1 - x0) * pw * sweep + 20, height: (y1 - y0) * ph + 16, background: "rgba(255,170,60,0.32)", mixBlendMode: "multiply", borderRadius: 6 }} />
                <svg style={{ position: "absolute", left: x0 * pw - 12, top: y0 * ph - 10, overflow: "visible" }} width={(x1 - x0) * pw + 24} height={(y1 - y0) * ph + 20}>
                  <rect x={2} y={2} width={(x1 - x0) * pw + 20} height={(y1 - y0) * ph + 16} rx={8} fill="none" stroke="#c94d1c" strokeWidth={4} pathLength={1} strokeDasharray={1} strokeDashoffset={1 - outline} />
                </svg>
              </div>
            );
          })()
        ) : null}
      </AbsoluteFill>

      {/* Headlines */}
      <Headline words={["Ask", "your", "papers."]} at={B[1] - 30} out={B[2] - 6} look={look} size={120} x={160} y={150} accentLast />
      <Headline words={["It", "searches", "your", "library."]} at={B[2] + 30} out={B[3] - 8} look={look} size={96} x={160} y={150} />
      <Headline words={["Every", "claim,", "cited."]} at={B[3] + 18} out={B[4] + 4} look={look} size={96} x={560} y={150} accentLast maxWidth={1300} />
      <Headline words={["The", "exact", "page."]} at={B[4] + 26} out={B[5] + 40} look={look} size={120} x={160} y={300} accentLast />

      {/* source caption for the page */}
      {frame >= B[4] + 50 ? (
        <div style={{ position: "absolute", left: 164, top: 570, fontFamily: font.sans, color: look.sub, fontSize: 28, fontWeight: 500, opacity: ramp(frame, B[4] + 50, B[4] + 66) }}>
          {SOURCE_NAMES[9]} · page 2
        </div>
      ) : null}

      <Grain strength={look.grain} />
      <Html5Audio src={staticFile("music/track.mp3")} />
    </AbsoluteFill>
  );
};
