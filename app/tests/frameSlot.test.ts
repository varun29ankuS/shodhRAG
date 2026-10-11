/**
 * Frame slot tests (the "Ask about this" button's scheduling). Run with Node 22.6+:
 *   node --experimental-strip-types --test app/tests/frameSlot.test.ts
 */
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { createFrameSlot } from '../src/features/focus/frameSlot.ts';

function fakeFrames() {
  let next = 1;
  const queued = new Map<number, () => void>();
  return {
    request: (cb: () => void) => {
      const id = next++;
      queued.set(id, cb);
      return id;
    },
    release: (id: number) => {
      queued.delete(id);
    },
    flush: () => {
      const callbacks = [...queued.values()];
      queued.clear();
      callbacks.forEach(cb => cb());
    },
    size: () => queued.size,
  };
}

test('only one frame is pending at a time', () => {
  const frames = fakeFrames();
  const slot = createFrameSlot(frames.request, frames.release);
  let runs = 0;
  slot.schedule(() => runs++);
  slot.schedule(() => runs++);
  assert.equal(frames.size(), 1);
  frames.flush();
  assert.equal(runs, 1);
  assert.equal(slot.pending, false);
});

test('a frame cancelled before it ran does not block later scheduling', () => {
  const frames = fakeFrames();
  const slot = createFrameSlot(frames.request, frames.release);
  let runs = 0;
  slot.schedule(() => runs++);
  slot.cancel();
  assert.equal(slot.pending, false);
  slot.schedule(() => runs++);
  frames.flush();
  assert.equal(runs, 1);
});

test('the slot is free again after its frame runs', () => {
  const frames = fakeFrames();
  const slot = createFrameSlot(frames.request, frames.release);
  let runs = 0;
  slot.schedule(() => runs++);
  frames.flush();
  slot.schedule(() => runs++);
  frames.flush();
  assert.equal(runs, 2);
});
