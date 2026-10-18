/**
 * PDF pages are drawn again at the new device pixel ratio when the window moves to
 * a monitor with another scale. Run with Node 22.6+:
 *   node --experimental-strip-types --test app/tests/pixelRatio.test.ts
 */
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { currentPixelRatio, resolutionQuery, watchPixelRatio } from '../src/features/ask/viewer/pixelRatio.ts';

test('a missing or broken ratio draws at 1', () => {
  assert.equal(currentPixelRatio(1.5), 1.5);
  assert.equal(currentPixelRatio(undefined), 1);
  assert.equal(currentPixelRatio(0), 1);
  assert.equal(currentPixelRatio(Number.NaN), 1);
  assert.equal(resolutionQuery(1.25), '(resolution: 1.25dppx)');
});

/** A window whose ratio the test changes; one listener per query, as matchMedia gives. */
function fakeWindow(start: number) {
  const queries: { query: string; listeners: Set<() => void> }[] = [];
  const win = {
    devicePixelRatio: start,
    matchMedia(query: string) {
      const entry = { query, listeners: new Set<() => void>() };
      queries.push(entry);
      return {
        addEventListener: (_: 'change', fn: () => void) => entry.listeners.add(fn),
        removeEventListener: (_: 'change', fn: () => void) => entry.listeners.delete(fn),
      };
    },
    move(ratio: number) {
      const armed = queries.filter(q => q.listeners.size > 0 && q.query !== resolutionQuery(ratio));
      win.devicePixelRatio = ratio;
      for (const q of armed) for (const fn of [...q.listeners]) fn();
    },
    armed: () => queries.filter(q => q.listeners.size > 0).map(q => q.query),
  };
  return win;
}

test('every move to another scale is reported, with the query armed for the new ratio', () => {
  const win = fakeWindow(1);
  const seen: number[] = [];
  const stop = watchPixelRatio(win, r => seen.push(r));
  assert.deepEqual(win.armed(), ['(resolution: 1dppx)']);
  win.move(1.5);
  win.move(2);
  assert.deepEqual(seen, [1.5, 2]);
  assert.deepEqual(win.armed(), ['(resolution: 2dppx)']);
  stop();
  assert.deepEqual(win.armed(), []);
  win.move(1);
  assert.deepEqual(seen, [1.5, 2]);
});
