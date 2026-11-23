/**
 * Repeating tasks in the task sheet: the wire shapes of the `task_repeat_*` commands
 * (`task_repeat.rs`, camelCase), the choices offered from a task's due date, and the
 * words of each state and history row. Pure (no runtime imports), so Node's strip-types
 * runner tests it directly (`app/tests/taskRepeat.test.ts`).
 */

export type WeekDay = 'mon' | 'tue' | 'wed' | 'thu' | 'fri' | 'sat' | 'sun';

/** `shodh_rag::engine::schedule::Schedule`, tagged by `kind`. */
export type Schedule =
  | { kind: 'once'; at: string }
  | { kind: 'daily'; at: string }
  | { kind: 'weekdays'; at: string }
  | { kind: 'weekly'; days: WeekDay[]; at: string }
  | { kind: 'monthly'; day: number | 'last'; at: string }
  | { kind: 'every_hours'; hours: number; from: string };

export interface RepeatView {
  jobId: string;
  schedule: Schedule;
  /** "Every week on Mon" */
  label: string;
  /** "Repeats every Monday at 17:00" */
  description: string;
  /** RFC 3339 */
  nextDueAt?: string | null;
  state: 'active' | 'waiting' | 'retrying' | 'held';
  decision?: { occurrence: string; missed: number; openTaskId: string; detail: string } | null;
  hold?: { reason: string; detail: string } | null;
  retry?: { at: string; detail: string } | null;
  isLatest: boolean;
  latestTaskId: string;
}

export interface RepeatRun {
  id: string;
  jobId: string;
  trigger: 'schedule' | 'catch_up' | 'manual' | 'event' | 'retry';
  scheduledFor?: string | null;
  missed: number;
  state: 'queued' | 'running' | 'waiting' | 'succeeded' | 'failed' | 'skipped' | 'cancelled';
  waitReason?: string | null;
  attempt: number;
  nextAttemptAt?: string | null;
  summary?: string | null;
  errorCode?: string | null;
  errorDetail?: string | null;
  createdAt: string;
  /** The copy this run made, while it exists. */
  copyTaskId?: string | null;
}

export type RepeatChoice = 'make' | 'skip' | 'mark_done';

const WEEKDAYS: readonly WeekDay[] = ['sun', 'mon', 'tue', 'wed', 'thu', 'fri', 'sat'];
const DAY_SHORT = ['Sun', 'Mon', 'Tue', 'Wed', 'Thu', 'Fri', 'Sat'];
const MONTH_SHORT = ['Jan', 'Feb', 'Mar', 'Apr', 'May', 'Jun', 'Jul', 'Aug', 'Sep', 'Oct', 'Nov', 'Dec'];

const pad = (n: number) => String(n).padStart(2, '0');

function ordinal(n: number): string {
  const tens = n % 100;
  if (tens >= 11 && tens <= 13) return `${n}th`;
  switch (n % 10) {
    case 1: return `${n}st`;
    case 2: return `${n}nd`;
    case 3: return `${n}rd`;
    default: return `${n}th`;
  }
}

/** A stored due date (`YYYY-MM-DD` or `YYYY-MM-DDTHH:MM[...]`) as a local date and time. */
export function parseDue(due: string | null | undefined): { date: Date; allDay: boolean } | null {
  if (!due) return null;
  const m = /^(\d{4})-(\d{2})-(\d{2})(?:[T ](\d{2}):(\d{2}))?/.exec(due.trim());
  if (!m) return null;
  const [, y, mo, d, h, mi] = m;
  const allDay = h === undefined;
  const date = new Date(Number(y), Number(mo) - 1, Number(d), allDay ? 0 : Number(h), allDay ? 0 : Number(mi));
  return Number.isNaN(date.getTime()) ? null : { date, allDay };
}

/** "Mon 24 Nov" in local time. */
export function dayLabel(date: Date): string {
  return `${DAY_SHORT[date.getDay()]} ${date.getDate()} ${MONTH_SHORT[date.getMonth()]}`;
}

/** "Mon 24 Nov, 17:00", or the day alone for a date-only task. */
export function whenLabel(iso: string, allDay: boolean): string {
  const date = new Date(iso);
  if (Number.isNaN(date.getTime())) return iso;
  return allDay ? dayLabel(date) : `${dayLabel(date)}, ${pad(date.getHours())}:${pad(date.getMinutes())}`;
}

export interface RepeatOption {
  id: string;
  label: string;
  /** None for "Doesn't repeat". */
  schedule: Schedule | null;
}

/** The choices of the Repeat menu, worded from the task's due date. */
export function optionsFromDue(due: string | null | undefined): RepeatOption[] {
  const none: RepeatOption = { id: 'none', label: "Doesn't repeat", schedule: null };
  const parsed = parseDue(due);
  if (!parsed) return [none];
  const { date, allDay } = parsed;
  const at = allDay ? '00:00' : `${pad(date.getHours())}:${pad(date.getMinutes())}`;
  const day = date.getDate();
  const options: RepeatOption[] = [
    none,
    { id: 'daily', label: 'Every day', schedule: { kind: 'daily', at } },
    { id: 'weekdays', label: 'Every weekday', schedule: { kind: 'weekdays', at } },
    {
      id: 'weekly',
      label: `Every week on ${DAY_SHORT[date.getDay()]}`,
      schedule: { kind: 'weekly', days: [WEEKDAYS[date.getDay()]], at },
    },
    { id: 'monthly', label: `Every month on the ${ordinal(day)}`, schedule: { kind: 'monthly', day, at } },
  ];
  if (day >= 29) {
    options.push({ id: 'monthly-last', label: 'Every month on the last day', schedule: { kind: 'monthly', day: 'last', at } });
  }
  return options;
}

/** Which option a stored schedule is; `current` when it is none of them (set by the assistant). */
export function optionIdOf(schedule: Schedule | null | undefined, options: readonly RepeatOption[]): string {
  if (!schedule) return 'none';
  const same = options.find(o => o.schedule && sameDays(o.schedule, schedule));
  return same ? same.id : 'current';
}

function sameDays(a: Schedule, b: Schedule): boolean {
  if (a.kind !== b.kind) return false;
  if (a.kind === 'weekly' && b.kind === 'weekly') return a.days.join() === b.days.join();
  if (a.kind === 'monthly' && b.kind === 'monthly') return a.day === b.day;
  return a.kind === 'daily' || a.kind === 'weekdays';
}

export type SeriesState = 'no-due' | 'none' | 'active' | 'waiting' | 'retrying' | 'held';

export function seriesState(view: RepeatView | null, hasDue: boolean): SeriesState {
  if (view) return view.state;
  return hasDue ? 'none' : 'no-due';
}

/** The fix the action row offers in a state (the row keeps its height when there is none). */
export function actionsOf(state: SeriesState, view: RepeatView | null): Array<'make' | 'skip' | 'mark_done' | 'resume' | 'open_latest'> {
  if (state === 'waiting') return ['make', 'skip', 'mark_done'];
  if (state === 'held') return ['resume'];
  if (view && !view.isLatest) return ['open_latest'];
  return [];
}

export const ACTION_LABELS: Record<ReturnType<typeof actionsOf>[number], string> = {
  make: 'Make the next one',
  skip: 'Skip this one',
  mark_done: 'Mark done',
  resume: 'Resume',
  open_latest: 'Open latest',
};

export interface SlotText {
  /** The state word, bold: Next, Waiting, Retrying, Paused; empty when nothing repeats. */
  word: string;
  /** One line after it. */
  text: string;
  /** The explanation behind the (i) button. */
  tooltip: string;
}

/** The one-line state of a task's repeat, always present (blank keeps its height). */
export function slotText(view: RepeatView | null, hasDue: boolean, allDay: boolean, now: Date = new Date()): SlotText {
  const state = seriesState(view, hasDue);
  switch (state) {
    case 'no-due':
      return { word: '', text: 'Needs a due date', tooltip: 'Set a due date first; each copy is due on the next date of the repeat.' };
    case 'none':
      return { word: '', text: '', tooltip: 'Make a new copy of this task on a schedule.' };
    case 'waiting':
      return {
        word: 'Waiting',
        text: view?.decision?.detail ?? 'Due again, but the last one isn’t done.',
        tooltip: (view?.decision?.missed ?? 0) > 0
          ? 'Shodh makes one copy at a time. Pick what to do with this one. Only one copy can come of it.'
          : 'Shodh makes one copy at a time. Pick what to do with this one.',
      };
    case 'retrying': {
      const at = view?.retry?.at ? new Date(view.retry.at) : null;
      const minutes = at && !Number.isNaN(at.getTime()) ? Math.max(1, Math.ceil((at.getTime() - now.getTime()) / 60_000)) : null;
      const detail = view?.retry?.detail ?? 'something went wrong';
      return {
        word: 'Retrying',
        text: minutes ? `in ${minutes} min · ${detail}` : detail,
        tooltip: 'Shodh tries 3 times, then pauses the repeat.',
      };
    }
    case 'held':
      return { word: 'Paused', text: view?.hold?.detail ?? 'Paused after failing again and again.', tooltip: 'Fix what failed, then Resume.' };
    case 'active': {
      const next = view?.nextDueAt ? whenLabel(view.nextDueAt, allDay) : '';
      const latest = view && !view.isLatest ? ' · set on the latest copy' : '';
      return {
        word: 'Next',
        text: `${next}${latest}`,
        tooltip: view && !view.isLatest
          ? 'The next one is made when the latest copy is done. Changing Repeat here changes the whole series.'
          : 'The next one is made when this one is done.',
      };
    }
  }
}

export interface HistoryRow {
  id: string;
  date: string;
  word: 'Set' | 'Made' | 'Skipped' | 'Failed' | 'Waiting' | 'Retrying' | 'Stopped';
  reason: string;
  copyTaskId: string | null;
}

function failureWords(code: string | null | undefined, detail: string | null | undefined): string {
  if (code === 'calendar_unreadable') return 'the task list couldn’t be read';
  return detail || 'something went wrong';
}

/** One history row per run: a date, a state word and the reason. */
export function historyRow(run: RepeatRun): HistoryRow {
  const at = new Date(run.scheduledFor ?? run.createdAt);
  const date = Number.isNaN(at.getTime()) ? '' : dayLabel(at);
  const summary = run.summary ?? '';
  const base = { id: run.id, date, copyTaskId: run.copyTaskId ?? null };
  switch (run.state) {
    case 'succeeded':
      if (run.trigger === 'manual') return { ...base, word: 'Set', reason: summary || 'Repeat turned on' };
      return { ...base, word: 'Made', reason: summary.replace(/^Made /, '') || 'made the next copy' };
    case 'skipped':
      return { ...base, word: 'Skipped', reason: summary.replace(/^Skipped(:)? /, '') || 'skipped' };
    case 'failed':
      return { ...base, word: 'Failed', reason: failureWords(run.errorCode, run.errorDetail) };
    case 'waiting':
      return run.waitReason === 'backoff'
        ? { ...base, word: 'Retrying', reason: failureWords(run.errorCode, run.errorDetail) }
        : { ...base, word: 'Waiting', reason: 'for your answer' };
    case 'cancelled':
      return { ...base, word: 'Stopped', reason: summary || 'stopped' };
    case 'queued':
    case 'running':
      return { ...base, word: 'Waiting', reason: 'making the next copy' };
  }
}

/** History shows at most this many rows until "Show all" is pressed. */
export const HISTORY_SHOWN = 5;

/** The undo toast of a deleted task: deleting the latest copy also stops its repeat. */
export function deleteToastText(title: string, stopsRepeat: boolean): string {
  return stopsRepeat ? `Deleted “${title}”. It won’t repeat.` : `Deleted “${title}”`;
}
