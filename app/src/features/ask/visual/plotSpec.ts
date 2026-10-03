/**
 * Interactive plots written by the agent as ```plot fences: a declarative
 * JSON spec whose formulas are compiled by the safe expression language in
 * `expr.ts` (never run as code).
 *
 *   { "title": "Projectile height", "x": {"min": 0, "max": 4, "label": "t (s)"},
 *     "y": {"min": 0, "max": 25, "label": "h (m)"},
 *     "params": [{"name": "v0", "min": 5, "max": 30, "step": 1, "value": 20, "label": "v0 (m/s)"}],
 *     "items": [{"type": "function", "expr": "v0*x - 0.5*g*x^2"}] }
 *
 * Item types: function (expr of x), parametric (x, y of t over "t": [a, b]),
 * point (x, y; draggable when x or y is a parameter name), vector and segment
 * ("from", "to"), label ("at", "text").
 *
 * Pure module, unit-tested with Node (`app/tests/visualPlot.test.ts`).
 */

import { type Compiled, isValidName, tryCompile } from './expr.ts';
import { pickColor, type VisualColor } from './palette.ts';

/** Largest plot spec rendered, in characters. Also the cap kept with a side thread. */
export const PLOT_MAX_CHARS = 20_000;
export const MAX_PARAMS = 8;
export const MAX_ITEMS = 40;
export const DEFAULT_SAMPLES = 400;
export const MAX_SAMPLES_PER_ITEM = 2_000;
/** Samples across all curves of one plot. */
export const MAX_TOTAL_SAMPLES = 12_000;
/** Largest magnitude of an axis bound. */
const MAX_BOUND = 1e9;
/** Variables reserved for curves; parameters may not use them. */
const CURVE_VARIABLES = new Set(['x', 't']);

export interface PlotAxis {
  min: number;
  max: number;
  label: string;
}

export interface PlotParam {
  name: string;
  min: number;
  max: number;
  step: number;
  value: number;
  label: string;
}

export type Pair = [Compiled, Compiled];

export type PlotItem =
  | { type: 'function'; fn: Compiled; samples: number; color: VisualColor; label: string }
  | { type: 'parametric'; x: Compiled; y: Compiled; t: Pair; samples: number; color: VisualColor; label: string }
  | { type: 'point'; at: Pair; drag: { x: string | null; y: string | null } | null; color: VisualColor; label: string }
  | { type: 'vector' | 'segment'; from: Pair; to: Pair; color: VisualColor; label: string }
  | { type: 'label'; at: Pair; text: string; color: VisualColor };

export interface PlotSpec {
  title: string;
  x: PlotAxis;
  y: PlotAxis;
  /** Same scale on both axes (geometry, trajectories). */
  equal: boolean;
  params: PlotParam[];
  items: PlotItem[];
}

export type PlotParseResult = { ok: true; spec: PlotSpec } | { ok: false; error: string };

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === 'object' && value !== null && !Array.isArray(value);
}

function finite(value: unknown): number | null {
  return typeof value === 'number' && Number.isFinite(value) ? value : null;
}

function text(value: unknown, max = 120): string {
  return typeof value === 'string' ? value.replace(/\s+/g, ' ').trim().slice(0, max) : '';
}

function readAxis(value: unknown, name: string): PlotAxis | string {
  if (!isRecord(value)) return `"${name}" must be an object with "min" and "max".`;
  const min = finite(value.min);
  const max = finite(value.max);
  if (min === null || max === null) return `"${name}.min" and "${name}.max" must be numbers.`;
  if (!(min < max)) return `"${name}.min" must be less than "${name}.max".`;
  if (Math.abs(min) > MAX_BOUND || Math.abs(max) > MAX_BOUND) return `"${name}" bounds are too large.`;
  return { min, max, label: text(value.label, 60) };
}

function readParam(value: unknown, index: number, seen: Set<string>): PlotParam | string {
  if (!isRecord(value)) return `Parameter ${index + 1} must be an object.`;
  const name = typeof value.name === 'string' ? value.name.trim() : '';
  if (!isValidName(name) || CURVE_VARIABLES.has(name)) return `Parameter ${index + 1} needs a "name" like "v0" (letters, digits, _; not x, t, pi, e or a function name).`;
  if (seen.has(name)) return `Parameter "${name}" is defined twice.`;
  const min = finite(value.min);
  const max = finite(value.max);
  if (min === null || max === null || !(min < max)) return `Parameter "${name}" needs numeric "min" < "max".`;
  if (Math.abs(min) > MAX_BOUND || Math.abs(max) > MAX_BOUND) return `Parameter "${name}" bounds are too large.`;
  const rawStep = finite(value.step);
  const step = rawStep !== null && rawStep > 0 && rawStep <= max - min ? rawStep : (max - min) / 100;
  const rawValue = finite(value.value);
  const start = rawValue === null ? min : Math.min(max, Math.max(min, rawValue));
  seen.add(name);
  return { name, min, max, step, value: start, label: text(value.label, 60) || name };
}

/** Parameters shared by plots and simulations. */
export function readParams(value: unknown, reserved: ReadonlySet<string> = CURVE_VARIABLES): PlotParam[] | string {
  if (value === undefined) return [];
  if (!Array.isArray(value)) return '"params" must be a list.';
  if (value.length > MAX_PARAMS) return `At most ${MAX_PARAMS} parameters are supported.`;
  const seen = new Set<string>();
  const params: PlotParam[] = [];
  for (let i = 0; i < value.length; i++) {
    const p = readParam(value[i], i, seen);
    if (typeof p === 'string') return p;
    if (reserved.has(p.name)) return `Parameter "${p.name}" clashes with a variable of the same name.`;
    params.push(p);
  }
  return params;
}

function compile(value: unknown, names: readonly string[], where: string): Compiled | string {
  const r = tryCompile(value, names);
  return r.ok ? r.fn : `${where}: ${r.error}`;
}

function readPair(value: unknown, names: readonly string[], where: string): Pair | string {
  if (!Array.isArray(value) || value.length !== 2) return `${where} must be a pair [x, y].`;
  const a = compile(value[0], names, where);
  if (typeof a === 'string') return a;
  const b = compile(value[1], names, where);
  if (typeof b === 'string') return b;
  return [a, b];
}

function readSamples(value: unknown): number {
  const n = finite(value);
  return n === null ? DEFAULT_SAMPLES : Math.max(16, Math.min(MAX_SAMPLES_PER_ITEM, Math.round(n)));
}

function readItem(value: unknown, index: number, paramNames: readonly string[]): PlotItem | string {
  if (!isRecord(value)) return `Item ${index + 1} must be an object.`;
  const type = typeof value.type === 'string' ? value.type.trim().toLowerCase() : '';
  const where = `Item ${index + 1} (${type || 'no type'})`;
  const color = pickColor(value.color, index);
  const label = text(value.label, 60);
  switch (type) {
    case 'function': {
      const fn = compile(value.expr ?? value.y, [...paramNames, 'x'], where);
      if (typeof fn === 'string') return fn;
      return { type: 'function', fn, samples: readSamples(value.samples), color, label };
    }
    case 'parametric': {
      const names = [...paramNames, 't'];
      const x = compile(value.x, names, `${where} x`);
      if (typeof x === 'string') return x;
      const y = compile(value.y, names, `${where} y`);
      if (typeof y === 'string') return y;
      const t = readPair(value.t ?? [0, 1], paramNames, `${where} "t"`);
      if (typeof t === 'string') return t;
      return { type: 'parametric', x, y, t, samples: readSamples(value.samples), color, label };
    }
    case 'point': {
      const at = readPair([value.x, value.y], paramNames, where);
      if (typeof at === 'string') return at;
      const bare = (v: unknown) => (typeof v === 'string' && paramNames.includes(v.trim()) ? v.trim() : null);
      const drag = value.draggable === true ? { x: bare(value.x), y: bare(value.y) } : null;
      return { type: 'point', at, drag: drag && (drag.x || drag.y) ? drag : null, color, label };
    }
    case 'vector':
    case 'segment': {
      const from = readPair(value.from ?? [0, 0], paramNames, `${where} "from"`);
      if (typeof from === 'string') return from;
      const to = readPair(value.to, paramNames, `${where} "to"`);
      if (typeof to === 'string') return to;
      return { type, from, to, color, label };
    }
    case 'label': {
      const at = readPair(value.at, paramNames, `${where} "at"`);
      if (typeof at === 'string') return at;
      const body = text(value.text, 80);
      if (!body) return `${where} needs "text".`;
      return { type: 'label', at, text: body, color: pickColor(value.color ?? 'text', index) };
    }
    default:
      return `${where}: unknown type; use function, parametric, point, vector, segment or label.`;
  }
}

export function parsePlotSpec(source: string): PlotParseResult {
  if (source.length > PLOT_MAX_CHARS) return { ok: false, error: `The plot spec is larger than ${PLOT_MAX_CHARS / 1000} KB.` };
  let parsed: unknown;
  try {
    parsed = JSON.parse(source);
  } catch {
    return { ok: false, error: 'The plot spec is not valid JSON.' };
  }
  if (!isRecord(parsed)) return { ok: false, error: 'The plot spec must be a JSON object.' };
  const x = readAxis(parsed.x, 'x');
  if (typeof x === 'string') return { ok: false, error: x };
  const y = readAxis(parsed.y, 'y');
  if (typeof y === 'string') return { ok: false, error: y };
  const params = readParams(parsed.params);
  if (typeof params === 'string') return { ok: false, error: params };
  if (!Array.isArray(parsed.items) || parsed.items.length === 0) return { ok: false, error: 'The plot needs a non-empty "items" list.' };
  if (parsed.items.length > MAX_ITEMS) return { ok: false, error: `At most ${MAX_ITEMS} items are supported.` };
  const names = params.map(p => p.name);
  const items: PlotItem[] = [];
  for (let i = 0; i < parsed.items.length; i++) {
    const item = readItem(parsed.items[i], i, names);
    if (typeof item === 'string') return { ok: false, error: item };
    items.push(item);
  }
  // Keep the total sampling work bounded however many curves there are.
  let budget = MAX_TOTAL_SAMPLES;
  for (const item of items) {
    if (item.type === 'function' || item.type === 'parametric') {
      item.samples = Math.max(16, Math.min(item.samples, budget));
      budget = Math.max(16, budget - item.samples);
    }
  }
  return { ok: true, spec: { title: text(parsed.title), x, y, equal: parsed.equal === true, params, items } };
}

/** Parameter values in spec order, with overrides (e.g. kept with a side thread) applied and clamped. */
export function initialValues(params: readonly PlotParam[], overrides: readonly { name: string; value: number }[] = []): number[] {
  return params.map(p => {
    const o = overrides.find(v => v.name === p.name);
    const v = o && Number.isFinite(o.value) ? o.value : p.value;
    return Math.min(p.max, Math.max(p.min, v));
  });
}

/** Snap a value to a parameter's step grid inside its range. */
export function snapParam(param: PlotParam, value: number): number {
  const steps = Math.round((value - param.min) / param.step);
  const snapped = param.min + steps * param.step;
  // Avoid 0.30000000000000004-style tails from the step arithmetic.
  const clean = Number(snapped.toPrecision(12));
  return Math.min(param.max, Math.max(param.min, clean));
}

export type Point = [number, number];

/** A sampled curve: runs of drawable points, broken where the curve is undefined or jumps. */
export type Segments = Point[][];

/**
 * Sample a curve `at(u)` for u in [u0, u1]. Points that are not finite end a
 * run; so does a step that leaps across the whole view (an asymptote, as in
 * tan or 1/x), which would otherwise draw a false vertical line.
 * Coordinates are clamped to a band around the view so paths stay small.
 */
export function sampleCurve(
  at: (u: number) => Point,
  u0: number,
  u1: number,
  samples: number,
  view: { x: PlotAxis; y: PlotAxis },
): Segments {
  const n = Math.max(2, Math.min(MAX_SAMPLES_PER_ITEM, Math.floor(samples)));
  const segments: Segments = [];
  if (!Number.isFinite(u0) || !Number.isFinite(u1)) return segments;
  const xs = view.x.max - view.x.min;
  const ys = view.y.max - view.y.min;
  const band = (v: number, lo: number, span: number) => Math.min(lo + span * 11, Math.max(lo - span * 10, v));
  let run: Point[] = [];
  let prev: Point | null = null;
  const flush = () => {
    if (run.length > 0) segments.push(run);
    run = [];
  };
  for (let i = 0; i < n; i++) {
    const u = u0 + ((u1 - u0) * i) / (n - 1);
    let p: Point;
    try {
      p = at(u);
    } catch {
      p = [NaN, NaN];
    }
    if (!Number.isFinite(p[0]) || !Number.isFinite(p[1])) {
      flush();
      prev = null;
      continue;
    }
    if (prev) {
      const crossesY = (prev[1] > view.y.max && p[1] < view.y.min) || (prev[1] < view.y.min && p[1] > view.y.max);
      const crossesX = (prev[0] > view.x.max && p[0] < view.x.min) || (prev[0] < view.x.min && p[0] > view.x.max);
      const leapY = Math.abs(p[1] - prev[1]) > ys * 4;
      const leapX = Math.abs(p[0] - prev[0]) > xs * 4;
      if (crossesY || crossesX || leapY || leapX) flush();
    }
    run.push([band(p[0], view.x.min, xs), band(p[1], view.y.min, ys)]);
    prev = p;
  }
  flush();
  // A lone point is not a curve.
  return segments.filter(s => s.length > 1);
}

/** Sample a function or parametric item at the given parameter values. */
export function sampleItem(item: PlotItem, values: readonly number[], spec: Pick<PlotSpec, 'x' | 'y'>): Segments {
  const slots = [...values, 0];
  const last = values.length;
  if (item.type === 'function') {
    return sampleCurve(u => {
      slots[last] = u;
      return [u, item.fn(slots)];
    }, spec.x.min, spec.x.max, item.samples, spec);
  }
  if (item.type === 'parametric') {
    const t0 = item.t[0](values);
    const t1 = item.t[1](values);
    return sampleCurve(u => {
      slots[last] = u;
      return [item.x(slots), item.y(slots)];
    }, t0, t1, item.samples, spec);
  }
  return [];
}

/** Evenly spaced "nice" tick values (1, 2 or 5 × 10^k) covering [min, max]. */
export function niceTicks(min: number, max: number, count = 6): number[] {
  if (!(max > min) || !Number.isFinite(min) || !Number.isFinite(max)) return [];
  const raw = (max - min) / Math.max(1, count);
  const power = Math.pow(10, Math.floor(Math.log10(raw)));
  const unit = raw / power;
  const step = (unit <= 1 ? 1 : unit <= 2 ? 2 : unit <= 5 ? 5 : 10) * power;
  const first = Math.ceil(min / step - 1e-9);
  const last = Math.floor(max / step + 1e-9);
  const ticks: number[] = [];
  for (let k = first; k <= last && ticks.length <= 50; k++) ticks.push(Number((k * step).toPrecision(12)));
  return ticks;
}

/** A compact label for a number (ticks, slider values, readouts). */
export function formatNumber(value: number, digits = 3): string {
  if (!Number.isFinite(value)) return '—';
  if (value === 0) return '0';
  const abs = Math.abs(value);
  if (abs >= 1e5 || abs < 1e-3) return value.toExponential(Math.max(0, digits - 1)).replace('e+', 'e');
  return String(Number(value.toPrecision(digits + 1)));
}
