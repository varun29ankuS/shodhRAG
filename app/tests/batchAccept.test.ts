/**
 * A suggestion rejected inside its undo window is never saved by "Accept N non-sensitive"
 * (audit D1). Run with Node 22.6+:
 *   node --experimental-strip-types --test app/tests/batchAccept.test.ts
 */
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { UNDO_WINDOW_MS, UndoQueue } from '../src/lib/undoQueue.ts';
import { batchAcceptable, markRejecting, pruneRejecting, withoutRejecting } from '../src/features/memory/batchAccept.ts';

interface Item {
  id: string;
  status: string;
  sensitive: string[];
}

const item = (id: string, sensitive: string[] = []): Item => ({ id, status: 'pending', sensitive });

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
      await new Promise(resolve => setImmediate(resolve));
    },
  };
}

/** The list as Settings → Memory keeps it: what the backend says waits, and the ids being rejected. */
function memoryList(pendingIds: Item[]) {
  const state = { pending: pendingIds, rejecting: new Set<string>() as ReadonlySet<string> };
  const rejectedInBackend: string[] = [];
  return {
    state,
    rejectedInBackend,
    reject(queue: UndoQueue, id: string, fails = false) {
      return queue.schedule({
        hide: () => { state.rejecting = markRejecting(state.rejecting, id, true); },
        restore: () => { state.rejecting = markRejecting(state.rejecting, id, false); },
        commit: async () => {
          if (fails) throw new Error('the database is locked');
          rejectedInBackend.push(id);
        },
      });
    },
    /** The list reloads from the backend. */
    reload() {
      state.pending = state.pending.filter(s => !rejectedInBackend.includes(s.id));
      state.rejecting = pruneRejecting(state.rejecting, state.pending);
    },
    batch: () => batchAcceptable(state.pending, state.rejecting).map(s => s.id),
    shown: () => withoutRejecting(state.pending, state.rejecting).map(s => s.id),
  };
}

test('a suggestion rejected within its undo window is left out of the batch and the count', async () => {
  const clock = manualTimers();
  const queue = new UndoQueue(clock.timers);
  const list = memoryList([item('a'), item('b'), item('c'), item('s', ['health'])]);
  assert.deepEqual(list.batch(), ['a', 'b', 'c']);

  list.reject(queue, 'a');
  // Still pending in the backend for the whole undo window.
  await clock.advance(UNDO_WINDOW_MS - 1);
  assert.deepEqual(list.rejectedInBackend, []);
  assert.deepEqual(list.batch(), ['b', 'c'], 'the declined memory is not accepted with the others');
  assert.deepEqual(list.shown(), ['b', 'c', 's']);

  // The rejection is carried out; until the list reloads it stays excluded.
  await clock.advance(1);
  assert.deepEqual(list.rejectedInBackend, ['a']);
  assert.deepEqual(list.batch(), ['b', 'c']);
  list.reload();
  assert.deepEqual(list.batch(), ['b', 'c']);
  assert.equal(list.state.rejecting.size, 0);
});

test('Undo, or a rejection that fails, puts the suggestion back in the batch', async () => {
  const clock = manualTimers();
  const queue = new UndoQueue(clock.timers);
  const list = memoryList([item('a'), item('b')]);

  const pending = list.reject(queue, 'a');
  assert.deepEqual(list.batch(), ['b']);
  assert.equal(queue.undo(pending), true);
  assert.deepEqual(list.batch(), ['a', 'b']);

  list.reject(queue, 'b', true);
  assert.deepEqual(list.batch(), ['a']);
  await clock.advance(UNDO_WINDOW_MS);
  assert.deepEqual(list.rejectedInBackend, []);
  assert.deepEqual(list.batch(), ['a', 'b'], 'not rejected, so it is pending again');
});

test('a reload while the rejection waits keeps it excluded', () => {
  const clock = manualTimers();
  const queue = new UndoQueue(clock.timers);
  const list = memoryList([item('a'), item('b')]);
  list.reject(queue, 'a');
  list.reload();
  assert.deepEqual(list.batch(), ['b']);
  assert.deepEqual(list.shown(), ['b']);
});
