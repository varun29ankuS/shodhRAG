/**
 * Repeating tasks in the sheet: the menu from a due date, the state of a series in
 * words, its actions and its history rows. Run with Node 22.6+:
 *   node --experimental-strip-types --test app/tests/taskRepeat.test.ts
 */
import { test } from 'node:test';
import assert from 'node:assert/strict';
import {
  actionsOf,
  dayLabel,
  deleteToastText,
  historyRow,
  optionIdOf,
  optionsFromDue,
  parseDue,
  seriesState,
  slotText,
} from '../src/features/tasks/repeat.ts';
import type { RepeatRun, RepeatView } from '../src/features/tasks/repeat.ts';

const iso = (y: number, m: number, d: number, h = 0, min = 0) => new Date(y, m - 1, d, h, min).toISOString();

function view(over: Partial<RepeatView> = {}): RepeatView {
  return {
    jobId: 'j1',
    schedule: { kind: 'weekly', days: ['mon'], at: '17:00' },
    label: 'Every week on Mon at 17:00',
    description: 'Repeats every Monday at 17:00',
    nextDueAt: iso(2026, 12, 1, 17, 0),
    state: 'active',
    decision: null,
    hold: null,
    retry: null,
    isLatest: true,
    latestTaskId: 't1',
    ...over,
  };
}

function run(over: Partial<RepeatRun>): RepeatRun {
  return {
    id: 'r',
    jobId: 'j1',
    trigger: 'event',
    scheduledFor: iso(2026, 11, 24, 17, 0),
    missed: 0,
    state: 'succeeded',
    waitReason: null,
    attempt: 0,
    nextAttemptAt: null,
    summary: null,
    errorCode: null,
    errorDetail: null,
    createdAt: iso(2026, 11, 24, 17, 0),
    copyTaskId: null,
    ...over,
  };
}

test('the menu is worded from the due date', () => {
  // Monday 24 November 2025, 17:00.
  const options = optionsFromDue('2025-11-24T17:00');
  assert.deepEqual(options.map(o => o.label), [
    "Doesn't repeat",
    'Every day',
    'Every weekday',
    'Every week on Mon',
    'Every month on the 24th',
  ]);
  assert.deepEqual(options[3].schedule, { kind: 'weekly', days: ['mon'], at: '17:00' });
  assert.deepEqual(options[4].schedule, { kind: 'monthly', day: 24, at: '17:00' });
  const lateInMonth = optionsFromDue('2026-01-31');
  assert.equal(lateInMonth.at(-2)?.label, 'Every month on the 31st');
  assert.equal(lateInMonth.at(-1)?.label, 'Every month on the last day');
  assert.deepEqual(lateInMonth.at(-1)?.schedule, { kind: 'monthly', day: 'last', at: '00:00' });
  assert.deepEqual(optionsFromDue(null).map(o => o.id), ['none'], 'no due date, nothing to repeat on');
  assert.equal(optionsFromDue('2026-11-22')[4].label, 'Every month on the 22nd');
  assert.equal(optionsFromDue('2026-11-11')[4].label, 'Every month on the 11th');
});

test('a stored schedule selects its option, or shows as the current one', () => {
  const options = optionsFromDue('2025-11-24T17:00');
  assert.equal(optionIdOf(null, options), 'none');
  assert.equal(optionIdOf({ kind: 'weekly', days: ['mon'], at: '17:00' }, options), 'weekly');
  assert.equal(optionIdOf({ kind: 'daily', at: '09:00' }, options), 'daily', 'the time follows the due date');
  assert.equal(optionIdOf({ kind: 'weekly', days: ['mon', 'thu'], at: '17:00' }, options), 'current');
});

test('every state of a series has its word, its line and its fix', () => {
  assert.equal(seriesState(null, false), 'no-due');
  assert.equal(seriesState(null, true), 'none');
  assert.deepEqual(slotText(null, false, false), {
    word: '',
    text: 'Needs a due date',
    tooltip: 'Set a due date first; each copy is due on the next date of the repeat.',
  });
  assert.equal(slotText(null, true, false).text, '', 'blank, and the line keeps its height');
  const active = slotText(view(), true, false);
  assert.equal(active.word, 'Next');
  assert.equal(active.text, `${dayLabel(new Date(iso(2026, 12, 1)))}, 17:00`);
  assert.equal(active.tooltip, 'The next one is made when this one is done.');
  assert.deepEqual(actionsOf('active', view()), []);

  const waiting = view({
    state: 'waiting',
    decision: { occurrence: iso(2026, 12, 1, 17), missed: 0, openTaskId: 't1', detail: "Due again today, but last week's isn't done." },
  });
  assert.deepEqual(slotText(waiting, true, false).word, 'Waiting');
  assert.equal(slotText(waiting, true, false).text, "Due again today, but last week's isn't done.");
  assert.deepEqual(actionsOf('waiting', waiting), ['make', 'skip', 'mark_done']);

  const held = view({ state: 'held', hold: { reason: 'failing', detail: "Couldn't repeat: the task list couldn't be read, three times in a row." } });
  assert.equal(slotText(held, true, false).word, 'Paused');
  assert.deepEqual(actionsOf('held', held), ['resume']);

  const now = new Date(2026, 10, 30, 17, 0);
  const retrying = view({ state: 'retrying', retry: { at: new Date(2026, 10, 30, 17, 5).toISOString(), detail: "the task list couldn't be read" } });
  assert.deepEqual(slotText(retrying, true, false, now), {
    word: 'Retrying',
    text: "in 5 min · the task list couldn't be read",
    tooltip: 'Shodh tries 3 times, then pauses the repeat.',
  });

  const older = view({ isLatest: false, latestTaskId: 't2' });
  assert.ok(slotText(older, true, false).text.endsWith('set on the latest copy'));
  assert.deepEqual(actionsOf('active', older), ['open_latest']);
  assert.equal(slotText(view(), true, true).text, dayLabel(new Date(iso(2026, 12, 1))), 'a date-only task shows no time');
});

test('history rows say what happened, with a reason', () => {
  const made = historyRow(run({ summary: "Made when last week's was done (Fri 21 Nov)", copyTaskId: 'rep-j1-202611241700' }));
  assert.deepEqual(
    { word: made.word, reason: made.reason, copy: made.copyTaskId },
    { word: 'Made', reason: "when last week's was done (Fri 21 Nov)", copy: 'rep-j1-202611241700' },
  );
  assert.equal(made.date, dayLabel(new Date(iso(2026, 11, 24, 17))));
  assert.deepEqual(
    [historyRow(run({ trigger: 'manual', summary: 'Repeat turned on by you' })).word, historyRow(run({ trigger: 'manual', summary: 'Repeat turned on by you' })).reason],
    ['Set', 'Repeat turned on by you'],
  );
  assert.equal(historyRow(run({ state: 'skipped', summary: 'Skipped by you' })).reason, 'by you');
  assert.equal(historyRow(run({ state: 'skipped', summary: 'Skipped: the previous run was still going.' })).reason, 'the previous run was still going.');
  const failed = historyRow(run({ state: 'failed', errorCode: 'calendar_unreadable', errorDetail: 'locked' }));
  assert.deepEqual([failed.word, failed.reason], ['Failed', "the task list couldn’t be read"]);
  assert.deepEqual([historyRow(run({ state: 'waiting', waitReason: 'decision' })).word, historyRow(run({ state: 'waiting', waitReason: 'decision' })).reason], ['Waiting', 'for your answer']);
  assert.equal(historyRow(run({ state: 'waiting', waitReason: 'backoff', errorCode: 'calendar_unreadable' })).word, 'Retrying');
});

test('due dates parse as local dates and times', () => {
  assert.deepEqual(parseDue('2026-11-24'), { date: new Date(2026, 10, 24), allDay: true });
  assert.deepEqual(parseDue('2026-11-24T17:30'), { date: new Date(2026, 10, 24, 17, 30), allDay: false });
  assert.equal(parseDue('next week'), null);
  assert.equal(parseDue(undefined), null);
});

test('the delete toast says it stops repeating only for the latest copy', () => {
  assert.equal(deleteToastText('Submit timesheet', true), 'Submit timesheet. It won’t repeat.');
  assert.equal(deleteToastText('Submit timesheet', false), 'Submit timesheet');
});
