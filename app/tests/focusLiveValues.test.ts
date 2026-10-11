/**
 * Refine and new versions in the pop-out start from the sliders as the reader
 * has them now.
 *   node --experimental-strip-types --test app/tests/focusLiveValues.test.ts
 */
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { clearLiveValues, currentValues, setLiveValues } from '../src/features/focus/liveValues.ts';
import type { FocusTarget } from '../src/features/focus/focusTypes.ts';

const plot: FocusTarget = { kind: 'plot', label: 'Projectile', source: '{}', values: [{ name: 'v0', value: 20 }] };

test('a level without reports uses the positions it was opened with', () => {
  assert.deepEqual(currentValues(101, plot), [{ name: 'v0', value: 20 }]);
});

test('moved sliders win over the opening positions, per level', () => {
  setLiveValues(102, [{ name: 'v0', value: 27 }]);
  assert.deepEqual(currentValues(102, plot), [{ name: 'v0', value: 27 }]);
  // Another level is unaffected.
  assert.deepEqual(currentValues(103, plot), [{ name: 'v0', value: 20 }]);
  setLiveValues(102, [{ name: 'v0', value: 5 }]);
  assert.deepEqual(currentValues(102, plot), [{ name: 'v0', value: 5 }]);
  clearLiveValues(102);
  assert.deepEqual(currentValues(102, plot), [{ name: 'v0', value: 20 }]);
});

test('non-finite reports are dropped and other kinds have no sliders', () => {
  setLiveValues(104, [{ name: 'v0', value: Number.NaN }]);
  assert.deepEqual(currentValues(104, plot), [{ name: 'v0', value: 20 }]);
  setLiveValues(105, [{ name: 'v0', value: 9 }]);
  assert.deepEqual(currentValues(105, { kind: 'chart', label: 'c', source: '{}' }), []);
});

test('the store stays bounded', () => {
  for (let i = 0; i < 200; i++) setLiveValues(1_000 + i, [{ name: 'v0', value: i }]);
  assert.deepEqual(currentValues(1_000, plot), [{ name: 'v0', value: 20 }]);
  assert.deepEqual(currentValues(1_199, plot), [{ name: 'v0', value: 199 }]);
});
