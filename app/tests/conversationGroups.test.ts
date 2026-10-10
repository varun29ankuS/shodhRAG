/**
 * Conversation history grouping tests. Run with Node 22.6+:
 *   node --experimental-strip-types --test app/tests/conversationGroups.test.ts
 */
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { groupConversations, historyBucket, isBlankConversation } from '../src/lib/conversationGroups.ts';

// Local time: Saturday 3 October 2026, 09:30.
const NOW = new Date(2026, 9, 3, 9, 30);
const at = (y: number, m: number, d: number, h = 12, min = 0) => new Date(y, m, d, h, min).toISOString();

const conv = (id: string, updatedAt: string, pinned = false) => ({ id, title: id, updatedAt, pinned });

test('buckets by local calendar day, not by 24-hour windows', () => {
  assert.equal(historyBucket(at(2026, 9, 3, 0, 0), NOW), 'today');
  assert.equal(historyBucket(at(2026, 9, 2, 23, 59), NOW), 'yesterday');
  // 45 minutes ago, but on the previous calendar day.
  assert.equal(historyBucket(at(2026, 9, 2, 23, 30), new Date(2026, 9, 3, 0, 15)), 'yesterday');
  assert.equal(historyBucket(at(2026, 9, 2, 0, 0), NOW), 'yesterday');
  assert.equal(historyBucket(at(2026, 9, 1, 23, 59), NOW), 'previous7');
  assert.equal(historyBucket(at(2026, 8, 26, 0, 0), NOW), 'previous7');
  assert.equal(historyBucket(at(2026, 8, 25, 23, 59), NOW), 'older');
});

test('future timestamps are today and unparseable ones are older', () => {
  assert.equal(historyBucket(at(2026, 9, 5), NOW), 'today');
  assert.equal(historyBucket('not a date', NOW), 'older');
  assert.equal(historyBucket('', NOW), 'older');
});

test('groups in order, pinned first, newest first, empty groups omitted', () => {
  const groups = groupConversations(
    [
      conv('old', at(2026, 0, 1)),
      conv('today-early', at(2026, 9, 3, 8)),
      conv('pinned-old', at(2025, 5, 1), true),
      conv('today-late', at(2026, 9, 3, 9)),
      conv('week', at(2026, 8, 29)),
    ],
    NOW,
  );
  assert.deepEqual(groups.map(g => g.id), ['pinned', 'today', 'previous7', 'older']);
  assert.deepEqual(groups.map(g => g.label), ['Pinned', 'Today', 'Previous 7 days', 'Older']);
  assert.deepEqual(groups[1].items.map(c => c.id), ['today-late', 'today-early']);
  assert.deepEqual(groups[0].items.map(c => c.id), ['pinned-old']);
});

test('no conversations, no groups', () => {
  assert.deepEqual(groupConversations([], NOW), []);
});

test('does not mutate its input', () => {
  const input = [conv('a', at(2026, 9, 3, 8)), conv('b', at(2026, 9, 3, 9))];
  const copy = input.map(c => ({ ...c }));
  groupConversations(input, NOW);
  assert.deepEqual(input, copy);
});

test('blank conversations are untitled and empty', () => {
  assert.equal(isBlankConversation({ title: 'New Chat', messages: [] }), true);
  assert.equal(isBlankConversation({ title: 'New Chat' }), true);
  assert.equal(isBlankConversation({ title: 'New Chat', messages: [{}] }), false);
  assert.equal(isBlankConversation({ title: 'Quarterly numbers', messages: [] }), false);
});
