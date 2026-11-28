/**
 * A task's recorded changes in words: "Shodh · Due: Thu 27 Nov → Fri 28 Nov · 2 h ago".
 * Pure (tested in tests/taskHistory.test.ts).
 */
import { formatDate } from '../../i18n/format.ts';
import { PRIORITY_LABELS, STATUS_LABELS } from './types.ts';
import type { TaskChange } from './types.ts';

export interface HistoryRow {
  key: string;
  who: string;
  what: string;
  when: string;
}

const WHO: Record<string, string> = {
  user: 'You',
  agent: 'Shodh',
  phone: 'Your phone',
  automation: 'Automation',
  import: 'Moved here',
  system: 'Shodh',
};

const FIELD: Record<string, string> = {
  title: 'Title',
  notes: 'Notes',
  due: 'Due',
  priority: 'Priority',
  status: 'Status',
  tags: 'Labels',
  project: 'Project',
  workspace: 'Workspace',
  reminder: 'Reminder',
  subtasks: 'Checklist',
  parent: 'Parent',
  owner: 'Owner',
  link: 'Link',
};

/** A value as the row says it. */
function said(field: string, value: unknown): string {
  if (value === null || value === undefined || value === '') return 'none';
  if (field === 'status' && typeof value === 'string') return STATUS_LABELS[value] ?? value;
  if (field === 'priority' && typeof value === 'string') return PRIORITY_LABELS[value] ?? value;
  if (Array.isArray(value)) return value.length === 0 ? 'none' : `${value.length}`;
  if (typeof value === 'object') {
    const kind = (value as { kind?: unknown }).kind;
    if (field === 'owner' && typeof kind === 'string') return kind === 'me' ? 'You' : kind === 'shodh' ? 'Shodh' : String((value as { ref?: unknown }).ref ?? 'Someone');
    return 'changed';
  }
  const text = String(value);
  return text.length > 40 ? `${text.slice(0, 39)}…` : text;
}

/** When, in short words: "just now", "5 min ago", "2 h ago", "28 Nov". */
export function whenText(at: string, now: Date): string {
  const t = new Date(at);
  if (Number.isNaN(t.getTime())) return '';
  const minutes = Math.round((now.getTime() - t.getTime()) / 60_000);
  if (minutes < 1) return 'just now';
  if (minutes < 60) return `${minutes} min ago`;
  if (minutes < 24 * 60) return `${Math.round(minutes / 60)} h ago`;
  return formatDate(t, { day: 'numeric', month: 'short' });
}

/** The rows of a task's History, newest first. */
export function historyRows(changes: readonly TaskChange[], now: Date): HistoryRow[] {
  return changes
    .map((change, index) => {
      let what: string;
      if (change.field === 'created') what = 'Created';
      else if (change.field === 'imported') what = 'Kept from the old task list';
      else if (change.field === 'deleted') what = 'Deleted';
      else {
        const field = FIELD[change.field] ?? change.field;
        what = `${field}: ${said(change.field, change.before)} → ${said(change.field, change.after)}`;
      }
      if (change.reason) what = `${what} (${change.reason})`;
      return { key: `${index}-${change.at}`, who: WHO[change.actor] ?? change.actor, what, when: whenText(change.at, now) };
    })
    .reverse();
}
