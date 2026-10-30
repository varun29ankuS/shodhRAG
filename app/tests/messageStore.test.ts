/**
 * The chat provider's pure parts: a question about a file keeps the file through storage
 * and Retry, and asks about only that file; a new answer is saved from its start, so an
 * app closed mid-answer never leaves a question with no answer. Run with Node 22.6+:
 *   node --experimental-strip-types --test app/tests/messageStore.test.ts
 */
import { test } from 'node:test';
import assert from 'node:assert/strict';
import {
  fromStored,
  isPersistable,
  questionIndexBefore,
  questionMessage,
  retryOptions,
  scopeOf,
  startedAnswer,
  toStored,
} from '../src/features/ask/messageStore.ts';
import type { SendOptions } from '../src/features/ask/types.ts';

const PATH = 'c:/users/someone/documents/research papers/markov memory.pdf';
const NONE: SendOptions = { spaceId: null, spaceName: null };

test('a question about a file keeps the file, and keeps it through storage', () => {
  const asked = questionMessage('u1', 'About markov memory.pdf: summarise it', '2026-10-30T10:00:00.000Z', {
    ...NONE,
    sourceFiles: [PATH],
  });
  assert.deepEqual(asked.aboutFiles, [PATH]);
  assert.ok(!asked.content.includes('someone'));
  const stored = toStored(asked);
  assert.deepEqual(stored.metadata, { aboutFiles: [PATH] });
  const restored = fromStored(JSON.parse(JSON.stringify(stored)));
  assert.deepEqual(restored.aboutFiles, [PATH]);
  // The key is read into the field, not left in the shown metadata.
  assert.equal(restored.metadata, undefined);
});

test('a question about nothing in particular carries no files', () => {
  const asked = questionMessage('u1', 'What is closure augmentation?', 't', { ...NONE, sourceIds: ['a'] });
  assert.equal(asked.aboutFiles, undefined);
  assert.equal(toStored(asked).metadata, undefined);
  assert.equal(fromStored(toStored(asked)).aboutFiles, undefined);
});

test('stored files on an answer are ignored: only questions carry them', () => {
  const answer = fromStored({ id: 'a1', role: 'assistant', content: 'x', timestamp: 't', metadata: { aboutFiles: [PATH] } });
  assert.equal(answer.aboutFiles, undefined);
});

test('Retry asks about the same file, and only that file, whatever sources are included now', () => {
  const asked = questionMessage('u1', 'About it: eq 3?', 't', { ...NONE, sourceFiles: [PATH] });
  const now: SendOptions = { spaceId: 'a', spaceName: 'Folder a', sourceIds: ['a', 'c'] };
  const options = retryOptions(now, asked);
  assert.deepEqual(scopeOf(options, null), { sourceIds: [], sourceFiles: [PATH] });
  // In a workspace the file stays the narrower limit.
  assert.deepEqual(scopeOf(options, 'ws-1'), { sourceIds: [], sourceFiles: [PATH], workspaceId: 'ws-1' });
  // A question about nothing in particular is retried with the current options.
  const plain = questionMessage('u2', 'Anything', 't', NONE);
  assert.equal(retryOptions(now, plain), now);
  assert.deepEqual(scopeOf(retryOptions(now, plain), null), { sourceIds: ['a', 'c'], sourceFiles: [] });
});

test('the question an answer replies to is the nearest one before it', () => {
  const messages = [{ role: 'user' as const }, { role: 'assistant' as const }, { role: 'system' as const }, { role: 'assistant' as const }];
  assert.equal(questionIndexBefore(messages, 1), 0);
  assert.equal(questionIndexBefore(messages, 3), 0);
  assert.equal(questionIndexBefore(messages, 0), -1);
});

test('a new answer is saved from its start and restores as interrupted, with its question answered', () => {
  const { message, checkpoint } = startedAnswer('a1', 'run-1', 1_000, null, ['run-0']);
  // Live: shown running, saved as the checkpoint until it settles.
  assert.equal(isPersistable(message), false);
  assert.equal(message.transcript?.status, 'starting');
  assert.deepEqual(message.transcript?.priorRunIds, ['run-0']);
  assert.equal(checkpoint.id, 'a1');
  assert.equal(checkpoint.role, 'assistant');
  assert.ok(checkpoint.transcript, 'the checkpoint holds the transcript');
  // The app closed before the answer ended: it comes back interrupted, with Retry.
  const restored = fromStored(JSON.parse(JSON.stringify(checkpoint)));
  assert.equal(restored.transcript?.status, 'aborted');
  assert.equal(restored.transcript?.error, 'The app closed before this answer finished.');
  assert.deepEqual(restored.transcript?.priorRunIds, ['run-0']);
  assert.equal(isPersistable(restored), true);
});
