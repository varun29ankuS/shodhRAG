/**
 * Sketches, plots and simulations in the focus pop-out: targets, stored
 * threads, context blocks, and one-at-a-time playback.
 *   node --experimental-strip-types --test app/tests/visualFocus.test.ts
 */
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { MAX_CONTEXT_CHARS, MAX_VISUAL_SOURCE_CHARS, buildContextBlock, composeSideQuestion, contextLabel, sliderLines, targetDigest } from '../src/features/focus/contextBlock.ts';
import { plotTarget, simulationTarget, svgTarget } from '../src/features/focus/targets.ts';
import { MAX_PARAM_VALUES, readParamValues, readTarget } from '../src/features/focus/threadStore.ts';
import { SVG_MAX_CHARS } from '../src/features/ask/visual/svgSanitize.ts';
import { PLOT_MAX_CHARS } from '../src/features/ask/visual/plotSpec.ts';
import { claimPlayback, otherIsPlaying, playingId, releasePlayback } from '../src/features/ask/visual/playback.ts';

const SVG = '<svg viewBox="0 0 10 10"><title>Free-body diagram &amp; forces</title><line x1="0" y1="0" x2="5" y2="5"/></svg>';
const PLOT = JSON.stringify({ title: 'Trajectory', x: { min: 0, max: 40 }, y: { min: 0, max: 20 }, params: [{ name: 'v0', min: 1, max: 30, value: 20 }], items: [{ type: 'function', expr: 'x' }] });
const SIM = JSON.stringify({ title: 'Pendulum', state: { th: '0.3', w: '0' }, derivatives: { th: 'w', w: '-g*sin(th)' }, view: { x: [-1, 1], y: [-1, 1] }, draw: [{ type: 'circle', at: ['sin(th)', '-cos(th)'] }] });

test('svg target: labelled from <title>, oversize sources are not kept', () => {
  const t = svgTarget(SVG);
  assert.ok(t && t.kind === 'svg');
  assert.equal(t?.label, 'Free-body diagram & forces');
  assert.equal(svgTarget('<svg viewBox="0 0 1 1"/>')?.label, 'Sketch');
  assert.equal(svgTarget('x'.repeat(SVG_MAX_CHARS + 1)), null);
});

test('plot and simulation targets keep the slider positions', () => {
  const p = plotTarget(PLOT, null, [{ name: 'v0', value: 12.5 }]);
  assert.ok(p && p.kind === 'plot');
  assert.equal(p?.label, 'Trajectory');
  assert.deepEqual(p && p.kind === 'plot' ? p.values : null, [{ name: 'v0', value: 12.5 }]);
  const s = simulationTarget(SIM, null, []);
  assert.equal(s?.label, 'Pendulum');
  assert.equal(plotTarget('x'.repeat(PLOT_MAX_CHARS + 1), 'Big', []), null);
});

test('stored targets: new kinds read back, invalid ones are dropped', () => {
  const p = plotTarget(PLOT, null, [{ name: 'v0', value: 3 }]);
  assert.deepEqual(readTarget(JSON.parse(JSON.stringify(p))), p);
  const s = svgTarget(SVG);
  assert.deepEqual(readTarget(JSON.parse(JSON.stringify(s))), s);
  // Over the render cap: dropped whole rather than cut into markup that cannot be drawn.
  assert.equal(readTarget({ kind: 'svg', label: 'Big', source: 'x'.repeat(SVG_MAX_CHARS + 1) }), null);
  assert.equal(readTarget({ kind: 'simulation', label: 'S', source: '  ' }), null);
  const odd = readTarget({ kind: 'plot', label: 'P', source: PLOT, values: [{ name: 'v0', value: 'NaN' }, { name: '__proto__', value: 1 }, { name: 'a', value: Infinity }, { name: 'b', value: 2 }, { name: 'b', value: 3 }] });
  assert.deepEqual(odd && odd.kind === 'plot' ? odd.values : null, [{ name: 'b', value: 2 }]);
  const many = readParamValues(Array.from({ length: 20 }, (_, i) => ({ name: `p${i}`, value: i })));
  assert.equal(many.length, MAX_PARAM_VALUES);
});

test('context: svg source in an svg fence with long path data shortened', () => {
  const longPath = `<svg viewBox="0 0 1 1"><path d="${'M0 0 L1 1 '.repeat(500)}"/><text>N</text></svg>`;
  const block = buildContextBlock({ kind: 'svg', label: 'FBD', source: longPath });
  assert.ok(block.startsWith('Context — sketch (SVG source; long path data shortened):\n```svg\n'));
  assert.ok(block.includes('…') && block.includes('<text>N</text>'));
  assert.ok(block.length < 1_000);
  assert.equal(contextLabel({ kind: 'svg', label: 'FBD', source: SVG }), 'sketch source');
});

test('context: plot spec plus slider values; the spec is capped so values survive', () => {
  const big = JSON.stringify({ title: 'T', pad: 'x'.repeat(15_000) });
  const target = { kind: 'plot' as const, label: 'T', source: big, values: [{ name: 'v0', value: 20 }, { name: 'theta', value: 0.7853981634 }] };
  const block = buildContextBlock(target);
  assert.ok(block.includes('Context — interactive plot (JSON spec'));
  assert.ok(block.includes('```json\n'));
  assert.ok(block.includes('more characters not included'));
  assert.ok(block.includes('Context — slider values when the reader opened it:\n```text\nv0 = 20\ntheta = 0.785398\n```'));
  assert.ok(MAX_VISUAL_SOURCE_CHARS < MAX_CONTEXT_CHARS);
  assert.equal(contextLabel(target), 'plot spec and slider values');
  assert.equal(contextLabel({ ...target, values: [] }), 'plot spec');
  assert.equal(sliderLines([{ name: 'k', value: 1 / 3 }]), 'k = 0.333333');
});

test('context: simulation spec in a question, digest for nested levels', () => {
  const target = simulationTarget(SIM, null, [])!;
  const q = composeSideQuestion(target, 'Why does the period not depend on mass?');
  assert.ok(q.includes('"Pendulum"'));
  assert.ok(q.includes('Context — simulation (JSON spec: state, derivatives, events, drawing):'));
  assert.ok(!q.includes('slider values'));
  assert.ok(q.endsWith('Question: Why does the period not depend on mass?'));
  assert.equal(contextLabel(target), 'simulation spec');
  assert.ok(targetDigest(target).startsWith('{"title":"Pendulum"'));
});

test('playback: one simulation at a time', () => {
  const paused: string[] = [];
  claimPlayback('a', () => paused.push('a'));
  assert.equal(playingId(), 'a');
  assert.ok(otherIsPlaying('b') && !otherIsPlaying('a'));
  claimPlayback('b', () => paused.push('b'));
  assert.deepEqual(paused, ['a']);
  assert.equal(playingId(), 'b');
  claimPlayback('b', () => paused.push('b'));
  assert.deepEqual(paused, ['a'], 'reclaiming does not pause itself');
  releasePlayback('a');
  assert.equal(playingId(), 'b', 'a stale release is ignored');
  releasePlayback('b');
  assert.equal(playingId(), null);
  assert.ok(!otherIsPlaying('a'));
});
