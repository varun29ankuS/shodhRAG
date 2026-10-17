/**
 * Conversation save scheduling tests. Run with Node 22.6+:
 *   node --experimental-strip-types --test app/tests/conversationSaves.test.ts
 */
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { ConversationSaves } from '../src/hooks/conversationSaves.ts';

interface Conv {
  id: string;
  rev: string;
}

/** A store whose writes finish only when the test says, and a manual clock. */
function harness() {
  const written: string[] = [];
  const started: string[] = [];
  const finishers: Array<() => void> = [];
  const timers: Array<{ run: () => void; cancelled: boolean }> = [];
  const errors: string[] = [];
  const saves = new ConversationSaves<Conv>({
    write: conv => {
      started.push(conv.rev);
      return new Promise<void>(resolve => {
        finishers.push(() => {
          written.push(conv.rev);
          resolve();
        });
      });
    },
    schedule: run => {
      const timer = { run, cancelled: false };
      timers.push(timer);
      return () => { timer.cancelled = true; };
    },
    onError: (conv, error) => errors.push(`${conv.rev}: ${String(error)}`),
    delayMs: 500,
  });
  const fireTimers = () => {
    for (const t of timers.splice(0)) if (!t.cancelled) t.run();
  };
  /** Let queued promise callbacks run. */
  const settle = () => new Promise<void>(resolve => setImmediate(resolve));
  const finishNext = async () => {
    await settle();
    const finish = finishers.shift();
    assert.ok(finish, 'a write is in flight');
    finish();
    await settle();
  };
  return { saves, written, started, errors, fireTimers, settle, finishNext };
}

test('an immediate save writes without waiting for the delay and replaces the waiting save', async () => {
  const h = harness();
  h.saves.save({ id: 'c1', rev: 'user message' });
  await h.settle();
  assert.deepEqual(h.started, [], 'a debounced save waits');
  h.saves.save({ id: 'c1', rev: 'finished answer' }, undefined, true);
  await h.settle();
  assert.deepEqual(h.started, ['finished answer']);
  h.fireTimers();
  await h.finishNext();
  assert.deepEqual(h.written, ['finished answer'], 'the replaced debounced save is never written');
});

test('writes of one conversation reach the store in the order they were made', async () => {
  const h = harness();
  h.saves.save({ id: 'c1', rev: 'checkpoint' }, undefined, true);
  h.saves.save({ id: 'c1', rev: 'final' }, undefined, true);
  await h.settle();
  assert.deepEqual(h.started, ['checkpoint'], 'the second write waits for the first');
  await h.finishNext();
  assert.deepEqual(h.started, ['checkpoint', 'final']);
  await h.finishNext();
  assert.deepEqual(h.written, ['checkpoint', 'final']);
});

test('a failed write does not stop the next one', async () => {
  const written: string[] = [];
  const errors: string[] = [];
  let calls = 0;
  const saves = new ConversationSaves<Conv>({
    write: async conv => {
      calls += 1;
      if (calls === 1) throw new Error('disk full');
      written.push(conv.rev);
    },
    schedule: () => () => {},
    onError: (conv, error) => errors.push(`${conv.rev}: ${(error as Error).message}`),
    delayMs: 500,
  });
  await saves.write({ id: 'c1', rev: 'a' });
  await saves.write({ id: 'c1', rev: 'b' });
  assert.deepEqual(errors, ['a: disk full']);
  assert.deepEqual(written, ['b']);
});

test('different conversations do not wait for each other', async () => {
  const h = harness();
  h.saves.save({ id: 'c1', rev: 'one' }, undefined, true);
  h.saves.save({ id: 'c2', rev: 'two' }, undefined, true);
  await h.settle();
  assert.deepEqual(h.started, ['one', 'two']);
});

test('debounced saves: the latest per conversation is written after the delay, and onSaved runs once written', async () => {
  const h = harness();
  const saved: string[] = [];
  h.saves.save({ id: 'c1', rev: 'a' }, () => saved.push('a'));
  h.saves.save({ id: 'c1', rev: 'b' }, () => saved.push('b'));
  h.saves.save({ id: 'c2', rev: 'x' });
  h.fireTimers();
  await h.finishNext();
  await h.finishNext();
  assert.deepEqual(h.written.sort(), ['b', 'x']);
  assert.deepEqual(saved, ['b']);
});

test('flush writes every waiting save now, and only once', async () => {
  const h = harness();
  h.saves.save({ id: 'c1', rev: 'a' });
  h.saves.save({ id: 'c2', rev: 'b' });
  h.saves.flush();
  h.saves.flush();
  h.fireTimers();
  await h.settle();
  assert.deepEqual(h.started.sort(), ['a', 'b']);
});
