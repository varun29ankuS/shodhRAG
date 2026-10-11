/**
 * Focus pop-out: zoom math and keyboard map.
 *   node --experimental-strip-types --test app/tests/focusZoom.test.ts
 */
import { test } from 'node:test';
import assert from 'node:assert/strict';
import {
  FIT_PADDING,
  MAX_FIT_SCALE,
  MAX_ZOOM,
  MIN_ZOOM,
  canPan,
  centeredView,
  clampScale,
  clampView,
  fitScale,
  fitView,
  panBy,
  scaledFontSize,
  wheelZoomFactor,
  zoomAt,
  zoomBy,
} from '../src/features/focus/zoomMath.ts';
import { focusCommand, panDelta } from '../src/features/focus/focusKeys.ts';
import type { FocusKeyInput } from '../src/features/focus/focusKeys.ts';

const viewport = { width: 800, height: 600 };
const content = { width: 400, height: 300 };

const close = (a: number, b: number, eps = 1e-6) => assert.ok(Math.abs(a - b) < eps, `${a} ≈ ${b}`);

test('clamp: bounds and non-finite input', () => {
  assert.equal(clampScale(100), MAX_ZOOM);
  assert.equal(clampScale(0.001), MIN_ZOOM);
  assert.equal(clampScale(Number.NaN), 1);
  assert.equal(clampScale(1.5), 1.5);
});

test('fit: limited by the tighter axis, padding respected, small content not blown up', () => {
  close(fitScale(content, viewport), Math.min((800 - 2 * FIT_PADDING) / 400, (600 - 2 * FIT_PADDING) / 300));
  assert.equal(fitScale({ width: 10, height: 10 }, viewport), MAX_FIT_SCALE);
  assert.equal(fitScale({ width: 0, height: 10 }, viewport), 1);
  const big = fitView({ width: 4000, height: 1000 }, viewport);
  close(big.scale, (800 - 2 * FIT_PADDING) / 4000);
  close(big.x, (800 - 4000 * big.scale) / 2);
});

test('zoom around a point keeps that content point under the cursor', () => {
  const start = { scale: 2, x: -100, y: -50 };
  const big = { width: 1000, height: 1000 };
  const point = { x: 300, y: 200 };
  const before = { x: (point.x - start.x) / start.scale, y: (point.y - start.y) / start.scale };
  const next = zoomAt(start, 3, point, big, viewport);
  const after = { x: (point.x - next.x) / next.scale, y: (point.y - next.y) / next.scale };
  close(before.x, after.x);
  close(before.y, after.y);
});

test('zoom is clamped and keeps small content centred', () => {
  const v = zoomBy({ scale: 1, x: 200, y: 150 }, 1000, content, viewport);
  assert.equal(v.scale, MAX_ZOOM);
  const small = zoomBy({ scale: 1, x: 0, y: 0 }, 0.5, content, viewport);
  close(small.x, (800 - 200) / 2);
  close(small.y, (600 - 150) / 2);
});

test('pan is clamped to the content edges', () => {
  const view = centeredView(content, viewport, 4); // 1600×1200
  assert.ok(canPan(view, content, viewport));
  const far = panBy(view, 10_000, 10_000, content, viewport);
  assert.equal(far.x, FIT_PADDING);
  assert.equal(far.y, FIT_PADDING);
  const other = panBy(view, -10_000, -10_000, content, viewport);
  assert.equal(other.x, 800 - 1600 - FIT_PADDING);
  assert.equal(other.y, 600 - 1200 - FIT_PADDING);
  assert.equal(canPan(centeredView(content, viewport, 1), content, viewport), false);
  assert.deepEqual(clampView({ scale: 1, x: 5, y: 5 }, { width: 0, height: 0 }, viewport), { scale: 1, x: 5, y: 5 });
});

test('wheel: direction, normalisation and per-event cap', () => {
  assert.ok(wheelZoomFactor(-100) > 1);
  assert.ok(wheelZoomFactor(100) < 1);
  assert.equal(wheelZoomFactor(0), 1);
  assert.equal(wheelZoomFactor(-100_000), 2);
  assert.equal(wheelZoomFactor(100_000), 0.5);
  close(wheelZoomFactor(-3, 1), wheelZoomFactor(-48, 0));
});

test('font scaling for equations and tables', () => {
  assert.equal(scaledFontSize(16, 2), 32);
  assert.equal(scaledFontSize(16, 100), 16 * MAX_ZOOM);
});

const key = (k: string, opts: Partial<FocusKeyInput> = {}) =>
  focusCommand({ key: k, ctrlKey: false, metaKey: false, altKey: false, shiftKey: false, editable: false, pannable: false, ...opts });

test('keys: zoom, fit, actual size, maximise', () => {
  assert.equal(key('+'), 'zoomIn');
  assert.equal(key('='), 'zoomIn');
  assert.equal(key('-'), 'zoomOut');
  assert.equal(key('_'), 'zoomOut');
  assert.equal(key('0'), 'fit');
  assert.equal(key('1'), 'actualSize');
  assert.equal(key('m'), 'toggleMaximize');
  assert.equal(key('=', { ctrlKey: true }), 'zoomIn');
  assert.equal(key('-', { metaKey: true }), 'zoomOut');
  assert.equal(key('0', { ctrlKey: true }), 'fit');
  assert.equal(key('1', { ctrlKey: true }), null);
  assert.equal(key('+', { altKey: true }), null);
});

test('keys: typing in the side composer is never captured, except Ctrl zoom', () => {
  assert.equal(key('+', { editable: true }), null);
  assert.equal(key('m', { editable: true }), null);
  assert.equal(key('ArrowLeft', { editable: true, pannable: true }), null);
  assert.equal(key('=', { editable: true, ctrlKey: true }), 'zoomIn');
});

test('keys: arrows pan only when there is something to pan', () => {
  assert.equal(key('ArrowLeft'), null);
  assert.equal(key('ArrowLeft', { pannable: true }), 'panLeft');
  assert.equal(key('ArrowDown', { pannable: true }), 'panDown');
  assert.deepEqual(panDelta('panLeft', 10), { dx: 10, dy: 0 });
  assert.deepEqual(panDelta('panDown', 10), { dx: 0, dy: -10 });
  assert.equal(panDelta('fit', 10), null);
});
