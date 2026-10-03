/**
 * Live simulations written by the agent as ```simulation fences: a
 * declarative model (state, derivatives, events) integrated by the app.
 * Formulas use the safe expression language in `expr.ts`; nothing the model
 * writes runs as code, and every run is bounded in steps and time.
 *
 *   { "title": "Projectile with bounce",
 *     "params": [{"name": "v0", "min": 1, "max": 30, "value": 15, "label": "v0 (m/s)"}],
 *     "state": {"x": "0", "y": "0", "vx": "v0*cos(pi/4)", "vy": "v0*sin(pi/4)"},
 *     "derivatives": {"x": "vx", "y": "vy", "vx": "0", "vy": "-g"},
 *     "events": [{"when": "y < 0", "set": {"y": "0", "vy": "-0.8*vy"}}],
 *     "view": {"x": [0, 40], "y": [0, 15]},
 *     "draw": [{"type": "trail", "at": ["x", "y"]}, {"type": "circle", "at": ["x", "y"], "r": 0.4}],
 *     "readouts": [{"label": "Height", "expr": "y", "unit": "m"}] }
 *
 * Integrators: "rk4" (default, classic 4th-order Runge–Kutta) or "euler"
 * (semi-implicit: state variables update in the listed order, each using the
 * values already updated this step, so list velocities before positions for
 * a symplectic step). Events fire when their condition becomes true
 * (edge-triggered); their assignments use the values from before the event.
 *
 * Pure module, unit-tested with Node (`app/tests/visualSimulation.test.ts`).
 */

import { type Compiled, isValidName, tryCompile } from './expr.ts';
import { pickColor, type VisualColor } from './palette.ts';
import { type PlotParam, readParams } from './plotSpec.ts';

/** Largest simulation spec rendered, in characters. Also the cap kept with a side thread. */
export const SIMULATION_MAX_CHARS = 20_000;
export const MAX_STATE = 16;
export const MAX_EVENTS = 8;
export const MAX_DRAW = 40;
export const MAX_READOUTS = 8;
export const DEFAULT_DT = 1 / 240;
export const MIN_DT = 1e-4;
export const MAX_DT = 0.05;
export const DEFAULT_DURATION = 30;
export const MAX_DURATION = 600;
/** Integration steps allowed in one animation frame (keeps a frame short whatever dt and speed are). */
export const MAX_STEPS_PER_FRAME = 4_000;
/** Integration steps allowed in one run. */
export const MAX_TOTAL_STEPS = 2_000_000;
export const MAX_TRAIL_POINTS = 3_000;
const MAX_BOUND = 1e9;
const TIME = 't';

export type Pair = [Compiled, Compiled];

export type DrawItem =
  | { type: 'circle'; at: Pair; r: Compiled; fill: boolean; color: VisualColor; label: string }
  | { type: 'rect'; at: Pair; size: Pair; angle: Compiled | null; fill: boolean; color: VisualColor; label: string }
  | { type: 'line'; from: Pair; to: Pair; width: number; color: VisualColor; label: string }
  | { type: 'spring'; from: Pair; to: Pair; coils: number; color: VisualColor; label: string }
  | { type: 'vector'; from: Pair; to: Pair; color: VisualColor; label: string }
  | { type: 'trail'; at: Pair; max: number; color: VisualColor; label: string }
  | { type: 'label'; at: Pair; text: string; color: VisualColor };

export interface Readout {
  label: string;
  expr: Compiled;
  unit: string;
  digits: number;
}

export interface SimEvent {
  when: Compiled;
  /** Assignments: state index and new value. */
  set: { index: number; value: Compiled }[];
}

export interface SimView {
  x: [number, number];
  y: [number, number];
  xLabel: string;
  yLabel: string;
  grid: boolean;
  equal: boolean;
}

export interface SimModel {
  title: string;
  params: PlotParam[];
  stateNames: string[];
  /** Initial value of each state variable (reads parameters only). */
  initial: Compiled[];
  /** Time derivative of each state variable (reads parameters, state and t). */
  derivatives: Compiled[];
  events: SimEvent[];
  stop: Compiled | null;
  dt: number;
  duration: number;
  speed: number;
  integrator: 'rk4' | 'euler';
  view: SimView;
  draw: DrawItem[];
  readouts: Readout[];
}

export type SimParseResult = { ok: true; model: SimModel } | { ok: false; error: string };

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === 'object' && value !== null && !Array.isArray(value);
}

function finite(value: unknown): number | null {
  return typeof value === 'number' && Number.isFinite(value) ? value : null;
}

function text(value: unknown, max = 80): string {
  return typeof value === 'string' ? value.replace(/\s+/g, ' ').trim().slice(0, max) : '';
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

function readRange(value: unknown, where: string): [number, number] | string {
  if (!Array.isArray(value) || value.length !== 2) return `${where} must be [min, max].`;
  const a = finite(value[0]);
  const b = finite(value[1]);
  if (a === null || b === null || !(a < b)) return `${where} must be two numbers, min < max.`;
  if (Math.abs(a) > MAX_BOUND || Math.abs(b) > MAX_BOUND) return `${where} is too large.`;
  return [a, b];
}

function readDraw(value: unknown, index: number, names: readonly string[]): DrawItem | string {
  if (!isRecord(value)) return `Draw item ${index + 1} must be an object.`;
  const type = typeof value.type === 'string' ? value.type.trim().toLowerCase() : '';
  const where = `Draw item ${index + 1} (${type || 'no type'})`;
  const color = pickColor(value.color, index);
  const label = text(value.label, 40);
  const fill = value.fill !== false;
  switch (type) {
    case 'circle': {
      const at = readPair(value.at, names, `${where} "at"`);
      if (typeof at === 'string') return at;
      const r = compile(value.r ?? 0.5, names, `${where} "r"`);
      if (typeof r === 'string') return r;
      return { type, at, r, fill, color, label };
    }
    case 'rect': {
      const at = readPair(value.at, names, `${where} "at"`);
      if (typeof at === 'string') return at;
      const size = readPair(value.size ?? [1, 1], names, `${where} "size"`);
      if (typeof size === 'string') return size;
      let angle: Compiled | null = null;
      if (value.angle !== undefined) {
        const a = compile(value.angle, names, `${where} "angle"`);
        if (typeof a === 'string') return a;
        angle = a;
      }
      return { type, at, size, angle, fill, color, label };
    }
    case 'line':
    case 'rod':
    case 'spring':
    case 'vector': {
      const from = readPair(value.from, names, `${where} "from"`);
      if (typeof from === 'string') return from;
      const to = readPair(value.to, names, `${where} "to"`);
      if (typeof to === 'string') return to;
      if (type === 'spring') {
        const coils = finite(value.coils);
        return { type, from, to, coils: coils === null ? 8 : Math.max(2, Math.min(40, Math.round(coils))), color, label };
      }
      if (type === 'vector') return { type, from, to, color, label };
      const width = finite(value.width);
      return { type: 'line', from, to, width: width === null ? (type === 'rod' ? 3 : 1.5) : Math.max(0.5, Math.min(8, width)), color, label };
    }
    case 'trail': {
      const at = readPair(value.at, names, `${where} "at"`);
      if (typeof at === 'string') return at;
      const max = finite(value.max);
      return { type, at, max: max === null ? 600 : Math.max(10, Math.min(MAX_TRAIL_POINTS, Math.round(max))), color, label };
    }
    case 'label': {
      const at = readPair(value.at, names, `${where} "at"`);
      if (typeof at === 'string') return at;
      const body = text(value.text, 60);
      if (!body) return `${where} needs "text".`;
      return { type, at, text: body, color: pickColor(value.color ?? 'text', index) };
    }
    default:
      return `${where}: unknown type; use circle, rect, line, rod, spring, vector, trail or label.`;
  }
}

export function parseSimulationSpec(source: string): SimParseResult {
  if (source.length > SIMULATION_MAX_CHARS) return { ok: false, error: `The simulation spec is larger than ${SIMULATION_MAX_CHARS / 1000} KB.` };
  let parsed: unknown;
  try {
    parsed = JSON.parse(source);
  } catch {
    return { ok: false, error: 'The simulation spec is not valid JSON.' };
  }
  if (!isRecord(parsed)) return { ok: false, error: 'The simulation spec must be a JSON object.' };

  if (!isRecord(parsed.state)) return { ok: false, error: '"state" must be an object of initial values, e.g. {"x": "0"}.' };
  // Object.keys on parsed JSON never yields inherited names; names are checked below anyway.
  const stateNames = Object.keys(parsed.state);
  if (stateNames.length === 0) return { ok: false, error: '"state" needs at least one variable.' };
  if (stateNames.length > MAX_STATE) return { ok: false, error: `At most ${MAX_STATE} state variables are supported.` };
  for (const name of stateNames) {
    if (!isValidName(name) || name === TIME) return { ok: false, error: `"${name}" cannot be a state variable name.` };
  }
  const params = readParams(parsed.params, new Set([...stateNames, TIME]));
  if (typeof params === 'string') return { ok: false, error: params };
  const paramNames = params.map(p => p.name);
  const all = [...paramNames, ...stateNames, TIME];

  const state = parsed.state as Record<string, unknown>;
  const initial: Compiled[] = [];
  for (const name of stateNames) {
    const fn = compile(state[name], paramNames, `Initial value of "${name}"`);
    if (typeof fn === 'string') return { ok: false, error: fn };
    initial.push(fn);
  }

  const derivSource = parsed.derivatives;
  if (!isRecord(derivSource)) return { ok: false, error: '"derivatives" must be an object, e.g. {"x": "vx", "vx": "-k*x"}.' };
  for (const key of Object.keys(derivSource)) {
    if (!stateNames.includes(key)) return { ok: false, error: `"derivatives" names "${key}", which is not in "state".` };
  }
  const derivatives: Compiled[] = [];
  for (const name of stateNames) {
    const fn = compile(Object.prototype.hasOwnProperty.call(derivSource, name) ? derivSource[name] : '0', all, `Derivative of "${name}"`);
    if (typeof fn === 'string') return { ok: false, error: fn };
    derivatives.push(fn);
  }

  const events: SimEvent[] = [];
  if (parsed.events !== undefined) {
    if (!Array.isArray(parsed.events)) return { ok: false, error: '"events" must be a list.' };
    if (parsed.events.length > MAX_EVENTS) return { ok: false, error: `At most ${MAX_EVENTS} events are supported.` };
    for (let i = 0; i < parsed.events.length; i++) {
      const ev = parsed.events[i];
      const where = `Event ${i + 1}`;
      if (!isRecord(ev) || !isRecord(ev.set)) return { ok: false, error: `${where} needs "when" and "set".` };
      const when = compile(ev.when, all, `${where} "when"`);
      if (typeof when === 'string') return { ok: false, error: when };
      const set: SimEvent['set'] = [];
      for (const key of Object.keys(ev.set)) {
        const index = stateNames.indexOf(key);
        if (index < 0) return { ok: false, error: `${where} sets "${key}", which is not in "state".` };
        const value = compile(ev.set[key], all, `${where} "set.${key}"`);
        if (typeof value === 'string') return { ok: false, error: value };
        set.push({ index, value });
      }
      events.push({ when, set });
    }
  }

  let stop: Compiled | null = null;
  if (parsed.stop !== undefined) {
    const fn = compile(parsed.stop, all, '"stop"');
    if (typeof fn === 'string') return { ok: false, error: fn };
    stop = fn;
  }

  const viewSource = isRecord(parsed.view) ? parsed.view : null;
  if (!viewSource) return { ok: false, error: '"view" must give the visible region, e.g. {"x": [0, 10], "y": [0, 5]}.' };
  const vx = readRange(viewSource.x, '"view.x"');
  if (typeof vx === 'string') return { ok: false, error: vx };
  const vy = readRange(viewSource.y, '"view.y"');
  if (typeof vy === 'string') return { ok: false, error: vy };
  const view: SimView = {
    x: vx,
    y: vy,
    xLabel: text(viewSource.xLabel, 40),
    yLabel: text(viewSource.yLabel, 40),
    grid: viewSource.grid !== false,
    equal: viewSource.equal !== false,
  };

  if (!Array.isArray(parsed.draw) || parsed.draw.length === 0) return { ok: false, error: '"draw" needs at least one item, e.g. a circle at ["x", "y"].' };
  if (parsed.draw.length > MAX_DRAW) return { ok: false, error: `At most ${MAX_DRAW} draw items are supported.` };
  const draw: DrawItem[] = [];
  for (let i = 0; i < parsed.draw.length; i++) {
    const item = readDraw(parsed.draw[i], i, all);
    if (typeof item === 'string') return { ok: false, error: item };
    draw.push(item);
  }

  const readouts: Readout[] = [];
  if (parsed.readouts !== undefined) {
    if (!Array.isArray(parsed.readouts)) return { ok: false, error: '"readouts" must be a list.' };
    for (const [i, r] of parsed.readouts.slice(0, MAX_READOUTS).entries()) {
      if (!isRecord(r)) return { ok: false, error: `Readout ${i + 1} must be an object.` };
      const expr = compile(r.expr, all, `Readout ${i + 1}`);
      if (typeof expr === 'string') return { ok: false, error: expr };
      const digits = finite(r.digits);
      readouts.push({ label: text(r.label, 30) || `Value ${i + 1}`, expr, unit: text(r.unit, 12), digits: digits === null ? 3 : Math.max(1, Math.min(8, Math.round(digits))) });
    }
  }

  const dtRaw = finite(parsed.dt);
  const durationRaw = finite(parsed.duration);
  const speedRaw = finite(parsed.speed);
  return {
    ok: true,
    model: {
      title: text(parsed.title, 120),
      params,
      stateNames,
      initial,
      derivatives,
      events,
      stop,
      dt: dtRaw === null ? DEFAULT_DT : Math.max(MIN_DT, Math.min(MAX_DT, dtRaw)),
      duration: durationRaw === null || durationRaw <= 0 ? DEFAULT_DURATION : Math.min(MAX_DURATION, durationRaw),
      speed: speedRaw === null || speedRaw <= 0 ? 1 : Math.max(0.05, Math.min(20, speedRaw)),
      integrator: parsed.integrator === 'euler' ? 'euler' : 'rk4',
      view,
      draw,
      readouts,
    },
  };
}

export type FinishReason = 'duration' | 'stop' | 'diverged' | 'steps';

/** The running state. `slots` holds parameters, then state variables, then t. */
export interface SimState {
  slots: Float64Array;
  steps: number;
  /** Simulated seconds not yet integrated (fixed-step accumulator). */
  pending: number;
  finished: FinishReason | null;
  /** Whether each event condition held after the previous step (edge detection). */
  active: boolean[];
}

function offsets(model: SimModel) {
  const p = model.params.length;
  const n = model.stateNames.length;
  return { p, n, t: p + n };
}

/** A fresh run at the given parameter values (spec order). */
export function createSimState(model: SimModel, paramValues: readonly number[]): SimState {
  const { p, n, t } = offsets(model);
  const slots = new Float64Array(p + n + 1);
  for (let i = 0; i < p; i++) slots[i] = paramValues[i] ?? model.params[i].value;
  const params = slots.subarray(0, p);
  for (let i = 0; i < n; i++) slots[p + i] = model.initial[i](params);
  slots[t] = 0;
  const state: SimState = { slots, steps: 0, pending: 0, finished: null, active: model.events.map(e => e.when(slots) !== 0) };
  if (!allFinite(slots)) state.finished = 'diverged';
  return state;
}

function allFinite(slots: Float64Array): boolean {
  for (let i = 0; i < slots.length; i++) if (!Number.isFinite(slots[i])) return false;
  return true;
}

/** Scratch buffers per model shape, reused across steps. */
interface Scratch {
  k1: Float64Array;
  k2: Float64Array;
  k3: Float64Array;
  k4: Float64Array;
  tmp: Float64Array;
}

const scratchCache = new WeakMap<SimModel, Scratch>();

function scratchFor(model: SimModel, size: number): Scratch {
  let s = scratchCache.get(model);
  if (!s || s.tmp.length !== size) {
    const n = model.stateNames.length;
    s = { k1: new Float64Array(n), k2: new Float64Array(n), k3: new Float64Array(n), k4: new Float64Array(n), tmp: new Float64Array(size) };
    scratchCache.set(model, s);
  }
  return s;
}

function derive(model: SimModel, slots: Float64Array, out: Float64Array) {
  for (let i = 0; i < out.length; i++) out[i] = model.derivatives[i](slots);
}

/** Advance one fixed step of `model.dt`, then apply events and the stop rule. */
export function stepOnce(model: SimModel, state: SimState): void {
  if (state.finished) return;
  const { p, n, t } = offsets(model);
  const dt = model.dt;
  const y = state.slots;
  if (model.integrator === 'euler') {
    for (let i = 0; i < n; i++) y[p + i] += dt * model.derivatives[i](y);
    y[t] += dt;
  } else {
    const s = scratchFor(model, y.length);
    const tmp = s.tmp;
    derive(model, y, s.k1);
    tmp.set(y);
    for (let i = 0; i < n; i++) tmp[p + i] = y[p + i] + (dt / 2) * s.k1[i];
    tmp[t] = y[t] + dt / 2;
    derive(model, tmp, s.k2);
    for (let i = 0; i < n; i++) tmp[p + i] = y[p + i] + (dt / 2) * s.k2[i];
    derive(model, tmp, s.k3);
    for (let i = 0; i < n; i++) tmp[p + i] = y[p + i] + dt * s.k3[i];
    tmp[t] = y[t] + dt;
    derive(model, tmp, s.k4);
    for (let i = 0; i < n; i++) y[p + i] += (dt / 6) * (s.k1[i] + 2 * s.k2[i] + 2 * s.k3[i] + s.k4[i]);
    y[t] += dt;
  }
  state.steps++;

  for (let e = 0; e < model.events.length; e++) {
    const ev = model.events[e];
    const now = ev.when(y) !== 0;
    if (now && !state.active[e]) {
      // All assignments read the values from before the event.
      const values = ev.set.map(a => a.value(y));
      ev.set.forEach((a, k) => {
        y[p + a.index] = values[k];
      });
      state.active[e] = ev.when(y) !== 0;
    } else {
      state.active[e] = now;
    }
  }

  if (!allFinite(y)) state.finished = 'diverged';
  else if (model.stop && model.stop(y) !== 0) state.finished = 'stop';
  else if (y[t] >= model.duration - 1e-12) state.finished = 'duration';
  else if (state.steps >= MAX_TOTAL_STEPS) state.finished = 'steps';
}

/**
 * Advance by `seconds` of wall time (scaled by the model's speed) using fixed
 * steps. At most `MAX_STEPS_PER_FRAME` steps run per call; time beyond that
 * is dropped so a slow frame never snowballs. Returns the steps taken.
 */
export function advance(model: SimModel, state: SimState, seconds: number): number {
  if (state.finished || !(seconds > 0)) return 0;
  state.pending = Math.min(state.pending + seconds * model.speed, model.dt * MAX_STEPS_PER_FRAME);
  let taken = 0;
  while (state.pending >= model.dt && !state.finished && taken < MAX_STEPS_PER_FRAME) {
    stepOnce(model, state);
    state.pending -= model.dt;
    taken++;
  }
  return taken;
}

/** Simulated time of a run. */
export function simTime(model: SimModel, state: SimState): number {
  return state.slots[offsets(model).t];
}

/** Current value of a state variable by name (tests and readouts). */
export function stateValue(model: SimModel, state: SimState, name: string): number {
  const i = model.stateNames.indexOf(name);
  return i < 0 ? NaN : state.slots[offsets(model).p + i];
}

/** Human description of why a run ended. */
export function finishMessage(reason: FinishReason): string {
  switch (reason) {
    case 'duration':
      return 'Finished: reached the end of the simulated time.';
    case 'stop':
      return 'Finished: the stop condition was met.';
    case 'diverged':
      return 'Stopped: a value became infinite or undefined. Check the formulas or try other parameter values.';
    case 'steps':
      return 'Stopped: the step limit for one run was reached.';
  }
}
