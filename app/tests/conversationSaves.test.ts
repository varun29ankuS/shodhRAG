/**
 * Conversation save scheduling tests. Run with Node 22.6+:
 *   node --experimental-strip-types --test app/tests/conversationSaves.test.ts
 */
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { ConversationSaves, FailureLatch, loadFailureNotice, quitRequestId, updateRecord } from '../src/hooks/conversationSaves.ts';

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

test('drain writes every waiting save and resolves only after every write, in order per conversation', async () => {
  const h = harness();
  h.saves.save({ id: 'c1', rev: 'checkpoint' }, undefined, true);
  h.saves.save({ id: 'c1', rev: 'edit' });
  h.saves.save({ id: 'c2', rev: 'other' });
  let drained = false;
  const done = h.saves.drain().then(() => { drained = true; });
  await h.settle();
  assert.deepEqual(h.started, ['checkpoint', 'other'], 'the waiting c1 save queues behind its write in flight');
  assert.equal(drained, false);
  await h.finishNext();
  assert.equal(drained, false, 'c1 still has the flushed save to write');
  await h.finishNext();
  assert.deepEqual(h.started, ['checkpoint', 'other', 'edit']);
  assert.equal(drained, false);
  await h.finishNext();
  await done;
  assert.equal(drained, true);
  assert.deepEqual(h.written, ['checkpoint', 'other', 'edit']);
  h.fireTimers();
  await h.settle();
  assert.equal(h.started.length, 3, 'flushed saves never write again when their timer was due');
});

test('drain with nothing to save resolves at once', async () => {
  const h = harness();
  await h.saves.drain();
  assert.deepEqual(h.started, []);
});

test('quitRequestId reads the backend request id and rejects anything else', () => {
  assert.equal(quitRequestId({ requestId: 7 }), 7);
  assert.equal(quitRequestId({ requestId: 0 }), 0);
  assert.equal(quitRequestId({ requestId: -1 }), null);
  assert.equal(quitRequestId({ requestId: 1.5 }), null);
  assert.equal(quitRequestId({ requestId: '7' }), null);
  assert.equal(quitRequestId({}), null);
  assert.equal(quitRequestId(null), null);
});

test('updateRecord calls the change once and returns the record it put in the list', () => {
  const list: Conv[] = [{ id: 'c1', rev: 'a' }, { id: 'c2', rev: 'b' }];
  let calls = 0;
  const { list: next, updated } = updateRecord(list, 'c2', c => {
    calls += 1;
    return { ...c, rev: 'b2' };
  });
  assert.equal(calls, 1);
  assert.deepEqual(next, [{ id: 'c1', rev: 'a' }, { id: 'c2', rev: 'b2' }]);
  assert.equal(updated, next[1], 'the saved record is the one in the list');
  assert.deepEqual(list[1], { id: 'c2', rev: 'b' }, 'the old list is not changed');
  const missing = updateRecord(list, 'nope', c => c);
  assert.equal(missing.updated, null);
  assert.equal(missing.list, list);
});

test('pause drops waiting saves and refuses later ones, so nothing overwrites an unreadable store', async () => {
  const h = harness();
  h.saves.save({ id: 'c1', rev: 'waiting' });
  assert.equal(h.saves.paused, false);
  h.saves.pause();
  assert.equal(h.saves.paused, true);
  h.fireTimers();
  h.saves.save({ id: 'c1', rev: 'later' }, undefined, true);
  await h.saves.write({ id: 'c2', rev: 'direct' });
  h.saves.flush();
  await h.saves.drain();
  await h.settle();
  assert.deepEqual(h.started, [], 'nothing reaches the store');
  assert.deepEqual(h.errors, []);
});

test('a successful write is reported, so a failure notice can be shown again later', async () => {
  const written: string[] = [];
  const saves = new ConversationSaves<Conv>({
    write: async () => {},
    schedule: () => () => {},
    onError: () => {},
    onWritten: conv => written.push(conv.rev),
    delayMs: 500,
  });
  await saves.write({ id: 'c1', rev: 'a' });
  assert.deepEqual(written, ['a']);
});

test('a store that keeps refusing saves is reported once until a save succeeds', () => {
  const latch = new FailureLatch();
  assert.equal(latch.fail(), true);
  assert.equal(latch.fail(), false);
  assert.equal(latch.fail(), false);
  latch.succeed();
  assert.equal(latch.fail(), true);
});

test('the load failure notice says what happened, that saving is paused, and where the file is', () => {
  const notice = loadFailureNotice('The saved conversations could not be read: C:/Users/me/AppData/Roaming/shodh/conversations.json could not be read (the file is empty) and its backup could not be used either (there is no backup)');
  const text = notice.paragraphs.join(' ');
  assert.match(notice.title, /could not be loaded/);
  assert.match(text, /Saving is paused/);
  assert.match(text, /nothing you do now overwrites/);
  assert.match(text, /conversations\.json/);
  assert.match(loadFailureNotice(new Error('disk gone')).paragraphs.join(' '), /Details: disk gone/);
  assert.match(loadFailureNotice(undefined).paragraphs.join(' '), /No details/);
});
