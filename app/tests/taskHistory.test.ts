/**
 * A task's History in words (src/features/tasks/history.ts).
 *   node --experimental-strip-types --test app/tests/taskHistory.test.ts
 */
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { historyRows, whenText } from '../src/features/tasks/history.ts';
import type { TaskChange } from '../src/features/tasks/types.ts';

const NOW = new Date('2026-11-28T12:00:00Z');
const change = (partial: Partial<TaskChange>): TaskChange => ({
  at: '2026-11-28T10:00:00Z',
  actor: 'user',
  field: 'status',
  before: null,
  after: null,
  ...partial,
});

test('each change says who made it and what changed, newest first', () => {
  const rows = historyRows(
    [
      change({ field: 'imported', actor: 'import', at: '2026-11-01T09:00:00Z' }),
      change({ field: 'status', before: 'pending', after: 'canceled' }),
      change({ field: 'priority', actor: 'agent', before: 'none', after: 'urgent', at: '2026-11-28T11:55:00Z' }),
      change({ field: 'project', actor: 'user', before: 'Baselines', after: null, reason: 'project deleted', at: '2026-11-28T11:59:40Z' }),
    ],
    NOW,
  );
  assert.deepEqual(
    rows.map(r => `${r.who} · ${r.what} · ${r.when}`),
    [
      'You · Project: Baselines → none (project deleted) · just now',
      'Shodh · Priority: No priority → Urgent · 5 min ago',
      'You · Status: Todo → Canceled · 2 h ago',
      `Moved here · Kept from the old task list · ${new Intl.DateTimeFormat(undefined, { day: 'numeric', month: 'short' }).format(new Date('2026-11-01T09:00:00Z'))}`,
    ],
  );
});

test('times read in short words', () => {
  assert.equal(whenText('2026-11-28T11:59:50Z', NOW), 'just now');
  assert.equal(whenText('2026-11-28T09:00:00Z', NOW), '3 h ago');
  assert.equal(whenText('nonsense', NOW), '');
});
