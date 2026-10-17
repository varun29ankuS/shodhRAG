/**
 * A suggestion rejected inside its undo window is never saved by "Accept N non-sensitive"
 * (audit D1). Run with Node 22.6+:
 *   node --experimental-strip-types --test app/tests/batchAccept.test.ts
 */
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { UNDO_WINDOW_MS, UndoQueue } from '../src/lib/undoQueue.ts';
import { batchAcceptable, withoutRejecting } from '../src/features/memory/batchAccept.ts';
import { RejectingStore } from '../src/features/memory/rejecting.ts';

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

/**
 * Settings → Memory's list and the Inbox over one store of ids being rejected: what the
 * backend says waits, and what each surface rejects through its undo queue.
 */
function memoryApp(pending: Item[]) {
  const store = new RejectingStore();
  const state = { pending };
  const rejectedInBackend: string[] = [];
  const reject = (queue: UndoQueue, id: string, fails = false) =>
    queue.schedule({
      ...store.around(id, async () => {
        if (fails) throw new Error('the database is locked');
        rejectedInBackend.push(id);
      }),
    });
  return {
    store,
    rejectedInBackend,
    /** Reject in Settings → Memory. */
    rejectInSettings: reject,
    /** Dismiss in the Inbox (another surface, the same store). */
    dismissInInbox: reject,
    /** The list reloads from the backend. */
    reload() {
      state.pending = state.pending.filter(s => !rejectedInBackend.includes(s.id));
    },
    batch: () => batchAcceptable(state.pending, store.get()).map(s => s.id),
    shown: () => withoutRejecting(state.pending, store.get()).map(s => s.id),
  };
}

test('a suggestion rejected within its undo window is left out of the batch and the count', async () => {
  const clock = manualTimers();
  const queue = new UndoQueue(clock.timers);
  const app = memoryApp([item('a'), item('b'), item('c'), item('s', ['health'])]);
  assert.deepEqual(app.batch(), ['a', 'b', 'c']);

  app.rejectInSettings(queue, 'a');
  // Still pending in the backend for the whole undo window.
  await clock.advance(UNDO_WINDOW_MS - 1);
  assert.deepEqual(app.rejectedInBackend, []);
  assert.deepEqual(app.batch(), ['b', 'c'], 'the declined memory is not accepted with the others');
  assert.deepEqual(app.shown(), ['b', 'c', 's']);

  // The rejection is carried out; the list reloads without it.
  await clock.advance(1);
  assert.deepEqual(app.rejectedInBackend, ['a']);
  app.reload();
  assert.deepEqual(app.batch(), ['b', 'c']);
  assert.equal(app.store.get().size, 0);
});

test('a suggestion dismissed in the Inbox is left out of the batch in Settings', async () => {
  const clock = manualTimers();
  const inboxQueue = new UndoQueue(clock.timers);
  const app = memoryApp([item('a'), item('b'), item('c')]);
  let changes = 0;
  const unsubscribe = app.store.subscribe(() => { changes++; });
  app.dismissInInbox(inboxQueue, 'b');
  assert.equal(changes, 1, 'Settings is told at once');
  assert.deepEqual(app.batch(), ['a', 'c']);
  assert.deepEqual(app.shown(), ['a', 'c']);
  await clock.advance(UNDO_WINDOW_MS);
  app.reload();
  assert.deepEqual(app.batch(), ['a', 'c']);
  unsubscribe();
});

test('Undo, or a rejection that fails, puts the suggestion back in the batch', async () => {
  const clock = manualTimers();
  const queue = new UndoQueue(clock.timers);
  const app = memoryApp([item('a'), item('b')]);

  const pending = app.rejectInSettings(queue, 'a');
  assert.deepEqual(app.batch(), ['b']);
  assert.equal(queue.undo(pending), true);
  assert.deepEqual(app.batch(), ['a', 'b']);

  app.dismissInInbox(queue, 'b', true);
  assert.deepEqual(app.batch(), ['a']);
  await clock.advance(UNDO_WINDOW_MS);
  assert.deepEqual(app.rejectedInBackend, []);
  assert.deepEqual(app.batch(), ['a', 'b'], 'not rejected, so it is pending again');
});

test('a reload while the rejection waits keeps it excluded', () => {
  const clock = manualTimers();
  const queue = new UndoQueue(clock.timers);
  const app = memoryApp([item('a'), item('b')]);
  app.rejectInSettings(queue, 'a');
  app.reload();
  assert.deepEqual(app.batch(), ['b']);
  assert.deepEqual(app.shown(), ['b']);
});
