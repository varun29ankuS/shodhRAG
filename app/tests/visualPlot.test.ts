/**
 * Interactive plot specs: validation and sampling.
 *   node --experimental-strip-types --test app/tests/visualPlot.test.ts
 */
import { test } from 'node:test';
import assert from 'node:assert/strict';
import {
  MAX_ITEMS,
  MAX_SAMPLES_PER_ITEM,
  MAX_TOTAL_SAMPLES,
  PLOT_MAX_CHARS,
  formatNumber,
  initialValues,
  niceTicks,
  parsePlotSpec,
  sampleCurve,
  sampleItem,
  snapParam,
} from '../src/features/ask/visual/plotSpec.ts';

const base = {
  title: 'Height',
  x: { min: 0, max: 4, label: 't (s)' },
  y: { min: 0, max: 25, label: 'h (m)' },
  params: [{ name: 'v0', min: 5, max: 30, step: 1, value: 20, label: 'v0 (m/s)' }],
  items: [{ type: 'function', expr: 'v0*x - 0.5*g*x^2' }],
};

function parse(spec: unknown) {
  return parsePlotSpec(JSON.stringify(spec));
}

test('plot: a valid spec compiles with params', () => {
  const r = parse(base);
  assert.ok(r.ok);
  if (!r.ok) return;
  assert.equal(r.spec.params[0].name, 'v0');
  const item = r.spec.items[0];
  assert.equal(item.type, 'function');
  if (item.type !== 'function') return;
  assert.equal(item.fn([20, 1]), 20 - 0.5 * 9.81);
});

test('plot: every item type is accepted', () => {
  const r = parse({
    ...base,
    params: [{ name: 'a', min: 0, max: 5, value: 2 }, { name: 'b', min: 0, max: 5, value: 1 }],
    items: [
      { type: 'function', expr: 'a*x' },
      { type: 'parametric', x: 'cos(t)', y: 'sin(t)', t: [0, '2*pi'] },
      { type: 'point', x: 'a', y: 'b', draggable: true, label: 'P' },
      { type: 'point', x: 'a+1', y: 2, draggable: true },
      { type: 'vector', from: [0, 0], to: ['a', 'b'] },
      { type: 'segment', from: [0, 0], to: [1, 1], color: 'warning' },
      { type: 'label', at: [1, 1], text: 'apex' },
    ],
  });
  assert.ok(r.ok, r.ok ? '' : r.error);
  if (!r.ok) return;
  const point = r.spec.items[2];
  assert.ok(point.type === 'point' && point.drag && point.drag.x === 'a' && point.drag.y === 'b');
  const fixed = r.spec.items[3];
  assert.ok(fixed.type === 'point' && fixed.drag === null, 'expressions are not draggable');
  const seg = r.spec.items[5];
  assert.equal(seg.type === 'segment' && seg.color, 'warning');
});

test('plot: validation errors name the problem', () => {
  const cases: [unknown, RegExp][] = [
    [{ ...base, x: { min: 4, max: 0 } }, /x.min/],
    [{ ...base, y: undefined }, /"y"/],
    [{ ...base, items: [] }, /items/],
    [{ ...base, items: [{ type: 'function', expr: 'v1*x' }] }, /Item 1 \(function\).*Unknown name "v1"/],
    [{ ...base, items: [{ type: 'bezier' }] }, /unknown type/],
    [{ ...base, params: [{ name: 'x', min: 0, max: 1 }] }, /Parameter 1/],
    [{ ...base, params: [{ name: 'sin', min: 0, max: 1 }] }, /Parameter 1/],
    [{ ...base, params: [{ name: 'a', min: 0, max: 1 }, { name: 'a', min: 0, max: 1 }] }, /twice/],
    [{ ...base, params: [{ name: 'a', min: 1, max: 1 }] }, /min" < "max/],
    [{ ...base, items: Array.from({ length: MAX_ITEMS + 1 }, () => ({ type: 'function', expr: 'x' })) }, /At most/],
    [{ ...base, items: [{ type: 'label', at: [0, 0] }] }, /needs "text"/],
  ];
  for (const [spec, pattern] of cases) {
    const r = parse(spec);
    assert.equal(r.ok, false);
    if (!r.ok) assert.match(r.error, pattern);
  }
  assert.equal(parsePlotSpec('{ not json').ok, false);
  assert.equal(parsePlotSpec('[1,2]').ok, false);
  const huge = JSON.stringify({ ...base, title: 'x'.repeat(PLOT_MAX_CHARS) });
  const r = parsePlotSpec(huge);
  assert.ok(!r.ok && /larger than/.test(r.error));
});

test('plot: parameter values clamp and snap', () => {
  const r = parse({ ...base, params: [{ name: 'k', min: 0, max: 1, step: 0.1, value: 7 }], items: [{ type: 'function', expr: 'k*x' }] });
  assert.ok(r.ok);
  if (!r.ok) return;
  const p = r.spec.params[0];
  assert.equal(p.value, 1);
  assert.equal(snapParam(p, 0.3333), 0.3);
  assert.equal(snapParam(p, -5), 0);
  assert.deepEqual(initialValues(r.spec.params, [{ name: 'k', value: 0.4 }]), [0.4]);
  assert.deepEqual(initialValues(r.spec.params, [{ name: 'k', value: 99 }]), [1]);
  assert.deepEqual(initialValues(r.spec.params, [{ name: 'other', value: 0.2 }]), [1]);
});

test('plot: sampling breaks at undefined points', () => {
  const view = { x: { min: -2, max: 2, label: '' }, y: { min: -2, max: 2, label: '' } };
  const segments = sampleCurve(x => [x, Math.sqrt(x)], -2, 2, 401, view);
  assert.equal(segments.length, 1);
  assert.ok(segments[0].every(([x]) => x >= 0));
});

test('plot: sampling breaks at asymptotes instead of drawing a vertical line', () => {
  const view = { x: { min: -Math.PI, max: Math.PI, label: '' }, y: { min: -5, max: 5, label: '' } };
  const segments = sampleCurve(x => [x, Math.tan(x)], -Math.PI, Math.PI, 400, view);
  assert.ok(segments.length >= 3, `expected a run per branch, got ${segments.length}`);
  for (const run of segments) {
    for (let i = 1; i < run.length; i++) {
      // Within a run, consecutive samples never jump from far above to far below the view.
      assert.ok(!(run[i - 1][1] > 5 && run[i][1] < -5) && !(run[i - 1][1] < -5 && run[i][1] > 5));
    }
  }
  const inv = sampleCurve(x => [x, 1 / x], -1, 1, 400, { x: { min: -1, max: 1, label: '' }, y: { min: -10, max: 10, label: '' } });
  assert.equal(inv.length, 2);
});

test('plot: coordinates are clamped to a band around the view', () => {
  const view = { x: { min: 0, max: 1, label: '' }, y: { min: 0, max: 1, label: '' } };
  const segments = sampleCurve(x => [x, Math.exp(1000 * x)], 0, 1, 100, view);
  const ys = segments.flat().map(p => p[1]);
  assert.ok(ys.every(y => y <= 11 && Number.isFinite(y)));
});

test('plot: sample counts are capped per item and in total', () => {
  const r = parse({ ...base, items: Array.from({ length: 30 }, () => ({ type: 'function', expr: 'x', samples: 1e9 })) });
  assert.ok(r.ok);
  if (!r.ok) return;
  let total = 0;
  for (const item of r.spec.items) {
    if (item.type === 'function') {
      assert.ok(item.samples <= MAX_SAMPLES_PER_ITEM);
      total += item.samples;
    }
  }
  assert.ok(total <= MAX_TOTAL_SAMPLES + 16 * 30, `total ${total}`);
  const segments = sampleCurve(x => [x, x], 0, 1, 1e9, { x: { min: 0, max: 1, label: '' }, y: { min: 0, max: 1, label: '' } });
  assert.ok(segments[0].length <= MAX_SAMPLES_PER_ITEM);
});

test('plot: parametric items sample over their t range at the given params', () => {
  const r = parse({ ...base, params: [{ name: 'R', min: 1, max: 3, value: 2 }], x: { min: -3, max: 3 }, y: { min: -3, max: 3 }, items: [{ type: 'parametric', x: 'R*cos(t)', y: 'R*sin(t)', t: [0, '2*pi'], samples: 100 }] });
  assert.ok(r.ok);
  if (!r.ok) return;
  const segs = sampleItem(r.spec.items[0], [2], r.spec);
  assert.equal(segs.length, 1);
  for (const [x, y] of segs[0]) assert.ok(Math.abs(Math.hypot(x, y) - 2) < 1e-9);
});

test('plot: an expression that throws inside a curve only ends the run', () => {
  const view = { x: { min: 0, max: 1, label: '' }, y: { min: 0, max: 1, label: '' } };
  const segs = sampleCurve(x => {
    if (x > 0.5) throw new Error('boom');
    return [x, x];
  }, 0, 1, 11, view);
  assert.equal(segs.length, 1);
});

test('plot: nice ticks and number labels', () => {
  assert.deepEqual(niceTicks(0, 10, 5), [0, 2, 4, 6, 8, 10]);
  assert.deepEqual(niceTicks(-1, 1, 4), [-1, -0.5, 0, 0.5, 1]);
  assert.deepEqual(niceTicks(1, 1), []);
  assert.equal(formatNumber(0.1 + 0.2), '0.3');
  assert.equal(formatNumber(123456), '1.23e5');
  assert.equal(formatNumber(NaN), '—');
});
