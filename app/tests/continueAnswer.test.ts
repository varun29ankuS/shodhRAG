/**
 * Continue: an answer Shodh closed during, and the answer that goes on from it, read as one
 * answer when the conversation is replayed (src/features/ask/chatHistory.ts). Run with
 * Node 22.6+:
 *   node --experimental-strip-types --test app/tests/continueAnswer.test.ts
 */
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { withContinuations } from '../src/features/ask/chatHistory.ts';

const question = { role: 'user', content: 'What is the notice period and the renewal fee?' };
const stopped = {
  role: 'assistant',
  content: "The notice period is 60 days [1]. Renewal follows the landlord's fee schedule [2], which",
  transcript: { runId: 'run-6' },
};
const continued = {
  role: 'assistant',
  content: ' is published each January.',
  transcript: { runId: 'run-7', continuesRunId: 'run-6' },
};

test('a continued answer is replayed as one answer, in its place', () => {
  const replayed = withContinuations([question, stopped, continued]);
  assert.equal(replayed.length, 2, 'the stopped answer and the rest are one');
  assert.equal(replayed[0], question);
  assert.equal(
    replayed[1].content,
    "The notice period is 60 days [1]. Renewal follows the landlord's fee schedule [2], which is published each January.",
  );
  assert.equal(replayed[1].transcript, continued.transcript, 'with the continued answer\'s transcript');
});

test('only the answer a continuation names is joined', () => {
  const other = { ...continued, transcript: { runId: 'run-7', continuesRunId: 'run-5' } };
  assert.equal(withContinuations([question, stopped, other]).length, 3);
  const plain = { role: 'assistant', content: 'Another answer.', transcript: { runId: 'run-8' } };
  assert.equal(withContinuations([question, stopped, plain]).length, 3);
  // A question between them: not a continuation of that answer.
  assert.equal(withContinuations([question, stopped, question, continued]).length, 4);
});

test('a stopped answer that had written nothing gives the continuation alone', () => {
  const empty = { ...stopped, content: '' };
  const replayed = withContinuations([question, empty, { ...continued, content: 'The notice period is 60 days [1].' }]);
  assert.equal(replayed.length, 2);
  assert.equal(replayed[1].content, 'The notice period is 60 days [1].');
});
