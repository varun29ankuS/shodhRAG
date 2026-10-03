/**
 * Task due dates and event times as stored by the backend
 * (`calendar_commands.rs`, `agent_tools.rs::parse_moment`).
 *
 * Three stored shapes exist:
 *   - `YYYY-MM-DD`                    date only (the agent's all-day form)
 *   - `YYYY-MM-DDTHH:MM[:SS]`         local wall-clock time, no offset (the UI's form)
 *   - RFC 3339 with `Z` or an offset  an absolute instant
 *
 * `new Date("2026-10-03")` is UTC midnight, which is the previous day west of
 * Greenwich, so nothing in the UI may hand a stored value to `Date` directly.
 * Everything goes through `parseMoment`, which reads date-only and naive
 * values as local. Writes use the first two shapes only.
 *
 * Pure module (no runtime imports) so it is unit-tested directly with Node
 * (`app/tests/dueDate.test.ts`).
 */

export type Moment =
  | { kind: 'date'; year: number; month: number; day: number }
  | { kind: 'datetime'; year: number; month: number; day: number; hour: number; minute: number; second: number };

const DATE_ONLY = /^(\d{4})-(\d{2})-(\d{2})$/;
const NAIVE = /^(\d{4})-(\d{2})-(\d{2})[T ](\d{2}):(\d{2})(?::(\d{2})(?:\.\d+)?)?$/;
const OFFSET = /^\d{4}-\d{2}-\d{2}[T ]\d{2}:\d{2}(?::\d{2}(?:\.\d+)?)?(?:[Zz]|[+-]\d{2}:?\d{2})$/;

const pad = (n: number, width = 2) => String(n).padStart(width, '0');

function validDate(year: number, month: number, day: number): boolean {
  if (month < 1 || month > 12 || day < 1) return false;
  return day <= new Date(year, month, 0).getDate();
}

function validTime(hour: number, minute: number, second: number): boolean {
  return hour >= 0 && hour <= 23 && minute >= 0 && minute <= 59 && second >= 0 && second <= 59;
}

/** Parse a stored date/time; null when empty or not one of the stored shapes. */
export function parseMoment(value: string | null | undefined): Moment | null {
  if (!value) return null;
  const text = value.trim();

  const date = DATE_ONLY.exec(text);
  if (date) {
    const [year, month, day] = [Number(date[1]), Number(date[2]), Number(date[3])];
    return validDate(year, month, day) ? { kind: 'date', year, month, day } : null;
  }

  const naive = NAIVE.exec(text);
  if (naive) {
    const [year, month, day, hour, minute] = [1, 2, 3, 4, 5].map(i => Number(naive[i]));
    const second = naive[6] ? Number(naive[6]) : 0;
    if (!validDate(year, month, day) || !validTime(hour, minute, second)) return null;
    return { kind: 'datetime', year, month, day, hour, minute, second };
  }

  if (OFFSET.test(text)) {
    const instant = new Date(text);
    if (Number.isNaN(instant.getTime())) return null;
    return fromLocalDate(instant, 'datetime');
  }
  return null;
}

/** The local wall-clock parts of `date` as a moment of `kind`. */
export function fromLocalDate(date: Date, kind: Moment['kind']): Moment {
  const year = date.getFullYear();
  const month = date.getMonth() + 1;
  const day = date.getDate();
  if (kind === 'date') return { kind, year, month, day };
  return { kind, year, month, day, hour: date.getHours(), minute: date.getMinutes(), second: date.getSeconds() };
}

/** Stored form: `YYYY-MM-DD`, or `YYYY-MM-DDTHH:MM` (`:SS` only when non-zero). */
export function formatMoment(moment: Moment): string {
  const day = `${pad(moment.year, 4)}-${pad(moment.month)}-${pad(moment.day)}`;
  if (moment.kind === 'date') return day;
  const seconds = moment.second ? `:${pad(moment.second)}` : '';
  return `${day}T${pad(moment.hour)}:${pad(moment.minute)}${seconds}`;
}

/** Local `Date` for a moment; date-only moments are local midnight. */
export function momentToDate(moment: Moment): Date {
  return moment.kind === 'date'
    ? new Date(moment.year, moment.month - 1, moment.day)
    : new Date(moment.year, moment.month - 1, moment.day, moment.hour, moment.minute, moment.second);
}

/** Local calendar day key (`YYYY-MM-DD`) of a moment. */
export function momentDayKey(moment: Moment): string {
  return `${pad(moment.year, 4)}-${pad(moment.month)}-${pad(moment.day)}`;
}

/** Day key of a stored value, or null when it does not parse. */
export function storedDayKey(value: string | null | undefined): string | null {
  const moment = parseMoment(value);
  return moment ? momentDayKey(moment) : null;
}

/** Milliseconds since the epoch for sorting; null when unparseable. */
export function storedTime(value: string | null | undefined): number | null {
  const moment = parseMoment(value);
  return moment ? momentToDate(moment).getTime() : null;
}

/** Value for `<input type="date">`. */
export function dateInputValue(value: string | null | undefined): string {
  const moment = parseMoment(value);
  return moment ? momentDayKey(moment) : '';
}

/** Value for `<input type="time">`; empty for date-only values. */
export function timeInputValue(value: string | null | undefined): string {
  const moment = parseMoment(value);
  return moment && moment.kind === 'datetime' ? `${pad(moment.hour)}:${pad(moment.minute)}` : '';
}

/**
 * Stored value for a date input plus an optional time input. An empty date
 * yields null; an empty time yields a date-only value.
 */
export function fromInputs(date: string, time: string): string | null {
  const day = DATE_ONLY.exec(date.trim());
  if (!day) return null;
  const [year, month, dayOfMonth] = [Number(day[1]), Number(day[2]), Number(day[3])];
  if (!validDate(year, month, dayOfMonth)) return null;
  const clock = /^(\d{2}):(\d{2})(?::(\d{2}))?$/.exec(time.trim());
  if (!time.trim()) return formatMoment({ kind: 'date', year, month, day: dayOfMonth });
  if (!clock) return null;
  const [hour, minute, second] = [Number(clock[1]), Number(clock[2]), clock[3] ? Number(clock[3]) : 0];
  if (!validTime(hour, minute, second)) return null;
  return formatMoment({ kind: 'datetime', year, month, day: dayOfMonth, hour, minute, second });
}

/**
 * True when two stored values denote the same moment and shape, so an edit
 * that only round-trips the value does not rewrite it.
 */
export function sameMoment(a: string | null | undefined, b: string | null | undefined): boolean {
  const ma = parseMoment(a);
  const mb = parseMoment(b);
  if (!ma || !mb) return (a ?? '') === (b ?? '');
  return formatMoment(ma) === formatMoment(mb);
}

/**
 * Move a stored value to another local day (`YYYY-MM-DD`), keeping its shape
 * and wall-clock time. Null when either side does not parse.
 */
export function rescheduleTo(value: string, targetDayKey: string): string | null {
  const moment = parseMoment(value);
  const target = DATE_ONLY.exec(targetDayKey);
  if (!moment || !target) return null;
  const [year, month, day] = [Number(target[1]), Number(target[2]), Number(target[3])];
  if (!validDate(year, month, day)) return null;
  return formatMoment({ ...moment, year, month, day });
}

/** Whole local calendar days from `from` to `to` (DST-safe). */
export function dayDelta(fromDayKey: string, toDayKey: string): number | null {
  const a = DATE_ONLY.exec(fromDayKey);
  const b = DATE_ONLY.exec(toDayKey);
  if (!a || !b) return null;
  const utcA = Date.UTC(Number(a[1]), Number(a[2]) - 1, Number(a[3]));
  const utcB = Date.UTC(Number(b[1]), Number(b[2]) - 1, Number(b[3]));
  return Math.round((utcB - utcA) / 86_400_000);
}

/**
 * Shift a stored value by whole calendar days, keeping shape and wall-clock
 * time (used to move an event's end with its start).
 */
export function shiftDays(value: string, days: number): string | null {
  const moment = parseMoment(value);
  if (!moment) return null;
  const shifted = new Date(moment.year, moment.month - 1, moment.day + days);
  return formatMoment({
    ...moment,
    year: shifted.getFullYear(),
    month: shifted.getMonth() + 1,
    day: shifted.getDate(),
  });
}

/**
 * A task is overdue once its local due day has passed. A due time earlier
 * today is not overdue yet, matching the list's long-standing behaviour.
 */
export function isOverdue(value: string | null | undefined, now: Date = new Date()): boolean {
  const key = storedDayKey(value);
  if (!key) return false;
  const delta = dayDelta(key, momentDayKey(fromLocalDate(now, 'date')));
  return delta !== null && delta > 0;
}

/** Short relative label for a due value: Today, Tomorrow, weekday or date. */
export function dueLabel(value: string, now: Date = new Date()): string {
  const moment = parseMoment(value);
  if (!moment) return value;
  const date = momentToDate(moment);
  const delta = dayDelta(momentDayKey(fromLocalDate(now, 'date')), momentDayKey(moment)) ?? 0;
  const time = moment.kind === 'datetime' ? ` ${date.toLocaleTimeString(undefined, { hour: 'numeric', minute: '2-digit' })}` : '';
  let day: string;
  if (delta === 0) day = 'Today';
  else if (delta === 1) day = 'Tomorrow';
  else if (delta === -1) day = 'Yesterday';
  else if (delta > 1 && delta < 7) day = date.toLocaleDateString(undefined, { weekday: 'short' });
  else if (moment.year === now.getFullYear()) day = date.toLocaleDateString(undefined, { month: 'short', day: 'numeric' });
  else day = date.toLocaleDateString(undefined, { month: 'short', day: 'numeric', year: 'numeric' });
  return `${day}${time}`;
}

/** Full label for a stored value, e.g. "Saturday, October 3, 2026 at 5:00 PM". */
export function fullLabel(value: string): string {
  const moment = parseMoment(value);
  if (!moment) return value;
  const date = momentToDate(moment);
  const day = date.toLocaleDateString(undefined, { weekday: 'long', month: 'long', day: 'numeric', year: 'numeric' });
  if (moment.kind === 'date') return day;
  return `${day} at ${date.toLocaleTimeString(undefined, { hour: 'numeric', minute: '2-digit' })}`;
}
