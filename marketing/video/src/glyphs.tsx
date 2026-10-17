// One line illustration per shot, drawn on as the shot lands (stroke
// draw-on), then kept alive with a small loop. White strokes on the orange
// ground, 120×120 viewBox, round caps and joins throughout.
import React from "react";
import { Easing, interpolate, spring, useVideoConfig } from "remotion";
import type { ShotId } from "./cut";

const ease = Easing.bezier(0.16, 1, 0.3, 1);
const clamp = { extrapolateLeft: "clamp", extrapolateRight: "clamp" } as const;

/** 0 → 1 over `duration` frames from `start`, eased out. */
const draw = (frame: number, start: number, duration: number) =>
  interpolate(frame, [start, start + duration], [0, 1], { ...clamp, easing: ease });

const stroke = {
  fill: "none",
  stroke: "#ffffff",
  strokeWidth: 4,
  strokeLinecap: "round",
  strokeLinejoin: "round",
} as const;

/** A path that draws itself on as `progress` goes 0 → 1. */
const Line: React.FC<{ d: string; progress: number; width?: number; opacity?: number }> = ({ d, progress, width = 4, opacity = 1 }) => (
  <path d={d} {...stroke} strokeWidth={width} opacity={opacity} pathLength={1} strokeDasharray={1} strokeDashoffset={1 - progress} />
);

const Ask: React.FC<{ frame: number }> = ({ frame }) => {
  const bubble = draw(frame, 0, 18);
  const dots = [0, 1, 2].map((i) => {
    const on = draw(frame, 12 + i * 2, 6);
    const bob = Math.sin((frame - i * 5) / 4) * 3 * on;
    return { on, bob };
  });
  return (
    <>
      <Line d="M24 26h72a12 12 0 0 1 12 12v34a12 12 0 0 1-12 12H58l-20 16V84H24a12 12 0 0 1-12-12V38a12 12 0 0 1 12-12z" progress={bubble} />
      {dots.map((d, i) => (
        <circle key={i} cx={42 + i * 18} cy={55 + d.bob} r={5} fill="#ffffff" opacity={d.on} />
      ))}
    </>
  );
};

const Answer: React.FC<{ frame: number; fps: number }> = ({ frame, fps }) => {
  const rows = [
    { y: 36, x2: 70, chip: "1" },
    { y: 60, x2: 84, chip: "2" },
    { y: 84, x2: 62, chip: "3" },
  ];
  return (
    <>
      {rows.map((r, i) => {
        const line = draw(frame, i * 4, 14);
        const pop = spring({ frame: frame - 10 - i * 4, fps, config: { damping: 12, stiffness: 220, mass: 0.6 } });
        const cx = r.x2 + 16;
        return (
          <g key={r.chip}>
            <Line d={`M14 ${r.y}H${r.x2}`} progress={line} />
            <g transform={`translate(${cx} ${r.y}) scale(${pop})`} opacity={Math.min(1, pop * 1.5)}>
              <rect x={-10} y={-10} width={20} height={20} rx={6} fill="#ffffff" />
              <text x={0} y={5} textAnchor="middle" fontSize={14} fontWeight={800} fill="#c94d1c" fontFamily="system-ui, sans-serif">
                {r.chip}
              </text>
            </g>
          </g>
        );
      })}
    </>
  );
};

const Cite: React.FC<{ frame: number }> = ({ frame }) => {
  const page = draw(frame, 0, 18);
  const lines = [40, 52, 64, 76, 88].map((y, i) => ({ y, p: draw(frame, 6 + i * 2, 10) }));
  const sweep = draw(frame, 18, 14);
  const outline = draw(frame, 28, 12);
  return (
    <>
      <Line d="M30 12h44l18 18v78H30z" progress={page} />
      <Line d="M74 12v18h18" progress={page} />
      {lines.map((l) => (
        <Line key={l.y} d={`M40 ${l.y}H82`} progress={l.p} width={3} opacity={0.7} />
      ))}
      <rect x={36} y={57} width={50 * sweep} height={24} rx={4} fill="#ffffff" opacity={0.28} />
      <rect x={36} y={57} width={50} height={24} rx={4} {...stroke} strokeWidth={3} pathLength={1} strokeDasharray={1} strokeDashoffset={1 - outline} />
    </>
  );
};

const Ground: React.FC<{ frame: number }> = ({ frame }) => {
  const shield = draw(frame, 0, 20);
  const check = draw(frame, 16, 12);
  const glow = 0.5 + 0.5 * Math.sin(frame / 6);
  return (
    <>
      <Line d="M60 10l38 14v30c0 26-17 44-38 54C39 98 22 80 22 54V24z" progress={shield} />
      <Line d="M42 60l13 13 25-27" progress={check} width={6} />
      <circle cx={60} cy={58} r={44} fill="none" stroke="#ffffff" strokeWidth={2} opacity={0.18 * check * glow} />
    </>
  );
};

const Deeper: React.FC<{ frame: number }> = ({ frame }) => {
  const lines = [30, 46, 62, 78, 94].map((y, i) => ({ y, p: draw(frame, i * 2, 12) }));
  const lens = draw(frame, 10, 16);
  const travel = interpolate(frame, [18, 60], [-14, 14], { ...clamp, easing: Easing.inOut(Easing.sin) });
  return (
    <>
      {lines.map((l) => (
        <Line key={l.y} d={`M12 ${l.y}H96`} progress={l.p} width={3} opacity={0.55} />
      ))}
      <g transform={`translate(${travel} 0)`}>
        <circle cx={56} cy={56} r={22} fill="#ffffff" opacity={0.18 * lens} />
        <Line d="M56 34a22 22 0 1 1 0 44a22 22 0 1 1 0-44" progress={lens} width={5} />
        <Line d="M72 72l22 22" progress={draw(frame, 20, 10)} width={7} />
      </g>
    </>
  );
};

const Create: React.FC<{ frame: number; fps: number }> = ({ frame, fps }) => {
  const box = draw(frame, 0, 16);
  const plus = draw(frame, 10, 12);
  const keys = ["Ctrl", "⇧", "N"];
  return (
    <>
      <Line d="M38 10h44a12 12 0 0 1 12 12v32a12 12 0 0 1-12 12H38a12 12 0 0 1-12-12V22a12 12 0 0 1 12-12z" progress={box} />
      <Line d="M60 24v28M46 38h28" progress={plus} width={5} />
      {keys.map((k, i) => {
        const pop = spring({ frame: frame - 14 - i * 5, fps, config: { damping: 11, stiffness: 240, mass: 0.6 } });
        const press = Math.max(0, Math.sin(((frame - 30 - i * 3) / 22) * Math.PI)) > 0.96 ? 2 : 0;
        const w = k === "Ctrl" ? 34 : 22;
        const x = [10, 52, 82][i];
        return (
          <g key={k} transform={`translate(0 ${(1 - pop) * 14 + press})`} opacity={pop}>
            <rect x={x} y={82} width={w} height={26} rx={6} fill="#ffffff" opacity={0.95} />
            <text x={x + w / 2} y={100} textAnchor="middle" fontSize={k === "Ctrl" ? 12 : 15} fontWeight={800} fill="#c94d1c" fontFamily="system-ui, sans-serif">
              {k}
            </text>
          </g>
        );
      })}
    </>
  );
};

const Library: React.FC<{ frame: number; fps: number }> = ({ frame, fps }) => {
  const screen = draw(frame, 0, 18);
  const base = draw(frame, 8, 12);
  const papers = [0, 1, 2].map((i) => spring({ frame: frame - 16 - i * 5, fps, config: { damping: 14, stiffness: 160 } }));
  return (
    <>
      <Line d="M22 22h76a6 6 0 0 1 6 6v50H16V28a6 6 0 0 1 6-6z" progress={screen} />
      <Line d="M6 90h108l-8 10H14z" progress={base} />
      {papers.map((p, i) => (
        <rect
          key={i}
          x={38 + i * 12}
          y={36 + (1 - p) * -30}
          width={20}
          height={26}
          rx={3}
          fill="#ffffff"
          opacity={p * (0.55 + i * 0.2)}
        />
      ))}
    </>
  );
};


const Hero: React.FC<{ frame: number }> = ({ frame }) => {
  const left = draw(frame, 0, 18);
  const right = draw(frame, 4, 18);
  const spark = draw(frame, 18, 10);
  const twinkle = 0.75 + 0.25 * Math.sin(frame / 5);
  return (
    <>
      <Line d="M60 34C48 26 32 24 14 26v62c18-2 34 0 46 8z" progress={left} />
      <Line d="M60 34c12-8 28-10 46-8v62c-18-2-34 0-46 8z" progress={right} />
      <Line d="M60 34v62" progress={right} width={3} opacity={0.7} />
      <g transform={`translate(92 14) scale(${spark * twinkle})`} opacity={spark}>
        <path d="M0-12L3-3 12 0 3 3 0 12-3 3-12 0-3-3z" fill="#ffffff" />
      </g>
    </>
  );
};

const Visual: React.FC<{ frame: number; fps: number }> = ({ frame, fps }) => {
  const nodes = [
    { x: 22, y: 30 },
    { x: 60, y: 22 },
    { x: 98, y: 36 },
    { x: 40, y: 86 },
    { x: 84, y: 92 },
  ];
  const edges = [
    [0, 1],
    [1, 2],
    [1, 3],
    [2, 4],
    [3, 4],
  ];
  return (
    <>
      {edges.map(([a, b], i) => (
        <Line key={i} d={`M${nodes[a].x} ${nodes[a].y}L${nodes[b].x} ${nodes[b].y}`} progress={draw(frame, 8 + i * 3, 12)} width={3} opacity={0.8} />
      ))}
      {nodes.map((n, i) => {
        const pop = spring({ frame: frame - i * 3, fps, config: { damping: 12, stiffness: 220, mass: 0.6 } });
        return <rect key={i} x={n.x - 11} y={n.y - 8} width={22} height={16} rx={5} fill="#ffffff" transform={`translate(${n.x} ${n.y}) scale(${pop}) translate(${-n.x} ${-n.y})`} opacity={pop} />;
      })}
    </>
  );
};

const GraphGlyph: React.FC<{ frame: number; fps: number }> = ({ frame, fps }) => {
  const hub = { x: 60, y: 60 };
  const ring = Array.from({ length: 8 }, (_, i) => {
    const a = (i / 8) * Math.PI * 2 + frame / 90;
    return { x: 60 + Math.cos(a) * 42, y: 60 + Math.sin(a) * 42 };
  });
  const hubPop = spring({ frame, fps, config: { damping: 12, stiffness: 200 } });
  return (
    <>
      {ring.map((p, i) => (
        <g key={i}>
          <Line d={`M${hub.x} ${hub.y}L${p.x} ${p.y}`} progress={draw(frame, 4 + i * 2, 10)} width={2.5} opacity={0.6} />
          <circle cx={p.x} cy={p.y} r={i % 3 === 0 ? 7 : 4.5} fill="#ffffff" opacity={draw(frame, 8 + i * 2, 8) * (i % 3 === 0 ? 1 : 0.7)} />
        </g>
      ))}
      <circle cx={hub.x} cy={hub.y} r={11 * hubPop} fill="#ffffff" />
    </>
  );
};

const Workspaces: React.FC<{ frame: number; fps: number }> = ({ frame, fps }) => {
  const layers = [0, 1, 2].map((i) => spring({ frame: frame - i * 5, fps, config: { damping: 14, stiffness: 170 } }));
  return (
    <>
      {layers.map((p, i) => {
        const y = 78 - i * 22 - (1 - p) * 18;
        return (
          <path
            key={i}
            d={`M60 ${y - 18}L104 ${y}L60 ${y + 18}L16 ${y}z`}
            {...stroke}
            fill={i === 2 ? "#ffffff" : "none"}
            fillOpacity={i === 2 ? 0.9 : 0}
            opacity={p}
          />
        );
      })}
    </>
  );
};

const Tasks: React.FC<{ frame: number; fps: number }> = ({ frame }) => {
  const rows = [30, 60, 90];
  return (
    <>
      {rows.map((y, i) => {
        const box = draw(frame, i * 4, 10);
        const tick = draw(frame, 14 + i * 6, 8);
        const line = draw(frame, 4 + i * 4, 12);
        return (
          <g key={y}>
            <rect x={14} y={y - 11} width={22} height={22} rx={6} {...stroke} pathLength={1} strokeDasharray={1} strokeDashoffset={1 - box} />
            {i < 2 ? <Line d={`M19 ${y}l5 5 8-10`} progress={tick} width={4} /> : null}
            <Line d={`M48 ${y}H${i === 1 ? 92 : 104}`} progress={line} width={4} opacity={i < 2 ? 0.55 : 1} />
          </g>
        );
      })}
    </>
  );
};

const Palette: React.FC<{ frame: number; fps: number }> = ({ frame, fps }) => {
  const bar = draw(frame, 0, 16);
  const caret = Math.floor(frame / 8) % 2 === 0 ? 1 : 0;
  const results = [0, 1, 2].map((i) => spring({ frame: frame - 12 - i * 4, fps, config: { damping: 14, stiffness: 200 } }));
  return (
    <>
      <Line d="M18 18h84a10 10 0 0 1 10 10v12a10 10 0 0 1-10 10H18A10 10 0 0 1 8 40V28a10 10 0 0 1 10-10z" progress={bar} />
      <Line d="M30 30a5 5 0 1 1 0 .1M34 34l4 4" progress={bar} width={3} />
      <rect x={46} y={27} width={2.5} height={14} fill="#ffffff" opacity={caret * bar} />
      {results.map((p, i) => (
        <rect key={i} x={14} y={62 + i * 16} width={i === 0 ? 92 : 70 - i * 10} height={10} rx={4} fill="#ffffff" opacity={p * (i === 0 ? 0.95 : 0.5)} transform={`translate(${(1 - p) * 12} 0)`} />
      ))}
    </>
  );
};

const Models: React.FC<{ frame: number }> = ({ frame }) => {
  const chip = draw(frame, 0, 16);
  const core = draw(frame, 12, 10);
  const pins = draw(frame, 6, 14);
  const glow = 0.5 + 0.5 * Math.sin(frame / 6);
  return (
    <>
      <Line d="M34 30h52a4 4 0 0 1 4 4v52a4 4 0 0 1-4 4H34a4 4 0 0 1-4-4V34a4 4 0 0 1 4-4z" progress={chip} />
      <Line d="M46 16v14M60 16v14M74 16v14M46 90v14M60 90v14M74 90v14M16 46h14M16 60h14M16 74h14M90 46h14M90 60h14M90 74h14" progress={pins} width={3.5} />
      <rect x={46} y={46} width={28} height={28} rx={5} fill="#ffffff" opacity={core * (0.7 + 0.3 * glow)} />
    </>
  );
};

const Activity: React.FC<{ frame: number }> = ({ frame }) => {
  const links = [0, 1, 2, 3].map((i) => draw(frame, i * 5, 12));
  const seal = draw(frame, 20, 12);
  return (
    <>
      {links.map((p, i) => (
        <rect key={i} x={20 + i * 20} y={46 + (i % 2) * 6} width={30} height={16} rx={8} {...stroke} strokeWidth={4} pathLength={1} strokeDasharray={1} strokeDashoffset={1 - p} />
      ))}
      <Line d="M44 86l10 10 22-22" progress={seal} width={5} />
    </>
  );
};

/** The shot's illustration at `frame` (frames since the shot started). */
export const Glyph: React.FC<{ id: ShotId; frame: number; size: number }> = ({ id, frame, size }) => {
  const { fps } = useVideoConfig();
  const body = (() => {
    switch (id) {
      case "ask":
        return <Ask frame={frame} />;
      case "answer":
        return <Answer frame={frame} fps={fps} />;
      case "cite":
        return <Cite frame={frame} />;
      case "ground":
        return <Ground frame={frame} />;
      case "deeper":
        return <Deeper frame={frame} />;
      case "create":
        return <Create frame={frame} fps={fps} />;
      case "library":
        return <Library frame={frame} fps={fps} />;
      case "hero":
        return <Hero frame={frame} />;
      case "visual":
        return <Visual frame={frame} fps={fps} />;
      case "graph":
        return <GraphGlyph frame={frame} fps={fps} />;
      case "workspaces":
        return <Workspaces frame={frame} fps={fps} />;
      case "tasks":
        return <Tasks frame={frame} fps={fps} />;
      case "palette":
        return <Palette frame={frame} fps={fps} />;
      case "models":
        return <Models frame={frame} />;
      case "activity":
        return <Activity frame={frame} />;
      case "close":
        return null;
    }
  })();
  return (
    <svg width={size} height={size} viewBox="0 0 120 120" style={{ overflow: "visible" }}>
      {body}
    </svg>
  );
};
