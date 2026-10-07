/**
 * Deterministic layout of a checked diagram (`diagramSpec.ts`): the same
 * spec always gives the same geometry, with no measuring of the DOM (text
 * width is estimated from character counts) and no randomness.
 *
 * Style follows diagram-design (MIT): orthogonal connectors with rounded
 * elbows, attach points spread along a box edge, labels on an opaque mask,
 * a 4px grid, low density.
 *
 * - flow / architecture: a layered DAG (longest-path layers after cycles are
 *   broken in declaration order; two barycenter sweeps order each layer, with
 *   ties kept in declaration order). Flow runs top to bottom, architecture
 *   left to right with its groups kept together and framed.
 * - sequence: participants across the top, one row per message.
 * - layers: a stack of bands, the first on top.
 * - timeline: nodes in order along an axis, alternating above and below.
 * - quadrant: a square with a point per node.
 * - loop: nodes clockwise on a circle from the top, every cycle listed with
 *   its R or B label.
 *
 * Pure module, unit-tested with Node (`app/tests/visualDiagram.test.ts`).
 */

import { feedbackLoops, isDashed } from './diagramSpec.ts';
import type { DiagramSpec, FeedbackLoop } from './diagramSpec.ts';

export const PAD = 24;
const CHAR_W = 7;
const LINE_H = 16;
const SUB_H = 14;
const WRAP = 22;
const MIN_BOX_W = 96;
const MAX_BOX_W = 200;
const GAP_TB = 64;
const GAP_LR = 96;
const GAP_IN_LAYER = 32;
const CORNER = 8;
const LABEL_PAD = 6;

export interface Box {
  /** Index into `spec.nodes`. */
  node: number;
  x: number;
  y: number;
  w: number;
  h: number;
  lines: string[];
  sub: string | null;
  /** Drawn as a dot with the label beside it (quadrant). */
  dot: boolean;
}

export interface Label {
  x: number;
  y: number;
  w: number;
  text: string;
}

export interface Wire {
  /** Index into `spec.edges`. */
  edge: number;
  d: string;
  dashed: boolean;
  arrow: boolean;
  label: Label | null;
  /** Where a loop's + or - is written. */
  sign: { x: number; y: number; text: string } | null;
}

export interface Frame {
  label: string;
  x: number;
  y: number;
  w: number;
  h: number;
}

export interface Rule {
  x1: number;
  y1: number;
  x2: number;
  y2: number;
  dashed: boolean;
}

export interface Caption {
  x: number;
  y: number;
  text: string;
  anchor: 'start' | 'middle' | 'end';
  /** mono: small technical text (times, axis ends, loop names). */
  mono: boolean;
}

export interface DiagramDrawing {
  width: number;
  height: number;
  boxes: Box[];
  wires: Wire[];
  frames: Frame[];
  rules: Rule[];
  captions: Caption[];
  loops: FeedbackLoop[];
}

const snap = (v: number) => Math.round(v / 4) * 4;
const round = (v: number) => Math.round(v * 10) / 10;

/** The label in at most two lines of about `WRAP` characters. */
export function wrapLabel(label: string, width = WRAP): string[] {
  const words = label.split(' ');
  const lines: string[] = [];
  let current = '';
  for (const word of words) {
    const next = current ? `${current} ${word}` : word;
    if (Array.from(next).length <= width || !current) current = next;
    else {
      lines.push(current);
      current = word;
    }
  }
  if (current) lines.push(current);
  if (lines.length <= 2) return lines.map(l => clip(l, width + 6));
  return [lines[0], clip(lines.slice(1).join(' '), width - 1, true)];
}

function clip(text: string, max: number, force = false): string {
  const chars = Array.from(text);
  if (chars.length <= max && !force) return text;
  if (chars.length <= max) return text;
  return `${chars.slice(0, max - 1).join('')}…`;
}

function textWidth(text: string, charW = CHAR_W): number {
  return Array.from(text).length * charW;
}

function makeBox(spec: DiagramSpec, node: number, minW = MIN_BOX_W): Box {
  const n = spec.nodes[node];
  const lines = wrapLabel(n.label);
  const sub = n.kind;
  const widest = Math.max(...lines.map(l => textWidth(l)), sub ? textWidth(sub, 6.6) : 0);
  const w = snap(Math.min(MAX_BOX_W, Math.max(minW, widest + 28)));
  const h = snap(20 + lines.length * LINE_H + (sub ? SUB_H : 0));
  return { node, x: 0, y: 0, w, h, lines, sub, dot: false };
}

/** A path through `points` with rounded corners. */
export function roundedPath(points: readonly [number, number][], radius = CORNER): string {
  const pts = points.filter((p, i) => i === 0 || p[0] !== points[i - 1][0] || p[1] !== points[i - 1][1]);
  if (pts.length === 0) return '';
  let d = `M${round(pts[0][0])} ${round(pts[0][1])}`;
  for (let i = 1; i < pts.length; i++) {
    const [x, y] = pts[i];
    if (i === pts.length - 1) {
      d += ` L${round(x)} ${round(y)}`;
      break;
    }
    const [px, py] = pts[i - 1];
    const [nx, ny] = pts[i + 1];
    const inLen = Math.hypot(x - px, y - py);
    const outLen = Math.hypot(nx - x, ny - y);
    const r = Math.min(radius, inLen / 2, outLen / 2);
    const ax = x - ((x - px) / (inLen || 1)) * r;
    const ay = y - ((y - py) / (inLen || 1)) * r;
    const bx = x + ((nx - x) / (outLen || 1)) * r;
    const by = y + ((ny - y) / (outLen || 1)) * r;
    d += ` L${round(ax)} ${round(ay)} Q${round(x)} ${round(y)} ${round(bx)} ${round(by)}`;
  }
  return d;
}

function edgeLabel(text: string | null, x: number, y: number): Label | null {
  if (!text) return null;
  return { x: round(x), y: round(y), w: snap(textWidth(text, 6.4) + LABEL_PAD * 2), text };
}

/** Spread `count` attach points along a side of `length` starting at `start`. */
function spread(start: number, length: number, count: number, k: number): number {
  return start + (length * (k + 1)) / (count + 1);
}

// ── Layered (flow, architecture) ───────────────────────────────────────────

function layered(spec: DiagramSpec, horizontal: boolean): DiagramDrawing {
  const n = spec.nodes.length;
  const index = new Map(spec.nodes.map((node, i) => [node.id, i]));
  const ends = spec.edges.map(e => [index.get(e.from) ?? 0, index.get(e.to) ?? 0] as const);

  // Back edges: found by a depth-first walk in declaration order.
  const back = new Set<number>();
  const state = new Array(n).fill(0);
  const outEdges: number[][] = spec.nodes.map(() => []);
  ends.forEach(([a], i) => outEdges[a].push(i));
  const visit = (v: number) => {
    state[v] = 1;
    for (const e of outEdges[v]) {
      const w = ends[e][1];
      if (state[w] === 1) back.add(e);
      else if (state[w] === 0) visit(w);
    }
    state[v] = 2;
  };
  for (let v = 0; v < n; v++) if (state[v] === 0) visit(v);

  // Longest-path layers over the forward edges.
  const layer = new Array(n).fill(0);
  for (let pass = 0; pass < n; pass++) {
    let moved = false;
    ends.forEach(([a, b], e) => {
      if (back.has(e) || a === b) return;
      if (layer[b] < layer[a] + 1) {
        layer[b] = layer[a] + 1;
        moved = true;
      }
    });
    if (!moved) break;
  }
  const layerCount = Math.max(...layer) + 1;
  const groupRank = new Map<string, number>();
  spec.nodes.forEach(node => {
    if (node.group && !groupRank.has(node.group)) groupRank.set(node.group, groupRank.size);
  });
  const rankOf = (v: number) => {
    const g = spec.nodes[v].group;
    return g === null ? -1 : groupRank.get(g) ?? -1;
  };
  const layers: number[][] = Array.from({ length: layerCount }, () => []);
  for (let v = 0; v < n; v++) layers[layer[v]].push(v);
  const position = new Array(n).fill(0);
  const reindex = () => layers.forEach(row => row.forEach((v, i) => { position[v] = i; }));
  reindex();
  const neighbours = (v: number, up: boolean) =>
    ends
      .filter(([a, b], e) => !back.has(e) && (up ? b === v && layer[a] < layer[v] : a === v && layer[b] > layer[v]))
      .map(([a, b]) => (up ? a : b));
  for (const up of [true, false]) {
    const order = up ? layers.map((_, i) => i) : layers.map((_, i) => layerCount - 1 - i);
    for (const li of order) {
      const row = layers[li];
      const bary = new Map<number, number>();
      for (const v of row) {
        const ns = neighbours(v, up);
        bary.set(v, ns.length ? ns.reduce((s, w) => s + position[w], 0) / ns.length : position[v]);
      }
      row.sort((a, b) => rankOf(a) - rankOf(b) || (bary.get(a) ?? 0) - (bary.get(b) ?? 0) || a - b);
      reindex();
    }
  }

  const boxes = spec.nodes.map((_, i) => makeBox(spec, i));
  const extent = layers.map(row =>
    horizontal
      ? row.reduce((s, v) => s + boxes[v].h, 0) + GAP_IN_LAYER * Math.max(0, row.length - 1)
      : row.reduce((s, v) => s + boxes[v].w, 0) + GAP_IN_LAYER * Math.max(0, row.length - 1),
  );
  const depth = layers.map(row => Math.max(...row.map(v => (horizontal ? boxes[v].w : boxes[v].h))));
  const widest = Math.max(...extent);
  const groupTop = groupRank.size > 0 ? 24 : 0;
  let along = PAD + groupTop;
  layers.forEach((row, li) => {
    let across = PAD + groupTop + snap((widest - extent[li]) / 2);
    for (const v of row) {
      const b = boxes[v];
      if (horizontal) {
        b.x = along + snap((depth[li] - b.w) / 2);
        b.y = across;
        across += b.h + GAP_IN_LAYER;
      } else {
        b.x = across;
        b.y = along + snap((depth[li] - b.h) / 2);
        across += b.w + GAP_IN_LAYER;
      }
    }
    along += depth[li] + (horizontal ? GAP_LR : GAP_TB);
  });

  // Attach points: per box side, ordered by where the other end is.
  const centre = (v: number) => (horizontal ? boxes[v].y + boxes[v].h / 2 : boxes[v].x + boxes[v].w / 2);
  const outSide: Map<number, number[]> = new Map();
  const inSide: Map<number, number[]> = new Map();
  ends.forEach(([a, b], e) => {
    if (back.has(e) || a === b) return;
    outSide.set(a, [...(outSide.get(a) ?? []), e]);
    inSide.set(b, [...(inSide.get(b) ?? []), e]);
  });
  for (const list of [...outSide.values()]) list.sort((x, y) => centre(ends[x][1]) - centre(ends[y][1]) || x - y);
  for (const list of [...inSide.values()]) list.sort((x, y) => centre(ends[x][0]) - centre(ends[y][0]) || x - y);
  const attach = (v: number, e: number, out: boolean): number => {
    const list = (out ? outSide : inSide).get(v) ?? [e];
    const b = boxes[v];
    return horizontal ? spread(b.y, b.h, list.length, list.indexOf(e)) : spread(b.x, b.w, list.length, list.indexOf(e));
  };

  const maxX = Math.max(...boxes.map(b => b.x + b.w));
  const maxY = Math.max(...boxes.map(b => b.y + b.h));
  let backLane = 0;
  const wires: Wire[] = ends.map(([a, b], e) => {
    const edge = spec.edges[e];
    const s = boxes[a];
    const t = boxes[b];
    const dashed = isDashed(edge.kind) || edge.change === 'removed';
    if (a === b || back.has(e)) {
      // Around the outside: right of the drawing (flow) or below it (architecture).
      backLane += 1;
      if (horizontal) {
        const lane = maxY + 16 + backLane * 12;
        const sx = s.x + s.w / 2 + (a === b ? -12 : 0);
        const tx = t.x + t.w / 2 + (a === b ? 12 : 0);
        const pts: [number, number][] = [[sx, s.y + s.h], [sx, lane], [tx, lane], [tx, t.y + t.h]];
        return { edge: e, d: roundedPath(pts), dashed, arrow: true, label: edgeLabel(edge.label, (sx + tx) / 2, lane + 12), sign: null };
      }
      const lane = maxX + 16 + backLane * 12;
      const sy = s.y + s.h / 2 + (a === b ? -8 : 0);
      const ty = t.y + t.h / 2 + (a === b ? 8 : 0);
      const pts: [number, number][] = [[s.x + s.w, sy], [lane, sy], [lane, ty], [t.x + t.w, ty]];
      return { edge: e, d: roundedPath(pts), dashed, arrow: true, label: edgeLabel(edge.label, lane + 8, (sy + ty) / 2), sign: null };
    }
    if (horizontal) {
      const sy = attach(a, e, true);
      const ty = attach(b, e, false);
      const sx = s.x + s.w;
      const tx = t.x;
      const mx = tx - GAP_LR / 2;
      const pts: [number, number][] = Math.abs(sy - ty) < 1 ? [[sx, sy], [tx, ty]] : [[sx, sy], [mx, sy], [mx, ty], [tx, ty]];
      const label = edgeLabel(edge.label, Math.abs(sy - ty) < 1 ? (sx + tx) / 2 : (sx + mx) / 2, sy - 10);
      return { edge: e, d: roundedPath(pts), dashed, arrow: true, label, sign: null };
    }
    const sx = attach(a, e, true);
    const tx = attach(b, e, false);
    const sy = s.y + s.h;
    const ty = t.y;
    const my = ty - GAP_TB / 2;
    const pts: [number, number][] = Math.abs(sx - tx) < 1 ? [[sx, sy], [tx, ty]] : [[sx, sy], [sx, my], [tx, my], [tx, ty]];
    const label = edgeLabel(edge.label, Math.abs(sx - tx) < 1 ? sx : (sx + tx) / 2, Math.abs(sx - tx) < 1 ? (sy + ty) / 2 : my - 10);
    return { edge: e, d: roundedPath(pts), dashed, arrow: true, label, sign: null };
  });

  const frames: Frame[] = [];
  for (const [group] of groupRank) {
    const members = boxes.filter(b => spec.nodes[b.node].group === group);
    const x = Math.min(...members.map(b => b.x)) - 12;
    const y = Math.min(...members.map(b => b.y)) - 28;
    const x2 = Math.max(...members.map(b => b.x + b.w)) + 12;
    const y2 = Math.max(...members.map(b => b.y + b.h)) + 12;
    frames.push({ label: group, x, y, w: x2 - x, h: y2 - y });
  }

  const labelRight = Math.max(0, ...wires.map(w => (w.label ? w.label.x + w.label.w / 2 : 0)));
  const labelBottom = Math.max(0, ...wires.map(w => (w.label ? w.label.y + 10 : 0)));
  const width = snap(Math.max(maxX + PAD + (horizontal ? 0 : backLane * 12 + (backLane ? 16 : 0)), labelRight + PAD, ...frames.map(f => f.x + f.w + PAD)));
  const height = snap(Math.max(maxY + PAD + (horizontal ? backLane * 12 + (backLane ? 16 : 0) : 0), labelBottom + PAD, ...frames.map(f => f.y + f.h + PAD)));
  return { width, height, boxes, wires, frames, rules: [], captions: [], loops: [] };
}

// ── Sequence ───────────────────────────────────────────────────────────────

function sequence(spec: DiagramSpec): DiagramDrawing {
  const boxes = spec.nodes.map((_, i) => makeBox(spec, i));
  const col = Math.max(...boxes.map(b => b.w), ...spec.edges.map(e => (e.label ? textWidth(e.label, 6.4) : 0))) + 48;
  const headH = Math.max(...boxes.map(b => b.h));
  boxes.forEach((b, i) => {
    b.x = PAD + i * col + snap((col - b.w) / 2);
    b.y = PAD + snap((headH - b.h) / 2);
  });
  const lifeX = (v: number) => PAD + v * col + col / 2;
  const index = new Map(spec.nodes.map((n, i) => [n.id, i]));
  const top = PAD + headH;
  const rowH = 40;
  const wires: Wire[] = spec.edges.map((edge, e) => {
    const a = index.get(edge.from) ?? 0;
    const b = index.get(edge.to) ?? 0;
    const y = top + 32 + e * rowH;
    const dashed = isDashed(edge.kind) || edge.change === 'removed';
    if (a === b) {
      const x = lifeX(a);
      const pts: [number, number][] = [[x, y], [x + 32, y], [x + 32, y + 16], [x, y + 16]];
      return { edge: e, d: roundedPath(pts, 6), dashed, arrow: true, label: edgeLabel(edge.label, x + 40 + (edge.label ? textWidth(edge.label, 6.4) / 2 + LABEL_PAD : 0), y + 8), sign: null };
    }
    const x1 = lifeX(a);
    const x2 = lifeX(b);
    return { edge: e, d: roundedPath([[x1, y], [x2, y]]), dashed, arrow: true, label: edgeLabel(edge.label, (x1 + x2) / 2, y - 10), sign: null };
  });
  const bottom = top + 32 + Math.max(0, spec.edges.length - 1) * rowH + 40;
  const rules: Rule[] = spec.nodes.map((_, i) => ({ x1: lifeX(i), y1: top, x2: lifeX(i), y2: bottom, dashed: true }));
  return {
    width: snap(PAD * 2 + col * spec.nodes.length),
    height: snap(bottom + PAD),
    boxes,
    wires,
    frames: [],
    rules,
    captions: [],
    loops: [],
  };
}

// ── Layers ─────────────────────────────────────────────────────────────────

function layers(spec: DiagramSpec): DiagramDrawing {
  const boxes = spec.nodes.map((_, i) => makeBox(spec, i));
  const w = snap(Math.max(320, ...boxes.map(b => b.w + 120)));
  let y = PAD;
  for (const b of boxes) {
    b.x = PAD;
    b.y = y;
    b.w = w;
    b.h = Math.max(b.h, 48);
    y += b.h + 8;
  }
  return { width: w + PAD * 2, height: snap(y - 8 + PAD), boxes, wires: [], frames: [], rules: [], captions: [], loops: [] };
}

// ── Timeline ───────────────────────────────────────────────────────────────

function timeline(spec: DiagramSpec): DiagramDrawing {
  const boxes = spec.nodes.map((_, i) => makeBox(spec, i));
  const step = Math.max(...boxes.map(b => b.w)) + 24;
  const tallest = Math.max(...boxes.map(b => b.h));
  const axisY = PAD + tallest + 24 + 20;
  const rules: Rule[] = [];
  const captions: Caption[] = [];
  boxes.forEach((b, i) => {
    const cx = PAD + step / 2 + i * step;
    b.x = snap(cx - b.w / 2);
    const above = i % 2 === 0;
    b.y = above ? axisY - 24 - b.h : axisY + 24 + 20;
    rules.push({ x1: cx, y1: above ? b.y + b.h : axisY + 20, x2: cx, y2: above ? axisY : b.y, dashed: false });
    captions.push({ x: cx, y: above ? axisY + 16 : axisY - 8, text: spec.nodes[i].at ?? '', anchor: 'middle', mono: true });
  });
  const width = snap(PAD * 2 + step * boxes.length);
  rules.unshift({ x1: PAD, y1: axisY, x2: width - PAD, y2: axisY, dashed: false });
  const bottom = Math.max(axisY + 24, ...boxes.map(b => b.y + b.h));
  return { width, height: snap(bottom + PAD), boxes, wires: [], frames: [], rules, captions, loops: [] };
}

// ── Quadrant ───────────────────────────────────────────────────────────────

function quadrant(spec: DiagramSpec): DiagramDrawing {
  const size = 360;
  const left = PAD + 8;
  const top = PAD + 20;
  const rules: Rule[] = [
    { x1: left, y1: top, x2: left, y2: top + size, dashed: false },
    { x1: left, y1: top + size, x2: left + size, y2: top + size, dashed: false },
    { x1: left + size / 2, y1: top, x2: left + size / 2, y2: top + size, dashed: true },
    { x1: left, y1: top + size / 2, x2: left + size, y2: top + size / 2, dashed: true },
  ];
  const axes = spec.axes ?? { x: ['', ''], y: ['', ''] };
  const captions: Caption[] = [
    { x: left, y: top + size + 18, text: axes.x[0], anchor: 'start', mono: true },
    { x: left + size, y: top + size + 18, text: `${axes.x[1]} →`, anchor: 'end', mono: true },
    { x: left, y: top - 8, text: `↑ ${axes.y[1]}`, anchor: 'start', mono: true },
    { x: left + 6, y: top + size - 8, text: axes.y[0], anchor: 'start', mono: true },
  ];
  const boxes: Box[] = spec.nodes.map((node, i) => {
    const cx = left + (node.x ?? 0.5) * size;
    const cy = top + (1 - (node.y ?? 0.5)) * size;
    const label = clip(node.label, 28, false);
    return { node: i, x: round(cx - 6), y: round(cy - 6), w: 12, h: 12, lines: [label], sub: null, dot: true };
  });
  const labelRight = Math.max(0, ...boxes.map(b => b.x + 18 + textWidth(b.lines[0])));
  return {
    width: snap(Math.max(left + size + PAD, labelRight + PAD)),
    height: snap(top + size + 28 + PAD),
    boxes,
    wires: [],
    frames: [],
    rules,
    captions,
    loops: [],
  };
}

// ── Loop ───────────────────────────────────────────────────────────────────

/** Where the line from the centre of `b` towards (`x`, `y`) leaves the box. */
function boundary(b: Box, x: number, y: number): [number, number] {
  const cx = b.x + b.w / 2;
  const cy = b.y + b.h / 2;
  const dx = x - cx;
  const dy = y - cy;
  if (dx === 0 && dy === 0) return [cx, cy];
  const sx = dx === 0 ? Infinity : b.w / 2 / Math.abs(dx);
  const sy = dy === 0 ? Infinity : b.h / 2 / Math.abs(dy);
  const s = Math.min(sx, sy);
  return [cx + dx * s, cy + dy * s];
}

function loop(spec: DiagramSpec): DiagramDrawing {
  const boxes = spec.nodes.map((_, i) => makeBox(spec, i));
  const count = boxes.length;
  const widest = Math.max(...boxes.map(b => b.w));
  const radius = Math.max(120, count * 40, count > 1 ? (widest + 32) / (2 * Math.sin(Math.PI / count)) : 0);
  const cx = PAD + widest / 2 + radius;
  const cy = PAD + 24 + radius;
  boxes.forEach((b, k) => {
    const angle = -Math.PI / 2 + (k * 2 * Math.PI) / count;
    b.x = snap(cx + radius * Math.cos(angle) - b.w / 2);
    b.y = snap(cy + radius * Math.sin(angle) - b.h / 2);
  });
  const index = new Map(spec.nodes.map((n, i) => [n.id, i]));
  const wires: Wire[] = spec.edges.map((edge, e) => {
    const a = index.get(edge.from) ?? 0;
    const b = index.get(edge.to) ?? 0;
    const s = boxes[a];
    const t = boxes[b];
    const sign = edge.sign === '-' ? '−' : '+';
    const dashed = isDashed(edge.kind) || edge.change === 'removed';
    if (a === b) {
      const x = s.x + s.w / 2;
      const pts: [number, number][] = [[x - 12, s.y], [x - 12, s.y - 24], [x + 12, s.y - 24], [x + 12, s.y]];
      return { edge: e, d: roundedPath(pts, 6), dashed, arrow: true, label: null, sign: { x: x + 22, y: s.y - 16, text: sign } };
    }
    // Opposite links between the same pair run side by side.
    const twin = spec.edges.some((o, i) => i !== e && index.get(o.from) === b && index.get(o.to) === a);
    const scx = s.x + s.w / 2;
    const scy = s.y + s.h / 2;
    const tcx = t.x + t.w / 2;
    const tcy = t.y + t.h / 2;
    const len = Math.hypot(tcx - scx, tcy - scy) || 1;
    const nx = -(tcy - scy) / len;
    const ny = (tcx - scx) / len;
    const off = twin ? 8 : 0;
    const [x1, y1] = boundary(s, tcx + nx * off, tcy + ny * off);
    const [x2, y2] = boundary(t, scx + nx * off, scy + ny * off);
    const p1: [number, number] = [x1 + nx * off, y1 + ny * off];
    const p2: [number, number] = [x2 + nx * off, y2 + ny * off];
    const at = 0.72;
    const sx = p1[0] + (p2[0] - p1[0]) * at + nx * 12;
    const sy = p1[1] + (p2[1] - p1[1]) * at + ny * 12;
    return { edge: e, d: roundedPath([p1, p2]), dashed, arrow: true, label: edgeLabel(edge.label, (p1[0] + p2[0]) / 2 - nx * 14, (p1[1] + p2[1]) / 2 - ny * 14), sign: { x: round(sx), y: round(sy), text: sign } };
  });
  const loops = feedbackLoops(spec);
  const byId = new Map(spec.nodes.map(n => [n.id, n.label]));
  const listTop = Math.max(...boxes.map(b => b.y + b.h)) + 32;
  const captions: Caption[] = loops.map((l, i) => ({
    x: PAD,
    y: listTop + i * 18,
    text: `${l.polarity}${i + 1}  ${[...l.nodes, l.nodes[0]].map(id => byId.get(id) ?? id).join(' → ')}`,
    anchor: 'start',
    mono: true,
  }));
  if (loops.length === 1) {
    captions.push({ x: cx, y: cy + 6, text: loops[0].polarity, anchor: 'middle', mono: false });
  }
  const widestCaption = Math.max(0, ...captions.map(c => textWidth(c.text, 6.6)));
  return {
    width: snap(Math.max(cx + radius + widest / 2 + PAD + 24, PAD * 2 + widestCaption)),
    height: snap(listTop + loops.length * 18 + PAD),
    boxes,
    wires,
    frames: [],
    rules: [],
    captions,
    loops,
  };
}

/** The geometry of a checked diagram. */
export function layoutDiagram(spec: DiagramSpec): DiagramDrawing {
  switch (spec.layout) {
    case 'flow':
      return layered(spec, false);
    case 'architecture':
      return layered(spec, true);
    case 'sequence':
      return sequence(spec);
    case 'layers':
      return layers(spec);
    case 'timeline':
      return timeline(spec);
    case 'quadrant':
      return quadrant(spec);
    case 'loop':
      return loop(spec);
  }
}
