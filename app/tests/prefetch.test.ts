/**
 * Prefetch scheduling and cancellation tests. Run with Node 22.6+:
 *   node --experimental-strip-types --test app/tests/prefetch.test.ts
 */
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { Prefetcher } from '../src/features/library/prefetch.ts';
import type { PrefetchSignal, PrefetchTimers } from '../src/features/library/prefetch.ts';

/** Manual clock: timers fire only when `advance` passes their due time. */
function fakeTimers() {
  let now = 0;
  let nextId = 1;
  const pending = new Map<number, { due: number; fn: () => void }>();
  const timers: PrefetchTimers = {
    setTimeout(fn, ms) {
      const id = nextId++;
      pending.set(id, { due: now + ms, fn });
      return id;
    },
    clearTimeout(handle) {
      pending.delete(handle as number);
    },
  };
  const advance = (ms: number) => {
    now += ms;
    for (const [id, t] of [...pending].sort((a, b) => a[1].due - b[1].due)) {
      if (t.due <= now) {
        pending.delete(id);
        t.fn();
      }
    }
  };
  return { timers, advance, pendingCount: () => pending.size };
}

/** A task whose completion the test controls. */
function deferredTask() {
  const calls: PrefetchSignal[] = [];
  let finish: () => void = () => {};
  const run = (signal: PrefetchSignal) => {
    calls.push(signal);
    return new Promise<void>(resolve => {
      finish = resolve;
    });
  };
  return { run, calls, finish: () => finish() };
}

const flush = () => new Promise<void>(resolve => setImmediate(resolve));

test('runs after the delay, not before', () => {
  const { timers, advance } = fakeTimers();
  const p = new Prefetcher(timers);
  const t = deferredTask();
  p.schedule('a', 200, t.run);
  advance(199);
  assert.equal(t.calls.length, 0);
  assert.equal(p.stateOf('a'), 'waiting');
  advance(1);
  assert.equal(t.calls.length, 1);
  assert.equal(p.stateOf('a'), 'running');
});

test('cancelling a waiting task clears its timer and it never runs', () => {
  const { timers, advance, pendingCount } = fakeTimers();
  const p = new Prefetcher(timers);
  const t = deferredTask();
  p.schedule('a', 200, t.run);
  p.cancel('a');
  assert.equal(pendingCount(), 0);
  advance(500);
  assert.equal(t.calls.length, 0);
  assert.deepEqual(p.activeKeys(), []);
});

test('cancelling a running task flags its signal so the result is discarded', async () => {
  const { timers } = fakeTimers();
  const p = new Prefetcher(timers);
  const t = deferredTask();
  p.schedule('a', 0, t.run);
  assert.equal(t.calls[0].cancelled, false);
  p.cancel('a');
  assert.equal(t.calls[0].cancelled, true);
  t.finish();
  await flush();
  assert.deepEqual(p.activeKeys(), []);
});

test('scheduling a key that is already waiting or running is a no-op', () => {
  const { timers, advance } = fakeTimers();
  const p = new Prefetcher(timers);
  const t = deferredTask();
  p.schedule('a', 100, t.run);
  p.schedule('a', 0, t.run);
  advance(100);
  p.schedule('a', 0, t.run);
  assert.equal(t.calls.length, 1);
});

test('concurrency 1 queues tasks and starts the next when one settles', async () => {
  const { timers } = fakeTimers();
  const p = new Prefetcher(timers, 1);
  const a = deferredTask();
  const b = deferredTask();
  p.schedule('a', 0, a.run);
  p.schedule('b', 0, b.run);
  assert.equal(p.stateOf('b'), 'queued');
  assert.equal(b.calls.length, 0);
  a.finish();
  await flush();
  assert.equal(b.calls.length, 1);
});

test('a queued task cancelled before its turn never runs', async () => {
  const { timers } = fakeTimers();
  const p = new Prefetcher(timers, 1);
  const a = deferredTask();
  const b = deferredTask();
  const c = deferredTask();
  p.schedule('a', 0, a.run);
  p.schedule('b', 0, b.run);
  p.schedule('c', 0, c.run);
  p.cancel('b');
  a.finish();
  await flush();
  assert.equal(b.calls.length, 0);
  assert.equal(c.calls.length, 1);
});

test('retain cancels everything outside the kept set (selection moved on)', () => {
  const { timers, advance } = fakeTimers();
  const p = new Prefetcher(timers, 2);
  const prev = deferredTask();
  const next = deferredTask();
  const hover = deferredTask();
  p.schedule('prev', 0, prev.run);
  p.schedule('next', 0, next.run);
  p.schedule('hover', 200, hover.run);
  p.retain(['next']);
  advance(300);
  assert.equal(prev.calls[0].cancelled, true);
  assert.equal(next.calls[0].cancelled, false);
  assert.equal(hover.calls.length, 0);
  assert.deepEqual(p.activeKeys(), ['next']);
});

test('a failing task frees its slot and is not retried', async () => {
  const { timers } = fakeTimers();
  const p = new Prefetcher(timers, 1);
  const b = deferredTask();
  p.schedule('a', 0, () => Promise.reject(new Error('read failed')));
  p.schedule('b', 0, b.run);
  await flush();
  assert.equal(b.calls.length, 1);
  assert.equal(p.stateOf('a'), null);
});

test('a task that throws synchronously is contained', async () => {
  const { timers } = fakeTimers();
  const p = new Prefetcher(timers, 1);
  p.schedule('a', 0, () => {
    throw new Error('boom');
  });
  await flush();
  assert.deepEqual(p.activeKeys(), []);
});

test('a key can be scheduled again after it was cancelled', () => {
  const { timers, advance } = fakeTimers();
  const p = new Prefetcher(timers);
  const t = deferredTask();
  p.schedule('a', 100, t.run);
  p.cancel('a');
  p.schedule('a', 100, t.run);
  advance(100);
  assert.equal(t.calls.length, 1);
});
