/**
 * Quick-create task and event drafts: validation, the default end, and the
 * store inputs. Run with Node 22.6+:
 *   node --experimental-strip-types --test app/tests/quickCreate.test.ts
 *
 * Runs in a zone with daylight saving so the hour arithmetic is exercised
 * across a clock change.
 */
process.env.TZ = 'America/Los_Angeles';

import { test } from 'node:test';
import assert from 'node:assert/strict';
import {
  defaultEventDraft,
  defaultTaskDraft,
  endAfterHour,
  eventDraftProblem,
  eventInput,
  quickCreateWorkspace,
  shownProblem,
  taskDraftProblem,
  taskInput,
  withStart,
} from '../src/features/tasks/quickCreate.ts';
import type { EventDraft, TaskDraft } from '../src/features/tasks/quickCreate.ts';

/** A fixed "now" (16 Oct 2026, 12:00 local) so reminder checks do not depend on the clock. */
const NOW = new Date(2026, 9, 16, 12, 0);

const task = (patch: Partial<TaskDraft> = {}): TaskDraft => ({ ...defaultTaskDraft(null), title: 'Read paper', ...patch });
const event = (patch: Partial<EventDraft> = {}): EventDraft => ({
  title: 'Review',
  allDay: false,
  startDate: '2026-10-16',
  startTime: '14:00',
  endDate: '2026-10-16',
  endTime: '15:00',
  ...patch,
});

test('a task needs a title', () => {
  assert.equal(taskDraftProblem(task({ title: '   ' })), 'Add a title.');
  assert.equal(taskDraftProblem(task()), null);
});

test('a reminder needs a time as well as a date', () => {
  assert.equal(taskDraftProblem(task({ reminderDate: '2026-10-17' }), NOW), 'Pick a time for the reminder.');
  assert.equal(taskDraftProblem(task({ reminderDate: '2026-10-17', reminderTime: '09:30' }), NOW), null);
});

test('a reminder in the past is rejected', () => {
  assert.equal(taskDraftProblem(task({ reminderDate: '2026-10-16', reminderTime: '11:59' }), NOW), 'The reminder is in the past.');
  assert.equal(taskDraftProblem(task({ reminderDate: '2026-10-15', reminderTime: '18:00' }), NOW), 'The reminder is in the past.');
  assert.equal(taskDraftProblem(task({ reminderDate: '2026-10-16', reminderTime: '12:01' }), NOW), null);
  assert.equal(taskInput(task({ reminderDate: '2026-10-16', reminderTime: '08:00' }), NOW), null);
});

test('a reminder valid when typed is rejected once its time has passed', () => {
  const draft = task({ reminderDate: '2026-10-16', reminderTime: '12:30' });
  assert.equal(taskDraftProblem(draft, NOW), null);
  assert.ok(taskInput(draft, NOW));
  // Create pressed at 12:31 with the dialog still open.
  const later = new Date(2026, 9, 16, 12, 31);
  assert.equal(taskDraftProblem(draft, later), 'The reminder is in the past.');
  assert.equal(taskInput(draft, later), null);
});

test('the missing-title reason waits until the title was touched or Create tried', () => {
  assert.equal(shownProblem('Add a title.', false), null);
  assert.equal(shownProblem('Add a title.', true), 'Add a title.');
  assert.equal(shownProblem('The reminder is in the past.', false), 'The reminder is in the past.');
  assert.equal(shownProblem(null, true), null);
});

test('a malformed due date is reported', () => {
  assert.equal(taskDraftProblem(task({ dueDate: '2026-02-30' })), 'The due date is not a valid date.');
});

test('task input carries only the fields given, in the stored shapes', () => {
  assert.deepEqual(taskInput(task({ title: '  Read paper ' })), { title: 'Read paper', dueDate: null, reminder: null, workspaceId: null });
  assert.deepEqual(
    taskInput(task({ dueDate: '2026-10-20', reminderDate: '2026-10-19', reminderTime: '18:05', workspaceId: 'ws-1' }), NOW),
    { title: 'Read paper', dueDate: '2026-10-20', reminder: '2026-10-19T18:05', workspaceId: 'ws-1' },
  );
  assert.deepEqual(
    taskInput(task({ dueDate: '2026-10-20', dueTime: '17:00' })),
    { title: 'Read paper', dueDate: '2026-10-20T17:00', reminder: null, workspaceId: null },
  );
  assert.equal(taskInput(task({ title: '' })), null);
});

test('the task workspace defaults to the active chat workspace when it is listed', () => {
  const listed = [{ id: 'ws-1' }, { id: 'ws-2' }];
  assert.equal(quickCreateWorkspace('ws-2', listed), 'ws-2');
  assert.equal(quickCreateWorkspace('gone', listed), null);
  assert.equal(quickCreateWorkspace(undefined, listed), null);
  assert.equal(defaultTaskDraft('ws-1').workspaceId, 'ws-1');
});

test('the end defaults to one hour after the start, across midnight', () => {
  assert.deepEqual(endAfterHour('2026-10-16', '14:30'), { date: '2026-10-16', time: '15:30' });
  assert.deepEqual(endAfterHour('2026-10-16', '23:30'), { date: '2026-10-17', time: '00:30' });
  assert.deepEqual(endAfterHour('2026-12-31', '23:00'), { date: '2027-01-01', time: '00:00' });
  assert.equal(endAfterHour('2026-10-16', ''), null);
  assert.equal(endAfterHour('', '10:00'), null);
});

test('a new event starts at the next full hour and ends an hour later', () => {
  const draft = defaultEventDraft(new Date(2026, 9, 16, 23, 10));
  assert.deepEqual(draft, {
    title: '',
    allDay: false,
    startDate: '2026-10-17',
    startTime: '00:00',
    endDate: '2026-10-17',
    endTime: '01:00',
  });
});

test('moving the start moves an untouched end with it', () => {
  const moved = withStart(event(), { startDate: '2026-10-18', startTime: '09:00' }, false);
  assert.equal(moved.endDate, '2026-10-18');
  assert.equal(moved.endTime, '10:00');
  const kept = withStart(event(), { startDate: '2026-10-18', startTime: '09:00' }, true);
  assert.equal(kept.endDate, '2026-10-16');
  assert.equal(kept.endTime, '15:00');
  // All-day events have no default end time.
  const allDay = withStart(event({ allDay: true, endTime: '' }), { startDate: '2026-10-18' }, false);
  assert.equal(allDay.endDate, '2026-10-18');
  assert.equal(allDay.endTime, '');
});

test('an event needs a title and a start', () => {
  assert.equal(eventDraftProblem(event({ title: ' ' })), 'Add a title.');
  assert.equal(eventDraftProblem(event({ startDate: '' })), 'Pick a start date.');
  assert.equal(eventDraftProblem(event({ startTime: '' })), 'Pick a start time, or choose All day.');
  assert.equal(eventDraftProblem(event({ allDay: true, startTime: '' })), null);
});

test('an end before the start is rejected; an equal end is allowed', () => {
  assert.equal(eventDraftProblem(event({ endTime: '13:59' })), 'The end cannot be before the start.');
  assert.equal(eventDraftProblem(event({ endDate: '2026-10-15', endTime: '20:00' })), 'The end cannot be before the start.');
  assert.equal(eventDraftProblem(event({ endTime: '14:00' })), null);
  assert.equal(eventDraftProblem(event({ allDay: true, endDate: '2026-10-15' })), 'The end cannot be before the start.');
  assert.equal(eventDraftProblem(event({ allDay: true, endDate: '2026-10-16' })), null);
});

test('a timed end needs a time', () => {
  assert.equal(eventDraftProblem(event({ endTime: '' })), 'Pick an end time, or remove the end date.');
});

test('the end is optional', () => {
  assert.equal(eventDraftProblem(event({ endDate: '', endTime: '' })), null);
});

test('event input uses date-only values for all-day events', () => {
  assert.deepEqual(eventInput(event()), {
    title: 'Review',
    startTime: '2026-10-16T14:00',
    endTime: '2026-10-16T15:00',
    allDay: false,
  });
  assert.deepEqual(eventInput(event({ allDay: true, startTime: '14:00', endDate: '2026-10-17', endTime: '15:00' })), {
    title: 'Review',
    startTime: '2026-10-16',
    endTime: '2026-10-17',
    allDay: true,
  });
  assert.deepEqual(eventInput(event({ endDate: '', endTime: '' })), {
    title: 'Review',
    startTime: '2026-10-16T14:00',
    endTime: null,
    allDay: false,
  });
  assert.equal(eventInput(event({ endTime: '13:00' })), null);
});
