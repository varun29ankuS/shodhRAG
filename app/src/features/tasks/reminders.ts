/**
 * Task reminders as the UI shows and edits them.
 *
 * The backend stores a reminder as an absolute local date-time,
 * `YYYY-MM-DDTHH:MM` (`calendar_store.rs::REMINDER_FORMAT`), and rings it
 * once (`reminderFiredAt`). The sheet offers presets relative to the due
 * time; choosing one stores the absolute time it works out to, so moving
 * the due date later does not move the reminder with it.
 *
 * A due date without a time counts from 09:00 that day, so "1 hour before"
 * a date-only task rings at 08:00.
 *
 * Pure module (type-only imports) so it is unit-tested with Node
 * (`app/tests/taskReminders.test.ts`).
 */

import type { TodoItem } from './types.ts';

/** Hour a date-only due date counts from. */
export const DATE_ONLY_DUE_HOUR = 9;

export interface ReminderPreset {
  id: string;
  label: string;
  /** Minutes before the due time. */
  minutesBefore: number;
}

export const REMINDER_PRESETS: readonly ReminderPreset[] = [
  { id: 'at-due', label: 'At due time', minutesBefore: 0 },
  { id: '10m', label: '10 minutes before', minutesBefore: 10 },
  { id: '1h', label: '1 hour before', minutesBefore: 60 },
  { id: '1d', label: '1 day before', minutesBefore: 24 * 60 },
];

const pad = (n: number) => String(n).padStart(2, '0');

const STORED = /^(\d{4})-(\d{2})-(\d{2})T(\d{2}):(\d{2})$/;
const DUE = /^(\d{4})-(\d{2})-(\d{2})(?:[T ](\d{2}):(\d{2})(?::\d{2}(?:\.\d+)?)?)?$/;

/** A local Date as the stored reminder shape. */
export function formatReminder(date: Date): string {
  return `${date.getFullYear()}-${pad(date.getMonth() + 1)}-${pad(date.getDate())}T${pad(date.getHours())}:${pad(date.getMinutes())}`;
}

/** A stored reminder as a local Date; null when it is not one. */
export function parseReminder(value: string | null | undefined): Date | null {
  const m = value ? STORED.exec(value.trim()) : null;
  if (!m) return null;
  const date = new Date(Number(m[1]), Number(m[2]) - 1, Number(m[3]), Number(m[4]), Number(m[5]));
  return date.getMonth() === Number(m[2]) - 1 ? date : null;
}

/**
 * The local moment a due date means for reminders. Due dates with an
 * offset (agent-written RFC 3339) are left out: presets work from the
 * shapes the UI writes, and a custom time still works for those.
 */
export function dueAnchor(dueDate: string | null | undefined): Date | null {
  const m = dueDate ? DUE.exec(dueDate.trim()) : null;
  if (!m) return null;
  const hour = m[4] === undefined ? DATE_ONLY_DUE_HOUR : Number(m[4]);
  const minute = m[5] === undefined ? 0 : Number(m[5]);
  return new Date(Number(m[1]), Number(m[2]) - 1, Number(m[3]), hour, minute);
}

/** The reminder a preset sets for `dueDate`, or null without a usable due date. */
export function presetReminder(dueDate: string | null | undefined, preset: ReminderPreset): string | null {
  const anchor = dueAnchor(dueDate);
  if (!anchor) return null;
  return formatReminder(new Date(anchor.getTime() - preset.minutesBefore * 60_000));
}

/** Which preset (by id) a stored reminder matches, `custom`, or `none`. */
export function reminderChoice(reminder: string | null | undefined, dueDate: string | null | undefined): string {
  if (!reminder) return 'none';
  const match = REMINDER_PRESETS.find(p => presetReminder(dueDate, p) === reminder.trim());
  return match ? match.id : 'custom';
}

export type ReminderStatus = 'none' | 'set' | 'snoozed' | 'rang';

/** Where a task's reminder stands. A done task's reminder never rings. */
export function reminderStatus(task: Pick<TodoItem, 'reminder' | 'snoozedUntil' | 'reminderFiredAt' | 'status'>): ReminderStatus {
  if (!task.reminder) return 'none';
  if (task.reminderFiredAt) return 'rang';
  if (task.snoozedUntil) return 'snoozed';
  return 'set';
}

/** The time the reminder rings next: the snooze, else the reminder. */
export function nextRing(task: Pick<TodoItem, 'reminder' | 'snoozedUntil'>): string | null {
  return task.snoozedUntil || task.reminder || null;
}

/** "Today 09:00", "Tomorrow 08:00", "Mon 3 Nov, 08:00". */
export function reminderLabel(value: string, now: Date = new Date()): string {
  const date = parseReminder(value);
  if (!date) return value;
  const time = `${pad(date.getHours())}:${pad(date.getMinutes())}`;
  const startOf = (d: Date) => new Date(d.getFullYear(), d.getMonth(), d.getDate()).getTime();
  const days = Math.round((startOf(date) - startOf(now)) / 86_400_000);
  if (days === 0) return `Today ${time}`;
  if (days === 1) return `Tomorrow ${time}`;
  if (days === -1) return `Yesterday ${time}`;
  const day = date.toLocaleDateString(undefined, {
    weekday: 'short',
    day: 'numeric',
    month: 'short',
    ...(date.getFullYear() !== now.getFullYear() ? { year: 'numeric' } : {}),
  });
  return `${day}, ${time}`;
}
