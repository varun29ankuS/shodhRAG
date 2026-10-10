/**
 * Session count and side-session key tests. Run with Node 22.6+:
 *   node --experimental-strip-types --test app/tests/sessionCounts.test.ts
 */
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { isSessionCounts, sessionCountsLabel } from '../src/features/agent/sessionCounts.ts';
import { sideSessionKey, sideSessionKeys } from '../src/features/focus/threadStore.ts';

test('session counts are labelled for the activity tray', () => {
  assert.equal(sessionCountsLabel({ main: 0, side: 0 }), 'No assistant sessions open');
  assert.equal(sessionCountsLabel({ main: 1, side: 0 }), '1 assistant session open');
  assert.equal(sessionCountsLabel({ main: 1, side: 2 }), '3 assistant sessions open (2 side discussions)');
  assert.equal(sessionCountsLabel({ main: 0, side: 1 }), '1 assistant session open (1 side discussion)');
});

test('session counts payloads are validated', () => {
  assert.equal(isSessionCounts({ main: 1, side: 0 }), true);
  assert.equal(isSessionCounts({ main: '1', side: 0 }), false);
  assert.equal(isSessionCounts(null), false);
});

test('a side thread closes its discussion, summary and refinement sessions', () => {
  const keys = sideSessionKeys('conv-1', 'thread-9');
  assert.deepEqual(keys, [
    sideSessionKey('conv-1', 'thread-9'),
    sideSessionKey('conv-1', 'thread-9-summary'),
    sideSessionKey('conv-1', 'thread-9-refine'),
  ]);
  assert.equal(new Set(keys).size, 3);
});
