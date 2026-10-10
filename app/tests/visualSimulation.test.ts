/**
 * Declarative simulations: validation and the integrator.
 *   node --experimental-strip-types --test app/tests/visualSimulation.test.ts
 */
import { test } from 'node:test';
import assert from 'node:assert/strict';
import {
  MAX_STEPS_PER_FRAME,
  SIMULATION_MAX_CHARS,
  advance,
  createSimState,
  parseSimulationSpec,
  simTime,
  stateValue,
  stepOnce,
} from '../src/features/ask/visual/simulationSpec.ts';
import { GRAVITY } from '../src/features/ask/visual/expr.ts';

function model(spec: Record<string, unknown>) {
  const r = parseSimulationSpec(JSON.stringify({ view: { x: [0, 10], y: [0, 10] }, draw: [{ type: 'circle', at: [0, 0] }], ...spec }));
  assert.ok(r.ok, r.ok ? '' : r.error);
  return r.ok ? r.model : (null as never);
}

function run(m: ReturnType<typeof model>, values: number[] = m.params.map(p => p.value), maxSteps = 1e6) {
  const s = createSimState(m, values);
  for (let i = 0; i < maxSteps && !s.finished; i++) stepOnce(m, s);
  return s;
}

test('simulation: projectile range matches v0^2 sin(2a) / g', () => {
  const m = model({
    params: [{ name: 'v0', min: 1, max: 40, value: 20 }, { name: 'a', min: 0, max: 1.5, value: Math.PI / 4 }],
    state: { x: '0', y: '0', vx: 'v0*cos(a)', vy: 'v0*sin(a)' },
    derivatives: { x: 'vx', y: 'vy', vx: '0', vy: '-g' },
    stop: 'y < 0',
    dt: 0.001,
  });
  const s = run(m);
  assert.equal(s.finished, 'stop');
  const expected = (20 * 20 * Math.sin(Math.PI / 2)) / GRAVITY;
  assert.ok(Math.abs(stateValue(m, s, 'x') - expected) < 0.05, `${stateValue(m, s, 'x')} vs ${expected}`);
});

test('simulation: small-angle pendulum period is 2*pi*sqrt(L/g)', () => {
  const m = model({
    params: [{ name: 'L', min: 0.1, max: 5, value: 1 }],
    state: { th: '0.05', w: '0' },
    derivatives: { th: 'w', w: '-g/L*sin(th)' },
    dt: 0.001,
    duration: 10,
  });
  const s = createSimState(m, [1]);
  const crossings: number[] = [];
  let prev = stateValue(m, s, 'th');
  while (!s.finished) {
    stepOnce(m, s);
    const th = stateValue(m, s, 'th');
    if (prev > 0 && th <= 0) crossings.push(simTime(m, s));
    prev = th;
  }
  assert.ok(crossings.length >= 3);
  const period = (crossings[crossings.length - 1] - crossings[0]) / (crossings.length - 1);
  const expected = 2 * Math.PI * Math.sqrt(1 / GRAVITY);
  assert.ok(Math.abs(period - expected) / expected < 1e-3, `${period} vs ${expected}`);
});

test('simulation: RK4 keeps a harmonic oscillator energy within a tight bound', () => {
  const m = model({ state: { x: '1', v: '0' }, derivatives: { x: 'v', v: '-x' }, dt: 0.01, duration: 100 });
  const s = run(m);
  const energy = 0.5 * stateValue(m, s, 'v') ** 2 + 0.5 * stateValue(m, s, 'x') ** 2;
  assert.ok(Math.abs(energy - 0.5) < 1e-6, `energy ${energy}`);
  // Semi-implicit Euler with velocity first stays bounded too (symplectic).
  const e = model({ state: { v: '0', x: '1' }, derivatives: { x: 'v', v: '-x' }, dt: 0.01, duration: 100, integrator: 'euler' });
  const se = run(e);
  const energyE = 0.5 * stateValue(e, se, 'v') ** 2 + 0.5 * stateValue(e, se, 'x') ** 2;
  assert.ok(Math.abs(energyE - 0.5) < 0.01, `euler energy ${energyE}`);
});

test('simulation: a bounce event flips the velocity once per contact', () => {
  const m = model({
    state: { y: '1', vy: '0' },
    derivatives: { y: 'vy', vy: '-g' },
    events: [{ when: 'y < 0', set: { y: '0', vy: '-0.5*vy' } }],
    dt: 0.001,
    duration: 0.6,
  });
  const s = createSimState(m, []);
  let bounces = 0;
  let prevVy = 0;
  while (!s.finished) {
    stepOnce(m, s);
    const vy = stateValue(m, s, 'vy');
    if (prevVy < 0 && vy > 0) bounces++;
    prevVy = vy;
    assert.ok(stateValue(m, s, 'y') >= 0);
  }
  assert.equal(bounces, 1);
  const impact = Math.sqrt(2 * GRAVITY);
  // After the bounce it rises with about half the impact speed, minus gravity since.
  assert.ok(stateValue(m, s, 'vy') < 0.5 * impact);
});

test('simulation: event assignments read values from before the event', () => {
  const m = model({ state: { a: '1', b: '2' }, derivatives: {}, events: [{ when: 't > 0', set: { a: 'b', b: 'a' } }], dt: 0.01, duration: 0.05 });
  const s = createSimState(m, []);
  stepOnce(m, s);
  assert.equal(stateValue(m, s, 'a'), 2);
  assert.equal(stateValue(m, s, 'b'), 1);
});

test('simulation: validation rejects unknown references and bad names', () => {
  const cases: [Record<string, unknown>, RegExp][] = [
    [{ state: { x: '0' }, derivatives: { x: 'vx' } }, /Unknown name "vx"/],
    [{ state: { x: '0' }, derivatives: { y: '1' } }, /not in "state"/],
    [{ state: { x: 'k' }, derivatives: {} }, /Initial value of "x".*Unknown name "k"/],
    [{ state: { t: '0' }, derivatives: {} }, /cannot be a state/],
    [{ state: {}, derivatives: {} }, /at least one/],
    [{ state: { x: '0' }, derivatives: {}, events: [{ when: 'x > 1', set: { z: '0' } }] }, /not in "state"/],
    [{ state: { x: '0' }, derivatives: {}, draw: [] }, /"draw"/],
    [{ state: { x: '0' }, derivatives: {}, draw: [{ type: 'teapot' }] }, /unknown type/],
    [{ state: { x: '0' }, derivatives: {}, view: { x: [1, 0], y: [0, 1] } }, /view.x/],
    [{ state: { x: '0' }, derivatives: {}, params: [{ name: 'x', min: 0, max: 1 }] }, /Parameter|clashes/],
  ];
  for (const [spec, pattern] of cases) {
    const raw = JSON.stringify({ view: { x: [0, 10], y: [0, 10] }, draw: [{ type: 'circle', at: ['x', '0'] }], ...spec });
    const r = parseSimulationSpec(raw);
    assert.equal(r.ok, false, JSON.stringify(spec));
    if (!r.ok) assert.match(r.error, pattern);
  }
  const proto = parseSimulationSpec('{"state": {"__proto__": "0"}, "derivatives": {}, "view": {"x": [0, 1], "y": [0, 1]}, "draw": [{"type": "circle", "at": [0, 0]}]}');
  assert.ok(!proto.ok && /cannot be a state/.test(proto.error));
  const big = parseSimulationSpec(`{"title":"${'x'.repeat(SIMULATION_MAX_CHARS)}"}`);
  assert.ok(!big.ok && /larger than/.test(big.error));
});

test('simulation: a diverging model stops instead of running on', () => {
  const m = model({ state: { x: '1' }, derivatives: { x: 'x^3' }, dt: 0.05, duration: 600 });
  const s = run(m, [], 1e5);
  assert.equal(s.finished, 'diverged');
});

test('simulation: work per frame is bounded and slow frames do not snowball', () => {
  const m = model({ state: { x: '0' }, derivatives: { x: '1' }, dt: 0.0001, duration: 600, speed: 20 });
  const s = createSimState(m, []);
  const taken = advance(m, s, 10);
  assert.ok(taken <= MAX_STEPS_PER_FRAME);
  assert.ok(s.pending <= m.dt * MAX_STEPS_PER_FRAME);
  assert.equal(advance(m, s, -1), 0);
  assert.equal(advance(m, s, NaN), 0);
});

test('simulation: duration, dt and speed are clamped', () => {
  const m = model({ state: { x: '0' }, derivatives: {}, dt: 10, duration: 1e9, speed: 1e6 });
  assert.equal(m.dt, 0.05);
  assert.equal(m.duration, 600);
  assert.equal(m.speed, 20);
  const s = run(m);
  assert.equal(s.finished, 'duration');
});

test('simulation: draw items and readouts compile against params, state and t', () => {
  const m = model({
    params: [{ name: 'L', min: 0.5, max: 2, value: 1 }],
    state: { th: '0.5', w: '0' },
    derivatives: { th: 'w', w: '-g/L*sin(th)' },
    draw: [
      { type: 'rod', from: [0, 0], to: ['L*sin(th)', '-L*cos(th)'] },
      { type: 'circle', at: ['L*sin(th)', '-L*cos(th)'], r: 0.1 },
      { type: 'spring', from: [0, 0], to: [1, 't'], coils: 100 },
      { type: 'trail', at: ['L*sin(th)', '-L*cos(th)'], max: 1e9 },
      { type: 'vector', from: [0, 0], to: ['w', 0] },
      { type: 'rect', at: [0, 0], size: [1, 0.2], angle: 'th' },
      { type: 'label', at: [0, 0], text: 'pivot' },
    ],
    readouts: [{ label: 'Angle', expr: 'th', unit: 'rad', digits: 2 }],
  });
  assert.equal(m.draw.length, 7);
  const spring = m.draw[2];
  assert.ok(spring.type === 'spring' && spring.coils === 40);
  const trail = m.draw[3];
  assert.ok(trail.type === 'trail' && trail.max === 3000);
  assert.equal(m.draw[0].type, 'line');
  const s = createSimState(m, [1]);
  assert.equal(m.readouts[0].expr(s.slots), 0.5);
});
