/**
 * Questions asked before Shodh is ready: they wait outside any view, go out once,
 * only from their own conversation, and leave the queue only when the send was
 * accepted. Run with Node 22.6+:
 *   node --experimental-strip-types --test app/tests/questionQueue.test.ts
 */
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { QuestionQueue } from '../src/features/setup/questionQueue.ts';

test('a waiting question goes out once, when its conversation is shown', () => {
  const queue = new QuestionQueue();
  let sent = 0;
  queue.hold('c1', () => {
    sent += 1;
    return true;
  });
  assert.equal(queue.isWaiting('c1'), true);
  // Another conversation on screen: it keeps waiting (a send goes to the conversation shown).
  assert.equal(queue.flush('c2'), 'none');
  assert.equal(queue.flush(null), 'none');
  assert.equal(sent, 0);
  assert.equal(queue.flush('c1'), 'sent');
  assert.equal(sent, 1);
  assert.equal(queue.isWaiting('c1'), false);
  // Nothing is sent twice.
  assert.equal(queue.flush('c1'), 'none');
  assert.equal(sent, 1);
});

test('a refused send stays queued and is tried again', () => {
  const queue = new QuestionQueue();
  let accept = false;
  let tries = 0;
  queue.hold('c1', () => {
    tries += 1;
    return accept;
  });
  assert.equal(queue.flush('c1'), 'refused');
  assert.equal(queue.isWaiting('c1'), true, 'not dropped when the send was refused');
  assert.equal(queue.refusals('c1'), 1);
  accept = true;
  assert.equal(queue.flush('c1'), 'sent');
  assert.equal(tries, 2);
  assert.equal(queue.isWaiting('c1'), false);

  // A send that throws counts as refused, never as sent.
  queue.hold('c2', () => {
    throw new Error('boom');
  });
  assert.equal(queue.flush('c2'), 'refused');
  assert.equal(queue.isWaiting('c2'), true);
});

test('Don’t send it, and a later question replacing the waiting one', () => {
  const queue = new QuestionQueue();
  const calls: string[] = [];
  queue.hold('c1', () => { calls.push('first'); return true; });
  queue.hold('c1', () => { calls.push('second'); return true; });
  queue.flush('c1');
  assert.deepEqual(calls, ['second']);
  queue.hold('c1', () => { calls.push('third'); return true; });
  queue.cancel('c1');
  assert.equal(queue.isWaiting('c1'), false);
  assert.equal(queue.flush('c1'), 'none');
  assert.deepEqual(calls, ['second']);
});

test('listeners hear every change (views show the waiting state from it)', () => {
  const queue = new QuestionQueue();
  let changes = 0;
  const unsubscribe = queue.subscribe(() => { changes += 1; });
  queue.hold('c1', () => true);
  queue.flush('c1');
  queue.cancel('missing');
  unsubscribe();
  queue.hold('c2', () => true);
  assert.equal(changes, 2);
});
