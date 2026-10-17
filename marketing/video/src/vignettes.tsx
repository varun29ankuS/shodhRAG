// One small animated scene per chapter, on a frosted stage above the
// headline: it acts out what the headline says (papers fly into the
// library, a citation opens its page, a statement is stamped "no source").
// White on the orange ground, 520×230 viewBox; `frame` counts from the
// chapter's start.
import React from "react";
import { Easing, interpolate, spring } from "remotion";
import type { ShotId } from "./cut";
import { color, font } from "./theme";

export const STAGE = { width: 520, height: 230 } as const;

const ease = Easing.bezier(0.16, 1, 0.3, 1);
const clamp = { extrapolateLeft: "clamp", extrapolateRight: "clamp" } as const;
const W = "#ffffff";
const ORANGE = color.accent;

/** 0 → 1 over `duration` frames from `start`, eased out. */
const ramp = (frame: number, start: number, duration: number) =>
  interpolate(frame, [start, start + duration], [0, 1], { ...clamp, easing: ease });

/** A springy 0 → 1 (with a little overshoot) from `start`. */
const pop = (frame: number, start: number, fps: number) =>
  spring({ frame: frame - start, fps, config: { damping: 13, stiffness: 210, mass: 0.6 } });

const mix = (a: number, b: number, t: number) => a + (b - a) * t;

const stroke = { fill: "none", stroke: W, strokeLinecap: "round", strokeLinejoin: "round" } as const;

/** A path drawn on as `p` goes 0 → 1. */
const Draw: React.FC<{ d: string; p: number; w?: number; o?: number; c?: string }> = ({ d, p, w = 3, o = 1, c = W }) => (
  <path d={d} {...stroke} stroke={c} strokeWidth={w} opacity={o} pathLength={1} strokeDasharray={1} strokeDashoffset={1 - p} />
);

/** A small label in the film's typeface. */
const Label: React.FC<{ x: number; y: number; children: React.ReactNode; size?: number; o?: number; fill?: string; anchor?: "start" | "middle" | "end"; weight?: number }> = ({
  x,
  y,
  children,
  size = 13,
  o = 1,
  fill = W,
  anchor = "start",
  weight = 600,
}) => (
  <text x={x} y={y} fontFamily={font.sans} fontSize={size} fontWeight={weight} fill={fill} opacity={o} textAnchor={anchor}>
    {children}
  </text>
);

/** A citation chip: a white rounded square with an orange number. */
const Chip: React.FC<{ x: number; y: number; n: number | string; s?: number; o?: number }> = ({ x, y, n, s = 1, o = 1 }) => (
  <g transform={`translate(${x} ${y}) scale(${s})`} opacity={o}>
    <rect x={-11} y={-11} width={22} height={22} rx={6} fill={W} />
    <Label x={0} y={5} size={13} fill={ORANGE} anchor="middle" weight={800}>
      {n}
    </Label>
  </g>
);

/** A paper: a page outline with a folded corner and a few lines. */
const Paper: React.FC<{ x: number; y: number; s?: number; o?: number; p?: number; fill?: boolean }> = ({ x, y, s = 1, o = 1, p = 1, fill = false }) => (
  <g transform={`translate(${x} ${y}) scale(${s})`} opacity={o}>
    {fill ? <path d="M0 0h26l12 12v42H0z" fill={W} fillOpacity={0.18} /> : null}
    <Draw d="M0 0h26l12 12v42H0zM26 0v12h12" p={p} w={2.5} />
    <Draw d="M7 22h24M7 30h24M7 38h16" p={p} w={2} o={0.7} />
  </g>
);

/** Lines of text appearing left to right, like a streamed answer. */
const Lines: React.FC<{ x: number; y: number; widths: number[]; gap: number; frame: number; start: number; each: number; o?: number }> = ({
  x,
  y,
  widths,
  gap,
  frame,
  start,
  each,
  o = 0.85,
}) => (
  <>
    {widths.map((w, i) => {
      const p = ramp(frame, start + i * each, each + 6);
      return <rect key={i} x={x} y={y + i * gap - 3} width={w * p} height={6} rx={3} fill={W} opacity={o} />;
    })}
  </>
);

type Scene = React.FC<{ frame: number; fps: number }>;

const Hero: Scene = ({ frame, fps }) => {
  const papers = [0, 1, 2].map((i) => pop(frame, i * 5, fps));
  const card = ramp(frame, 18, 20);
  const link = [0, 1, 2].map((i) => ramp(frame, 62 + i * 8, 22));
  return (
    <>
      {papers.map((p, i) => (
        <Paper key={i} x={mix(-60, 40, p)} y={34 + i * 58} s={0.95} o={Math.min(1, p * 1.4)} fill />
      ))}
      <rect x={250} y={36} width={232} height={160} rx={16} fill={W} fillOpacity={0.14 * card} />
      <Draw d="M266 36h200a16 16 0 0 1 16 16v128a16 16 0 0 1-16 16H266a16 16 0 0 1-16-16V52a16 16 0 0 1 16-16z" p={card} w={2.5} />
      <Lines x={272} y={68} widths={[150, 180, 120, 170]} gap={26} frame={frame} start={30} each={8} />
      {[0, 1, 2].map((i) => (
        <g key={i}>
          <Draw d={`M82 ${62 + i * 58} C 160 ${62 + i * 58}, 200 ${70 + i * 26}, 250 ${70 + i * 26}`} p={link[i]} w={2} o={0.65} />
          <Chip x={450} y={66 + i * 26} n={i + 1} s={pop(frame, 70 + i * 8, fps)} />
        </g>
      ))}
    </>
  );
};

const Library: Scene = ({ frame, fps }) => {
  const folder = ramp(frame, 0, 16);
  const tiles = Array.from({ length: 12 }, (_, i) => {
    const t = ramp(frame, 14 + i * 5, 22);
    const col = i % 6;
    const row = Math.floor(i / 6);
    const tx = 206 + col * 50;
    const ty = 48 + row * 78;
    return { x: mix(70, tx, t), y: mix(120, ty, t), s: mix(0.4, 0.82, t), o: Math.min(1, t * 2), tick: pop(frame, 30 + i * 5, fps), tx, ty };
  });
  const bar = ramp(frame, 20, 70);
  return (
    <>
      <Draw d="M24 92h34l10 12h50a8 8 0 0 1 8 8v64a8 8 0 0 1-8 8H32a8 8 0 0 1-8-8z" p={folder} w={3} />
      {tiles.map((t, i) => (
        <g key={i}>
          <Paper x={t.x} y={t.y} s={t.s} o={t.o} fill />
          <circle cx={t.tx + 34} cy={t.ty + 50} r={8 * t.tick} fill={W} />
          <Draw d={`M${t.tx + 30} ${t.ty + 50}l3 3 6-6`} p={t.tick} w={2} c={ORANGE} />
        </g>
      ))}
      <rect x={206} y={212} width={296} height={4} rx={2} fill={W} opacity={0.25} />
      <rect x={206} y={212} width={296 * bar} height={4} rx={2} fill={W} />
      <Label x={24} y={216} size={14} o={0.9}>
        {`${Math.round(bar * 12)} of 12 indexed`}
      </Label>
    </>
  );
};

const Ask: Scene = ({ frame, fps }) => {
  const q = "How does SPANN keep search fast?";
  const typed = Math.floor(interpolate(frame, [10, 70], [0, q.length], clamp));
  const caret = Math.floor(frame / 8) % 2 === 0 ? 1 : 0;
  const send = pop(frame, 76, fps);
  const rays = [0, 1, 2].map((i) => ramp(frame, 84 + i * 5, 18));
  const papers = [0, 1, 2].map((i) => pop(frame, 92 + i * 5, fps));
  return (
    <>
      <rect x={20} y={150} width={480} height={56} rx={28} fill={W} />
      <Label x={46} y={184} size={18} fill="#3f3f46" weight={500}>
        {q.slice(0, typed)}
      </Label>
      <rect x={46 + typed * 8.6} y={168} width={2} height={22} fill={ORANGE} opacity={typed < q.length ? caret : 0} />
      <circle cx={470} cy={178} r={18} fill={ORANGE} transform={`translate(470 178) scale(${0.85 + 0.15 * send}) translate(-470 -178)`} />
      <Draw d="M470 186v-15M463 177l7-7 7 7" p={1} w={2.5} />
      {[0, 1, 2].map((i) => (
        <g key={i}>
          <Draw d={`M260 148 L ${110 + i * 150} 92`} p={rays[i]} w={2} o={0.6} />
          <Paper x={92 + i * 150} y={24} s={papers[i]} o={Math.min(1, papers[i] * 1.4)} fill />
        </g>
      ))}
    </>
  );
};

const Answer: Scene = ({ frame, fps }) => {
  const card = ramp(frame, 0, 18);
  const rows = [
    { y: 64, w: 240 },
    { y: 102, w: 280 },
    { y: 140, w: 210 },
    { y: 178, w: 260 },
  ];
  return (
    <>
      <rect x={16} y={30} width={360} height={176} rx={16} fill={W} fillOpacity={0.12 * card} />
      <Draw d="M32 30h328a16 16 0 0 1 16 16v144a16 16 0 0 1-16 16H32a16 16 0 0 1-16-16V46a16 16 0 0 1 16-16z" p={card} w={2.5} />
      {rows.map((r, i) => {
        const line = ramp(frame, 14 + i * 14, 18);
        const fly = ramp(frame, 26 + i * 14, 18);
        const fromX = 470;
        const fromY = 60 + i * 42;
        return (
          <g key={i}>
            <rect x={36} y={r.y - 3} width={r.w * line} height={7} rx={3.5} fill={W} opacity={0.88} />
            <Paper x={440} y={fromY - 26} s={0.7} o={0.55 + 0.45 * pop(frame, 6 + i * 6, fps)} fill />
            <Chip x={mix(fromX, 36 + r.w + 20, fly)} y={mix(fromY, r.y, fly)} n={i + 1} s={fly > 0 ? 1 : 0} />
          </g>
        );
      })}
    </>
  );
};

const Cite: Scene = ({ frame, fps }) => {
  const press = frame > 22 && frame < 28 ? 0.86 : 1;
  const chip = pop(frame, 4, fps);
  const cursor = ramp(frame, 0, 20);
  const page = ramp(frame, 26, 22);
  const lines = [0, 1, 2, 3, 4, 5, 6].map((i) => ramp(frame, 34 + i * 3, 12));
  const sweep = ramp(frame, 60, 26);
  const outline = ramp(frame, 82, 18);
  const link = ramp(frame, 24, 20);
  return (
    <>
      <rect x={20} y={90} width={150} height={8} rx={4} fill={W} opacity={0.6} />
      <rect x={20} y={114} width={110} height={8} rx={4} fill={W} opacity={0.6} />
      <Chip x={152} y={118} n={2} s={chip * press} />
      <Draw d="M166 118 C 220 118, 240 118, 290 118" p={link} w={2.5} o={0.8} />
      <g transform={`translate(${mix(560, 300, page)} 0)`} opacity={Math.min(1, page * 1.6)}>
        <rect x={0} y={6} width={190} height={206} rx={10} fill={W} />
        {lines.map((p, i) => (
          <rect key={i} x={16} y={34 + i * 24} width={(i % 3 === 2 ? 110 : 156) * p} height={7} rx={3.5} fill="#d4d4d8" />
        ))}
        <rect x={10} y={96} width={170 * sweep} height={58} rx={6} fill={ORANGE} opacity={0.22} />
        <rect x={10} y={96} width={170} height={58} rx={6} fill="none" stroke={ORANGE} strokeWidth={3} pathLength={1} strokeDasharray={1} strokeDashoffset={1 - outline} />
        <Label x={95} y={232} size={13} anchor="middle" o={outline}>
          Cited passage · page 2
        </Label>
      </g>
      <g transform={`translate(${mix(100, 150, cursor)} ${mix(170, 124, cursor)})`}>
        <path d="M0 0v18l5-4 3 7 3-1-3-7h7z" fill={W} stroke="#18181b" strokeWidth={1} strokeLinejoin="round" opacity={page < 0.5 ? 1 : 1 - (page - 0.5) * 2} />
      </g>
    </>
  );
};

const Ground: Scene = ({ frame, fps }) => {
  const rows = [0, 1, 2];
  return (
    <>
      {rows.map((i) => {
        const y = 46 + i * 62;
        const line = ramp(frame, i * 6, 18);
        const verdict = pop(frame, 30 + i * 16, fps);
        const flagged = i === 1;
        return (
          <g key={i}>
            <rect x={20} y={y - 4} width={300 * line} height={8} rx={4} fill={W} opacity={0.85} />
            <rect x={20} y={y + 14} width={220 * line} height={8} rx={4} fill={W} opacity={0.55} />
            {flagged ? (
              <g transform={`translate(390 ${y + 8}) scale(${verdict}) rotate(${(1 - verdict) * -12})`} opacity={Math.min(1, verdict * 1.5)}>
                <rect x={-62} y={-17} width={124} height={34} rx={17} fill={W} />
                <Label x={0} y={6} size={15} fill={ORANGE} anchor="middle" weight={800}>
                  ⚠ no source
                </Label>
              </g>
            ) : (
              <g opacity={Math.min(1, verdict * 1.5)}>
                <circle cx={362} cy={y + 8} r={16 * verdict} fill={W} />
                <Draw d={`M354 ${y + 8}l6 6 11-12`} p={verdict} w={3} c={ORANGE} />
              </g>
            )}
          </g>
        );
      })}
      <Label x={20} y={222} size={14} o={ramp(frame, 80, 16)}>
        2 of 3 statements supported · 1 flagged
      </Label>
    </>
  );
};

const Deeper: Scene = ({ frame, fps }) => {
  const lines = [0, 1, 2, 3, 4].map((i) => ramp(frame, i * 3, 14));
  const mark = ramp(frame, 22, 16);
  const panel = ramp(frame, 44, 22);
  const q = pop(frame, 62, fps);
  const a = [0, 1, 2].map((i) => ramp(frame, 74 + i * 7, 14));
  return (
    <>
      {lines.map((p, i) => (
        <rect key={i} x={16} y={40 + i * 34} width={(i === 2 ? 230 : 270) * p} height={8} rx={4} fill={W} opacity={0.6} />
      ))}
      <rect x={96} y={104} width={120 * mark} height={20} rx={5} fill={W} opacity={0.95} />
      <Label x={100} y={119} size={13} fill={ORANGE} weight={700} o={mark}>
        balanced clustering
      </Label>
      <g transform={`translate(${mix(520, 0, panel)} 0)`}>
        <rect x={300} y={14} width={204} height={204} rx={14} fill={W} fillOpacity={0.16} stroke={W} strokeOpacity={0.5} strokeWidth={1.5} />
        <Label x={316} y={40} size={12} o={0.85} weight={700}>
          ASK ABOUT THIS
        </Label>
        <g transform={`translate(490 66) scale(${q}) translate(-490 -66)`}>
          <rect x={348} y={52} width={142} height={28} rx={14} fill={W} />
          <Label x={419} y={71} size={12} fill={ORANGE} anchor="middle" weight={700}>
            Why does this help?
          </Label>
        </g>
        {a.map((p, i) => (
          <rect key={i} x={316} y={104 + i * 22} width={(i === 2 ? 100 : 168) * p} height={7} rx={3.5} fill={W} opacity={0.85} />
        ))}
      </g>
    </>
  );
};

const Visual: Scene = ({ frame, fps }) => {
  const nodes = [
    { x: 20, y: 24, w: 112, label: "Query" },
    { x: 196, y: 24, w: 150, label: "Centroids · memory" },
    { x: 196, y: 150, w: 150, label: "Posting lists · disk" },
    { x: 392, y: 88, w: 112, label: "Results" },
  ];
  const edges = [
    "M132 46 H196",
    "M271 68 V150",
    "M346 172 C 380 172, 380 132, 392 120",
    "M346 46 C 380 46, 380 90, 392 100",
  ];
  return (
    <>
      {edges.map((d, i) => (
        <Draw key={i} d={d} p={ramp(frame, 20 + i * 12, 16)} w={2.5} o={0.85} />
      ))}
      {nodes.map((n, i) => {
        const p = pop(frame, i * 9, fps);
        return (
          <g key={n.label} transform={`translate(${n.x + n.w / 2} ${n.y + 22}) scale(${p}) translate(${-(n.x + n.w / 2)} ${-(n.y + 22)})`} opacity={Math.min(1, p * 1.5)}>
            <rect x={n.x} y={n.y} width={n.w} height={44} rx={10} fill={W} />
            <Label x={n.x + n.w / 2} y={n.y + 27} size={13} fill={ORANGE} anchor="middle" weight={700}>
              {n.label}
            </Label>
          </g>
        );
      })}
    </>
  );
};

const Graph: Scene = ({ frame, fps }) => {
  const turn = frame / 140;
  const library = Array.from({ length: 6 }, (_, i) => {
    const a = (i / 6) * Math.PI * 2 + turn;
    return { x: 260 + Math.cos(a) * 92, y: 115 + Math.sin(a) * 62 };
  });
  const cited = Array.from({ length: 22 }, (_, i) => {
    const a = (i / 22) * Math.PI * 2 - turn * 0.6;
    const r = 1 + (i % 3) * 0.12;
    return { x: 260 + Math.cos(a) * 200 * r * 0.95, y: 115 + Math.sin(a) * 92 * r, owner: i % 6 };
  });
  return (
    <>
      {cited.map((c, i) => {
        const p = ramp(frame, 24 + i * 2, 14);
        const o = library[c.owner];
        return (
          <g key={`c${i}`}>
            <line x1={o.x} y1={o.y} x2={mix(o.x, c.x, p)} y2={mix(o.y, c.y, p)} stroke={W} strokeWidth={1.2} opacity={0.4} />
            <circle cx={c.x} cy={c.y} r={4 * p} fill={W} opacity={0.7} />
          </g>
        );
      })}
      {library.map((n, i) => {
        const next = library[(i + 2) % 6];
        const p = ramp(frame, 10 + i * 4, 16);
        return <line key={`l${i}`} x1={n.x} y1={n.y} x2={mix(n.x, next.x, p)} y2={mix(n.y, next.y, p)} stroke={W} strokeWidth={2} opacity={0.75} />;
      })}
      {library.map((n, i) => (
        <circle key={`n${i}`} cx={n.x} cy={n.y} r={11 * pop(frame, i * 4, fps)} fill={W} stroke={ORANGE} strokeWidth={3} />
      ))}
    </>
  );
};

const Workspaces: Scene = ({ frame, fps }) => {
  const cards = [0, 1, 2].map((i) => pop(frame, i * 6, fps));
  const items = [0, 1, 2, 3].map((i) => ramp(frame, 30 + i * 10, 20));
  const glow = ramp(frame, 28, 16);
  return (
    <>
      {cards.map((p, i) => {
        const x = 20 + i * 168;
        const active = i === 1;
        return (
          <g key={i} transform={`translate(${x + 74} 130) scale(${p}) translate(${-(x + 74)} -130)`} opacity={Math.min(1, p * 1.4)}>
            <rect x={x} y={70} width={148} height={140} rx={14} fill={W} fillOpacity={active ? 0.22 + 0.1 * glow : 0.1} stroke={W} strokeOpacity={active ? 0.9 : 0.4} strokeWidth={active ? 2.5 : 1.5} />
            <Label x={x + 14} y={96} size={13} weight={700} o={active ? 1 : 0.7}>
              {["Thesis", "Vector search", "Reading group"][i]}
            </Label>
          </g>
        );
      })}
      {items.map((p, i) => {
        const kinds = ["Sources", "Chats", "Memory", "Instructions"];
        const tx = 202;
        const ty = 110 + i * 24;
        return (
          <g key={i} opacity={Math.min(1, p * 2)}>
            <rect x={mix(tx + 20, tx, p)} y={mix(14, ty, p)} width={110} height={18} rx={9} fill={W} />
            <Label x={mix(tx + 20, tx, p) + 55} y={mix(14, ty, p) + 13} size={11} fill={ORANGE} anchor="middle" weight={700}>
              {kinds[i]}
            </Label>
          </g>
        );
      })}
    </>
  );
};

const Tasks: Scene = ({ frame, fps }) => {
  const grid = ramp(frame, 0, 18);
  const drop = ramp(frame, 22, 22);
  const tick = pop(frame, 50, fps);
  const cells = Array.from({ length: 21 }, (_, i) => ({ x: 20 + (i % 7) * 70, y: 40 + Math.floor(i / 7) * 58 }));
  const target = cells[9];
  return (
    <>
      {cells.map((c, i) => (
        <rect key={i} x={c.x} y={c.y} width={62} height={50} rx={8} fill={W} fillOpacity={i === 9 ? 0.2 * drop : 0.08} stroke={W} strokeOpacity={0.35 * grid} strokeWidth={1.2} />
      ))}
      <g transform={`translate(0 ${mix(-90, 0, drop)})`} opacity={Math.min(1, drop * 2)}>
        <rect x={target.x - 30} y={target.y + 12} width={122} height={26} rx={13} fill={W} />
        <circle cx={target.x - 16} cy={target.y + 25} r={7} fill="none" stroke={ORANGE} strokeWidth={2} />
        <Draw d={`M${target.x - 20} ${target.y + 25}l3 3 6-6`} p={tick} w={2} c={ORANGE} />
        <Label x={target.x - 4} y={target.y + 30} size={12} fill={ORANGE} weight={700}>
          Re-read SPANN
        </Label>
      </g>
    </>
  );
};

const Create: Scene = ({ frame, fps }) => {
  const keys = ["Ctrl", "Shift", "N"];
  const menu = pop(frame, 34, fps);
  const items = ["New chat", "New workspace", "New task", "New event"];
  const pick = Math.min(2, Math.floor(interpolate(frame, [52, 76], [0, 3], clamp)));
  return (
    <>
      {keys.map((k, i) => {
        const p = pop(frame, i * 6, fps);
        const down = frame >= 22 + i * 2 && frame < 30 + i * 2 ? 4 : 0;
        const w = k === "N" ? 46 : 74;
        const x = [20, 106, 192][i];
        return (
          <g key={k} transform={`translate(0 ${(1 - p) * 20 + down})`} opacity={Math.min(1, p * 1.5)}>
            <rect x={x} y={88} width={w} height={46} rx={10} fill={W} />
            <rect x={x} y={130} width={w} height={6} rx={3} fill={W} opacity={0.45} />
            <Label x={x + w / 2} y={118} size={17} fill={ORANGE} anchor="middle" weight={800}>
              {k}
            </Label>
          </g>
        );
      })}
      <g transform={`translate(280 20) scale(${menu}) translate(-280 -20)`} opacity={Math.min(1, menu * 1.5)}>
        <rect x={280} y={20} width={220} height={190} rx={14} fill={W} />
        {items.map((it, i) => (
          <g key={it}>
            {i === pick ? <rect x={290} y={34 + i * 42} width={200} height={34} rx={8} fill={ORANGE} opacity={0.14} /> : null}
            <Label x={306} y={56 + i * 42} size={15} fill={i === pick ? ORANGE : "#3f3f46"} weight={i === pick ? 750 : 550}>
              {it}
            </Label>
          </g>
        ))}
      </g>
    </>
  );
};

const Palette: Scene = ({ frame, fps }) => {
  const bar = pop(frame, 0, fps);
  const q = "work";
  const typed = Math.floor(interpolate(frame, [14, 34], [0, q.length], clamp));
  const rows = ["Workspaces", "New workspace", "Vector search survey"];
  const sel = Math.min(2, Math.floor(interpolate(frame, [60, 90], [0, 3], clamp)));
  return (
    <g transform={`translate(260 115) scale(${0.9 + 0.1 * bar}) translate(-260 -115)`} opacity={Math.min(1, bar * 1.5)}>
      <rect x={30} y={14} width={460} height={202} rx={16} fill={W} />
      <circle cx={58} cy={44} r={8} fill="none" stroke="#71717a" strokeWidth={2} />
      <path d="M64 50l6 6" stroke="#71717a" strokeWidth={2} strokeLinecap="round" />
      <Label x={82} y={50} size={17} fill="#18181b" weight={550}>
        {q.slice(0, typed)}
      </Label>
      <rect x={30} y={70} width={460} height={1} fill="#e4e4e7" />
      {rows.map((r, i) => {
        const p = ramp(frame, 36 + i * 6, 14);
        return (
          <g key={r} opacity={p} transform={`translate(${(1 - p) * 14} 0)`}>
            {i === sel ? <rect x={42} y={82 + i * 42} width={436} height={36} rx={8} fill={ORANGE} opacity={0.12} /> : null}
            <Label x={58} y={105 + i * 42} size={15} fill={i === sel ? ORANGE : "#3f3f46"} weight={i === sel ? 750 : 550}>
              {r}
            </Label>
          </g>
        );
      })}
    </g>
  );
};

const Models: Scene = ({ frame, fps }) => {
  const opts = ["Subscription", "Your key", "On this computer"];
  const cards = [0, 1, 2].map((i) => pop(frame, i * 6, fps));
  const pos = interpolate(frame, [36, 60, 84, 108], [0, 1, 1, 2], { ...clamp, easing: ease });
  const lock = pop(frame, 112, fps);
  return (
    <>
      {opts.map((o, i) => (
        <g key={o} opacity={Math.min(1, cards[i] * 1.5)} transform={`translate(${20 + i * 166 + 76} 100) scale(${cards[i]}) translate(${-(20 + i * 166 + 76)} -100)`}>
          <rect x={20 + i * 166} y={50} width={152} height={100} rx={14} fill={W} fillOpacity={0.12} stroke={W} strokeOpacity={0.45} strokeWidth={1.5} />
          <Label x={20 + i * 166 + 76} y={106} size={14} anchor="middle" weight={700}>
            {o}
          </Label>
        </g>
      ))}
      <rect x={20 + pos * 166 - 4} y={46} width={160} height={108} rx={17} fill="none" stroke={W} strokeWidth={3.5} />
      <g transform={`translate(428 190) scale(${lock})`} opacity={lock}>
        <rect x={-34} y={-14} width={68} height={28} rx={14} fill={W} />
        <Label x={0} y={5} size={12} fill={ORANGE} anchor="middle" weight={800}>
          offline
        </Label>
      </g>
    </>
  );
};

const Activity: Scene = ({ frame, fps }) => {
  const blocks = [0, 1, 2, 3, 4];
  const seal = pop(frame, 70, fps);
  return (
    <>
      {blocks.map((i) => {
        const p = pop(frame, i * 9, fps);
        const x = 20 + i * 98;
        const link = ramp(frame, 8 + i * 9, 10);
        return (
          <g key={i}>
            {i > 0 ? <Draw d={`M${x - 20} 100 H${x}`} p={link} w={3} /> : null}
            <g transform={`translate(${x + 39} 100) scale(${p}) translate(${-(x + 39)} -100)`} opacity={Math.min(1, p * 1.5)}>
              <rect x={x} y={70} width={78} height={60} rx={10} fill={W} fillOpacity={0.16} stroke={W} strokeWidth={2} />
              <rect x={x + 12} y={86} width={54} height={6} rx={3} fill={W} opacity={0.8} />
              <rect x={x + 12} y={100} width={38} height={6} rx={3} fill={W} opacity={0.55} />
              <Label x={x + 12} y={122} size={10} o={0.8} weight={700}>
                {`#${String(41 + i)}`}
              </Label>
            </g>
          </g>
        );
      })}
      <g transform={`translate(260 184) scale(${seal}) rotate(${(1 - seal) * -10})`} opacity={Math.min(1, seal * 1.5)}>
        <rect x={-92} y={-20} width={184} height={40} rx={20} fill={W} />
        <Label x={0} y={6} size={16} fill={ORANGE} anchor="middle" weight={800}>
          ✓ 57 events verified
        </Label>
      </g>
    </>
  );
};

const SCENES: Record<Exclude<ShotId, "close">, Scene> = {
  hero: Hero,
  library: Library,
  ask: Ask,
  answer: Answer,
  cite: Cite,
  ground: Ground,
  deeper: Deeper,
  visual: Visual,
  graph: Graph,
  workspaces: Workspaces,
  tasks: Tasks,
  create: Create,
  palette: Palette,
  models: Models,
  activity: Activity,
};

/** The chapter's scene at `frame` (frames since the chapter started). */
export const Vignette: React.FC<{ id: Exclude<ShotId, "close">; frame: number; fps: number }> = ({ id, frame, fps }) => {
  const Scene = SCENES[id];
  return (
    <svg width={STAGE.width} height={STAGE.height} viewBox={`0 0 ${STAGE.width} ${STAGE.height}`} style={{ overflow: "visible" }}>
      <Scene frame={frame} fps={fps} />
    </svg>
  );
};
