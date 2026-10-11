/**
 * Due-date parsing/formatting and reschedule math. Run with Node 22.6+:
 *   node --experimental-strip-types --test app/tests/dueDate.test.ts
 *
 * Runs in a fixed zone west of UTC (node --test gives each file its own
 * process), where reading `2026-10-03` as UTC midnight would land on the
 * previous local day, so the date-only edge is actually exercised.
 */
process.env.TZ = 'America/Los_Angeles';

import { test } from 'node:test';
import assert from 'node:assert/strict';
import {
  dateInputValue,
  dayDelta,
  dueLabel,
  formatMoment,
  fromInputs,
  isOverdue,
  momentToDate,
  parseMoment,
  rescheduleTo,
  sameMoment,
  shiftDays,
  storedDayKey,
  storedTime,
  timeInputValue,
} from '../src/features/tasks/dueDate.ts';
import { groupByDay, isoDayKey } from '../src/features/tasks/calendarGrid.ts';

test('the test zone is really west of UTC', () => {
  // January: PST, UTC-8. If TZ did not apply, every edge case below is vacuous.
  assert.equal(new Date(2026, 0, 1).getTimezoneOffset(), 480);
});

test('date-only values are local days, not UTC midnight', () => {
  const m = parseMoment('2026-10-03');
  assert.deepEqual(m, { kind: 'date', year: 2026, month: 10, day: 3 });
  const local = momentToDate(m!);
  assert.equal(local.getDate(), 3);
  assert.equal(local.getHours(), 0);
  assert.equal(storedDayKey('2026-10-03'), '2026-10-03');
  // The trap this guards against:
  assert.equal(new Date('2026-10-03').getDate(), 2);
  assert.equal(isoDayKey('2026-10-03'), '2026-10-03');
});

test('naive date-times are local wall-clock times', () => {
  assert.deepEqual(parseMoment('2026-10-03T17:00'), {
    kind: 'datetime', year: 2026, month: 10, day: 3, hour: 17, minute: 0, second: 0,
  });
  assert.deepEqual(parseMoment('2026-10-03 09:05:30'), {
    kind: 'datetime', year: 2026, month: 10, day: 3, hour: 9, minute: 5, second: 30,
  });
  // Local midnight stays on its own day.
  assert.equal(storedDayKey('2026-10-03T00:00'), '2026-10-03');
  assert.equal(storedDayKey('2026-10-03T23:59'), '2026-10-03');
});

test('RFC 3339 instants are shown on their local day', () => {
  // 03:00 UTC on Oct 3 is 20:00 on Oct 2 in Los Angeles (PDT, UTC-7).
  assert.equal(storedDayKey('2026-10-03T03:00:00Z'), '2026-10-02');
  assert.equal(timeInputValue('2026-10-03T03:00:00Z'), '20:00');
  assert.equal(storedDayKey('2026-10-03T03:00:00+05:30'), '2026-10-02');
  assert.equal(storedDayKey('2026-10-03T12:00:00.123-07:00'), '2026-10-03');
});

test('invalid and empty values do not parse', () => {
  for (const bad of ['', '   ', 'tomorrow', '2026-13-01', '2026-02-30', '2026-10-03T24:00', '2026-10-03T12:60', '10/03/2026']) {
    assert.equal(parseMoment(bad), null, bad);
  }
  assert.equal(parseMoment(null), null);
  assert.equal(parseMoment(undefined), null);
  assert.equal(storedTime('nope'), null);
});

test('stored values round-trip unchanged', () => {
  for (const v of ['2026-10-03', '2026-10-03T17:00', '2026-10-03T00:00', '2026-02-28T23:59:59', '2028-02-29']) {
    assert.equal(formatMoment(parseMoment(v)!), v);
  }
  // Seconds are dropped only when zero.
  assert.equal(formatMoment(parseMoment('2026-10-03T17:00:00')!), '2026-10-03T17:00');
});

test('form inputs map to the stored shapes and back', () => {
  assert.equal(fromInputs('2026-10-03', ''), '2026-10-03');
  assert.equal(fromInputs('2026-10-03', '17:30'), '2026-10-03T17:30');
  assert.equal(fromInputs('2026-10-03', '00:00'), '2026-10-03T00:00');
  assert.equal(fromInputs('', '17:30'), null);
  assert.equal(fromInputs('2026-02-30', ''), null);
  assert.equal(fromInputs('2026-10-03', '25:00'), null);

  for (const v of ['2026-10-03', '2026-10-03T17:30', '2026-10-03T00:00']) {
    assert.equal(fromInputs(dateInputValue(v), timeInputValue(v)), v);
  }
  assert.equal(timeInputValue('2026-10-03'), '');
  assert.equal(dateInputValue(null), '');
});

test('sameMoment ignores spelling, not meaning', () => {
  assert.equal(sameMoment('2026-10-03T17:00:00', '2026-10-03T17:00'), true);
  assert.equal(sameMoment('2026-10-03', '2026-10-03T00:00'), false);
  assert.equal(sameMoment(null, undefined), true);
  assert.equal(sameMoment('2026-10-03', null), false);
});

test('rescheduling keeps the shape and wall-clock time', () => {
  assert.equal(rescheduleTo('2026-10-03', '2026-10-09'), '2026-10-09');
  assert.equal(rescheduleTo('2026-10-03T17:30', '2026-10-09'), '2026-10-09T17:30');
  assert.equal(rescheduleTo('2026-10-03T00:00', '2026-11-01'), '2026-11-01T00:00');
  assert.equal(rescheduleTo('2026-10-03', '2026-02-30'), null);
  assert.equal(rescheduleTo('garbage', '2026-10-09'), null);
});

test('reschedule math across the DST boundary', () => {
  // US DST ends on 2026-11-01: that day is 25 hours long in Los Angeles.
  assert.equal(dayDelta('2026-10-31', '2026-11-02'), 2);
  assert.equal(shiftDays('2026-10-31T09:00', 2), '2026-11-02T09:00');
  assert.equal(shiftDays('2026-10-31T23:30', 1), '2026-11-01T23:30');
  // DST starts on 2026-03-08 (23-hour day); 02:30 does not exist that day,
  // but a stored wall-clock value is kept as written.
  assert.equal(dayDelta('2026-03-07', '2026-03-09'), 2);
  assert.equal(rescheduleTo('2026-03-01T02:30', '2026-03-08'), '2026-03-08T02:30');
  assert.equal(shiftDays('2026-03-07', 1), '2026-03-08');
  assert.equal(shiftDays('2026-01-31', 29), '2026-03-01');
  assert.equal(dayDelta('2026-10-09', '2026-10-03'), -6);
});

test('overdue is by local day', () => {
  const now = new Date(2026, 9, 3, 10, 0);
  assert.equal(isOverdue('2026-10-02', now), true);
  assert.equal(isOverdue('2026-10-03', now), false);
  assert.equal(isOverdue('2026-10-03T08:00', now), false);
  assert.equal(isOverdue('2026-10-02T23:59', now), true);
  assert.equal(isOverdue(null, now), false);
  // Late evening local time, early next day UTC: still not overdue.
  assert.equal(isOverdue('2026-10-03', new Date(2026, 9, 3, 23, 30)), false);
});

test('due labels are relative to the local day', () => {
  const now = new Date(2026, 9, 3, 23, 30);
  assert.equal(dueLabel('2026-10-03', now), 'Today');
  assert.equal(dueLabel('2026-10-04', now), 'Tomorrow');
  assert.equal(dueLabel('2026-10-02', now), 'Yesterday');
  assert.match(dueLabel('2026-10-04T09:00', now), /^Tomorrow \S/);
});

test('calendar grouping places date-only tasks on their own day', () => {
  const map = groupByDay(
    [{ id: 'a', due: '2026-10-03' }, { id: 'b', due: '2026-10-03T00:00' }, { id: 'c', due: null }],
    t => t.due,
  );
  assert.deepEqual([...map.keys()], ['2026-10-03']);
  assert.equal(map.get('2026-10-03')!.length, 2);
});
