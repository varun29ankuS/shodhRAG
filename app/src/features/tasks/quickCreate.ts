/**
 * Quick-create task and event drafts (the New menu's "New task…" and
 * "New event…" dialogs): validation with the reason shown under the form,
 * the default end one hour after the start, and conversion of the native
 * date and time inputs to the store's inputs (`TasksStore`).
 *
 * Pure module (`.ts` imports only) so it is unit-tested with Node
 * (`app/tests/quickCreate.test.ts`).
 */

import { formatMoment, fromInputs, fromLocalDate, parseMoment, storedTime } from './dueDate.ts';

/** What the task dialog's inputs hold: `<input type="date">` / `type="time"` values. */
export interface TaskDraft {
  title: string;
  dueDate: string;
  dueTime: string;
  reminderDate: string;
  reminderTime: string;
  workspaceId: string | null;
}

/** What the event dialog's inputs hold. */
export interface EventDraft {
  title: string;
  allDay: boolean;
  startDate: string;
  startTime: string;
  endDate: string;
  endTime: string;
}

/** The task store's create input, plus the reminder `create_task` also takes. */
export interface QuickTaskInput {
  title: string;
  dueDate: string | null;
  /** Local `YYYY-MM-DDTHH:MM` (see `reminders.ts`). */
  reminder: string | null;
  workspaceId: string | null;
}

/** The event store's create input. */
export interface QuickEventInput {
  title: string;
  startTime: string;
  endTime: string | null;
  allDay: boolean;
}

/** The message shown when an end precedes the start (as in the event sheet). */
export const END_BEFORE_START = 'The end cannot be before the start.';

const pad = (n: number) => String(n).padStart(2, '0');

/** The workspace a new task starts in: the active chat's, when it is still listed. */
export function quickCreateWorkspace(
  activeWorkspaceId: string | null | undefined,
  listed: readonly { id: string }[],
): string | null {
  if (!activeWorkspaceId) return null;
  return listed.some(w => w.id === activeWorkspaceId) ? activeWorkspaceId : null;
}

export function defaultTaskDraft(workspaceId: string | null): TaskDraft {
  return { title: '', dueDate: '', dueTime: '', reminderDate: '', reminderTime: '', workspaceId };
}

/** Why the task cannot be created yet, or null when it can. */
export function taskDraftProblem(draft: TaskDraft): string | null {
  if (!draft.title.trim()) return 'Add a title.';
  if (draft.dueDate && !fromInputs(draft.dueDate, draft.dueTime)) return 'The due date is not a valid date.';
  if (draft.reminderDate) {
    if (!draft.reminderTime) return 'Pick a time for the reminder.';
    if (!fromInputs(draft.reminderDate, draft.reminderTime)) return 'The reminder is not a valid date and time.';
  }
  return null;
}

/** The store input for a valid draft; null while `taskDraftProblem` reports one. */
export function taskInput(draft: TaskDraft): QuickTaskInput | null {
  if (taskDraftProblem(draft)) return null;
  return {
    title: draft.title.trim(),
    dueDate: draft.dueDate ? fromInputs(draft.dueDate, draft.dueTime) : null,
    reminder: draft.reminderDate ? fromInputs(draft.reminderDate, draft.reminderTime) : null,
    workspaceId: draft.workspaceId || null,
  };
}

/** Date and time inputs one hour after the given ones (crossing midnight); null without both. */
export function endAfterHour(date: string, time: string): { date: string; time: string } | null {
  if (!date || !time) return null;
  const moment = parseMoment(fromInputs(date, time));
  if (!moment || moment.kind !== 'datetime') return null;
  const end = new Date(moment.year, moment.month - 1, moment.day, moment.hour + 1, moment.minute);
  return { date: formatMoment(fromLocalDate(end, 'date')), time: `${pad(end.getHours())}:${pad(end.getMinutes())}` };
}

/** A new event: starting at the next full hour, ending an hour later. */
export function defaultEventDraft(now: Date = new Date()): EventDraft {
  const start = new Date(now.getFullYear(), now.getMonth(), now.getDate(), now.getHours() + 1, 0);
  const startDate = formatMoment(fromLocalDate(start, 'date'));
  const startTime = `${pad(start.getHours())}:00`;
  const end = endAfterHour(startDate, startTime);
  return { title: '', allDay: false, startDate, startTime, endDate: end?.date ?? '', endTime: end?.time ?? '' };
}

/**
 * The draft with a new start. Until the user edits the end, it follows the
 * start: an hour later for timed events, the same day for all-day ones.
 */
export function withStart(
  draft: EventDraft,
  start: { startDate?: string; startTime?: string },
  endEdited: boolean,
): EventDraft {
  const next = { ...draft, ...start };
  if (endEdited) return next;
  if (next.allDay) return { ...next, endDate: next.startDate, endTime: '' };
  const end = endAfterHour(next.startDate, next.startTime);
  return end ? { ...next, endDate: end.date, endTime: end.time } : next;
}

/** Why the event cannot be created yet, or null when it can. */
export function eventDraftProblem(draft: EventDraft): string | null {
  if (!draft.title.trim()) return 'Add a title.';
  if (!draft.startDate) return 'Pick a start date.';
  if (!draft.allDay && !draft.startTime) return 'Pick a start time, or choose All day.';
  const start = fromInputs(draft.startDate, draft.allDay ? '' : draft.startTime);
  if (!start) return 'The start is not a valid date and time.';
  if (!draft.endDate) return null;
  if (!draft.allDay && !draft.endTime) return 'Pick an end time, or remove the end date.';
  const end = fromInputs(draft.endDate, draft.allDay ? '' : draft.endTime);
  if (!end) return 'The end is not a valid date and time.';
  const startMs = storedTime(start);
  const endMs = storedTime(end);
  return startMs !== null && endMs !== null && endMs < startMs ? END_BEFORE_START : null;
}

/** The store input for a valid draft; null while `eventDraftProblem` reports one. */
export function eventInput(draft: EventDraft): QuickEventInput | null {
  if (eventDraftProblem(draft)) return null;
  const time = (value: string) => (draft.allDay ? '' : value);
  const startTime = fromInputs(draft.startDate, time(draft.startTime));
  if (!startTime) return null;
  return {
    title: draft.title.trim(),
    startTime,
    endTime: draft.endDate ? fromInputs(draft.endDate, time(draft.endTime)) : null,
    allDay: draft.allDay,
  };
}
