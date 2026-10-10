/**
 * Undo instead of confirmation: a removal hides the record at once, runs when
 * the undo window ends, and Undo gives the very same record back. Run with
 * Node 22.6+:
 *   node --experimental-strip-types --test app/tests/undoQueue.test.ts
 */
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { UNDO_WINDOW_MS, UndoQueue, restoreAt } from '../src/lib/undoQueue.ts';

/** Manual clock: timers run only when the test advances it. */
function manualTimers() {
  let now = 0;
  let next = 1;
  const timers = new Map<number, { at: number; run: () => void }>();
  return {
    timers: {
      set: (run: () => void, ms: number) => {
        const id = next++;
        timers.set(id, { at: now + ms, run });
        return id;
      },
      clear: (handle: unknown) => { timers.delete(handle as number); },
    },
    async advance(ms: number) {
      now += ms;
      for (const [id, t] of [...timers]) {
        if (t.at <= now) {
          timers.delete(id);
          t.run();
        }
      }
      // Let the commit promises settle.
      await new Promise(resolve => setImmediate(resolve));
    },
  };
}

interface Row { id: string; title: string; addedAt: string; meta: { by: string } }

/** A list with a record removed through the queue, as the converted views do. */
function listWith(rows: Row[]) {
  let shown = [...rows];
  const store = new Map(rows.map(r => [r.id, r]));
  const removeLater = (queue: UndoQueue, id: string) => {
    const index = shown.findIndex(r => r.id === id);
    const record = shown[index];
    return queue.schedule({
      hide: () => { shown = shown.filter(r => r.id !== id); },
      restore: () => {
        const next = [...shown];
        next.splice(index, 0, record);
        shown = next;
      },
      commit: async () => {
        store.delete(id);
      },
    });
  };
  return { shown: () => shown, store, removeLater };
}

const ROWS: Row[] = [
  { id: 'a', title: 'Lease', addedAt: '2026-09-01T10:00:00.000Z', meta: { by: 'user' } },
  { id: 'b', title: 'Parking', addedAt: '2026-09-02T10:00:00.000Z', meta: { by: 'agent' } },
  { id: 'c', title: 'Rules', addedAt: '2026-09-03T10:00:00.000Z', meta: { by: 'user' } },
];

test('the window is six seconds', () => {
  assert.equal(UNDO_WINDOW_MS, 6000);
});

test('a removal hides at once and runs only when the window ends', async () => {
  const clock = manualTimers();
  const queue = new UndoQueue(clock.timers);
  const list = listWith(ROWS);
  list.removeLater(queue, 'b');
  assert.deepEqual(list.shown().map(r => r.id), ['a', 'c']);
  assert.ok(list.store.has('b'), 'nothing is removed before the window ends');
  await clock.advance(UNDO_WINDOW_MS - 1);
  assert.ok(list.store.has('b'));
  await clock.advance(1);
  assert.ok(!list.store.has('b'));
  assert.equal(queue.size, 0);
});

test('undo restores the exact record at its place and nothing is removed', async () => {
  const clock = manualTimers();
  const queue = new UndoQueue(clock.timers);
  const list = listWith(ROWS);
  const id = list.removeLater(queue, 'b');
  assert.equal(queue.undo(id), true);
  assert.deepEqual(list.shown(), ROWS);
  assert.strictEqual(list.shown()[1], ROWS[1], 'the same object, not a copy');
  await clock.advance(UNDO_WINDOW_MS * 2);
  assert.ok(list.store.has('b'));
  assert.equal(queue.undo(id), false, 'undo once');
});

test('U undoes the most recent pending removal first', async () => {
  const clock = manualTimers();
  const queue = new UndoQueue(clock.timers);
  const list = listWith(ROWS);
  list.removeLater(queue, 'a');
  list.removeLater(queue, 'c');
  assert.equal(queue.undoLast(), true);
  assert.deepEqual(list.shown().map(r => r.id), ['b', 'c']);
  await clock.advance(UNDO_WINDOW_MS);
  assert.ok(!list.store.has('a'));
  assert.ok(list.store.has('c'));
  assert.equal(queue.undoLast(), false, 'nothing left to undo');
});

test('closing the notice commits now; a removal that already ran cannot be undone', async () => {
  const clock = manualTimers();
  const queue = new UndoQueue(clock.timers);
  const list = listWith(ROWS);
  const id = list.removeLater(queue, 'a');
  await queue.commit(id);
  assert.ok(!list.store.has('a'));
  assert.equal(queue.undo(id), false);
  assert.deepEqual(list.shown().map(r => r.id), ['b', 'c']);
});

test('a failed removal puts the record back and reports the error', async () => {
  const clock = manualTimers();
  const errors: unknown[] = [];
  const queue = new UndoQueue(clock.timers);
  let shown = ['a', 'b'];
  queue.schedule({
    hide: () => { shown = ['a']; },
    restore: () => { shown = ['a', 'b']; },
    commit: async () => { throw new Error('disk full'); },
    onError: e => errors.push(e),
  });
  await clock.advance(UNDO_WINDOW_MS);
  assert.deepEqual(shown, ['a', 'b']);
  assert.equal((errors[0] as Error).message, 'disk full');
});

test('flush carries out every pending removal', async () => {
  const clock = manualTimers();
  const queue = new UndoQueue(clock.timers);
  const list = listWith(ROWS);
  list.removeLater(queue, 'a');
  list.removeLater(queue, 'b');
  await queue.flush();
  assert.deepEqual([...list.store.keys()], ['c']);
  assert.equal(queue.size, 0);
});

test('restoreAt puts the same record back at its place, once', () => {
  const [a, b, c] = ROWS;
  const restored = restoreAt([a, c], b, 1);
  assert.deepEqual(restored.map(r => r.id), ['a', 'b', 'c']);
  assert.strictEqual(restored[1], b, 'the same object, timestamps and all');
  // The list got shorter meanwhile (another removal): it goes at the end.
  assert.deepEqual(restoreAt([a], c, 2).map(r => r.id), ['a', 'c']);
  // Already back (restored twice, or reloaded): unchanged.
  const list = [a, b, c];
  assert.strictEqual(restoreAt(list, b, 0), list);
});
