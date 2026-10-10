/**
 * Tasks calendar grid tests. Run with Node 22.6+:
 *   node --experimental-strip-types --test app/tests/calendarGrid.test.ts
 */
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { addDays, dayKey, groupByDay, isoDayKey, monthGrid } from '../src/features/tasks/calendarGrid.ts';

test('day keys use the local calendar day', () => {
  assert.equal(dayKey(new Date(2026, 0, 5, 23, 59)), '2026-01-05');
  assert.equal(isoDayKey(new Date(2026, 9, 3, 0, 1).toISOString()), '2026-10-03');
  assert.equal(isoDayKey(null), null);
  assert.equal(isoDayKey('garbage'), null);
});

test('month grid is six whole weeks starting on the week start', () => {
  // October 2026 starts on a Thursday.
  const weeks = monthGrid(2026, 9);
  assert.equal(weeks.length, 6);
  for (const week of weeks) assert.equal(week.length, 7);
  assert.equal(weeks[0][0].key, '2026-09-27');
  assert.equal(weeks[0][0].date.getDay(), 0);
  assert.equal(weeks[0][4].key, '2026-10-01');
  assert.equal(weeks[0][3].inMonth, false);
  assert.equal(weeks[0][4].inMonth, true);
  assert.equal(weeks.flat().filter(d => d.inMonth).length, 31);
});

test('month grid honours a Monday week start', () => {
  const weeks = monthGrid(2026, 9, 1);
  assert.equal(weeks[0][0].key, '2026-09-28');
  assert.equal(weeks[0][0].date.getDay(), 1);
});

test('a month starting on the week start has no leading days', () => {
  // February 2026 starts on a Sunday.
  assert.equal(monthGrid(2026, 1)[0][0].key, '2026-02-01');
});

test('every cell is a distinct consecutive day', () => {
  const days = monthGrid(2026, 2).flat();
  assert.equal(new Set(days.map(d => d.key)).size, 42);
  for (let i = 1; i < days.length; i++) {
    assert.equal(dayKey(addDays(days[i - 1].date, 1)), days[i].key);
  }
});

test('groups items by local day and skips undated ones', () => {
  const items = [
    { id: 'a', at: new Date(2026, 9, 3, 9).toISOString() },
    { id: 'b', at: null },
    { id: 'c', at: new Date(2026, 9, 3, 18).toISOString() },
    { id: 'd', at: new Date(2026, 9, 4, 1).toISOString() },
  ];
  const map = groupByDay(items, i => i.at);
  assert.deepEqual(map.get('2026-10-03')?.map(i => i.id), ['a', 'c']);
  assert.deepEqual(map.get('2026-10-04')?.map(i => i.id), ['d']);
  assert.equal(map.size, 2);
});

test('addDays crosses month and year boundaries', () => {
  assert.equal(dayKey(addDays(new Date(2026, 11, 31), 1)), '2027-01-01');
  assert.equal(dayKey(addDays(new Date(2026, 2, 1), -1)), '2026-02-28');
});
