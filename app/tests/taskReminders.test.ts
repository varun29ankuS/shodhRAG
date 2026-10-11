/**
 * Task reminders (presets, status) and the update_task / update_event wire
 * arguments that clear optional fields.
 *   node --experimental-strip-types --test app/tests/taskReminders.test.ts
 */
import { test } from 'node:test';
import assert from 'node:assert/strict';
import {
  DATE_ONLY_DUE_HOUR,
  dueAnchor,
  formatReminder,
  nextRing,
  parseReminder,
  presetReminder,
  REMINDER_PRESETS,
  reminderChoice,
  reminderLabel,
  reminderStatus,
} from '../src/features/tasks/reminders.ts';
import { eventArgs, taskArgs } from '../src/features/tasks/taskStore.ts';

const preset = (id: string) => {
  const p = REMINDER_PRESETS.find(x => x.id === id);
  assert.ok(p, id);
  return p;
};

test('presets count back from the due time', () => {
  assert.equal(presetReminder('2026-10-29T17:30', preset('at-due')), '2026-10-29T17:30');
  assert.equal(presetReminder('2026-10-29T17:30', preset('10m')), '2026-10-29T17:20');
  assert.equal(presetReminder('2026-10-29T00:30', preset('1h')), '2026-10-28T23:30');
  assert.equal(presetReminder('2026-11-01T09:00', preset('1d')), '2026-10-31T09:00');
});

test('a date-only due date counts from 09:00', () => {
  assert.equal(DATE_ONLY_DUE_HOUR, 9);
  assert.equal(presetReminder('2026-10-29', preset('at-due')), '2026-10-29T09:00');
  assert.equal(presetReminder('2026-10-29', preset('1h')), '2026-10-29T08:00');
});

test('presets need a due date the UI can read', () => {
  assert.equal(presetReminder(null, preset('1h')), null);
  assert.equal(dueAnchor('2026-10-29T09:00:00Z'), null, 'offsets are not preset anchors');
  assert.equal(dueAnchor('soon'), null);
});

test('stored reminders map back to the preset that made them', () => {
  assert.equal(reminderChoice(null, '2026-10-29T17:30'), 'none');
  assert.equal(reminderChoice('2026-10-29T16:30', '2026-10-29T17:30'), '1h');
  assert.equal(reminderChoice('2026-10-29T16:31', '2026-10-29T17:30'), 'custom');
  assert.equal(reminderChoice('2026-10-29T16:30', null), 'custom');
});

test('reminders round-trip through the stored shape', () => {
  const date = parseReminder('2026-10-29T08:05');
  assert.ok(date);
  assert.equal(formatReminder(date), '2026-10-29T08:05');
  assert.equal(parseReminder('2026-02-30T08:00'), null);
  assert.equal(parseReminder('2026-10-29'), null);
});

test('status follows fired and snoozed state', () => {
  const base = { reminder: '2026-10-29T08:00', snoozedUntil: null, reminderFiredAt: null, status: 'pending' };
  assert.equal(reminderStatus({ ...base, reminder: null }), 'none');
  assert.equal(reminderStatus(base), 'set');
  assert.equal(reminderStatus({ ...base, reminderFiredAt: '2026-10-29T02:30:00Z' }), 'rang');
  assert.equal(reminderStatus({ ...base, snoozedUntil: '2026-10-29T08:10' }), 'snoozed');
  assert.equal(nextRing({ ...base, snoozedUntil: '2026-10-29T08:10' }), '2026-10-29T08:10');
  assert.equal(nextRing(base), '2026-10-29T08:00');
});

test('labels name the day relative to now', () => {
  const now = new Date(2026, 9, 29, 7, 0);
  assert.equal(reminderLabel('2026-10-29T08:00', now), 'Today 08:00');
  assert.equal(reminderLabel('2026-10-30T08:00', now), 'Tomorrow 08:00');
  assert.equal(reminderLabel('2026-10-28T08:00', now), 'Yesterday 08:00');
  assert.match(reminderLabel('2026-11-05T08:00', now), /08:00$/);
});

test('null clears an optional field by name and absent keeps it', () => {
  assert.deepEqual(taskArgs('t1', { title: 'A' }), { id: 't1', title: 'A' });
  assert.deepEqual(
    taskArgs('t1', { dueDate: null, project: null, reminder: null, priority: 'high' }),
    { id: 't1', priority: 'high', clear: ['due_date', 'project', 'reminder'] },
  );
  assert.deepEqual(taskArgs('t1', { project: '  ' }), { id: 't1', clear: ['project'] });
  assert.deepEqual(taskArgs('t1', { reminder: '2026-10-29T08:00' }), { id: 't1', reminder: '2026-10-29T08:00' });
  // Notes are cleared by saving them empty (not a clear name).
  assert.deepEqual(taskArgs('t1', { description: '' }), { id: 't1', description: '' });
  assert.deepEqual(
    eventArgs('e1', { endTime: null, location: null, title: 'Review' }),
    { id: 'e1', title: 'Review', clear: ['end_time', 'location'] },
  );
  assert.deepEqual(eventArgs('e1', { location: 'Room 4B' }), { id: 'e1', location: 'Room 4B' });
});
