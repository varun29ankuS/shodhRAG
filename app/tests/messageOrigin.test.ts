/**
 * Who wrote a question, kept with it (`features/ask/messageOrigin.ts`): a fix or check
 * the code review pane sent stays the review's when it is retried, never the user's
 * typed words, and its answer's edits keep asking. Run with Node 22.6+:
 *   node --experimental-strip-types --test app/tests/messageOrigin.test.ts
 */
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { metadataWithFromReview, originOf, readFromReview, wireOrigin } from '../src/features/ask/messageOrigin.ts';
import { fromStored, questionMessage, toStored } from '../src/features/ask/messageStore.ts';

test('a review message is sent as composed text that asks for every edit', () => {
  assert.deepEqual(wireOrigin(originOf({ fromReview: true })), { textOrigin: 'composed', reviewAction: true });
  assert.deepEqual(wireOrigin(originOf({})), { textOrigin: 'typed', reviewAction: false });
  assert.deepEqual(wireOrigin(originOf({ sideSummary: { id: 'x' } })), { textOrigin: 'composed', reviewAction: false });
});

test('a retry of a review message keeps its origin, through a save and a reload', () => {
  const asked = questionMessage('m1', 'Run this check…', '2026-11-06T00:00:00Z', { spaceId: null, spaceName: null }, { fromReview: true });
  assert.equal(asked.fromReview, true);
  const reloaded = fromStored(toStored(asked));
  assert.equal(reloaded.fromReview, true);
  assert.equal(originOf(reloaded), 'review', 'a retry is never typed text');
  const typed = fromStored(toStored(questionMessage('m2', 'hello', '2026-11-06T00:00:00Z', { spaceId: null, spaceName: null })));
  assert.equal(originOf(typed), 'typed');
  assert.equal(readFromReview(metadataWithFromReview({ other: 1 }, true)), true);
  assert.deepEqual(metadataWithFromReview({ other: 1, composedBy: 'review' }, false), { other: 1 });
  assert.equal(metadataWithFromReview(undefined, false), undefined);
});
