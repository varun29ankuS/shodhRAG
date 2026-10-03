/** Wire shapes of the calendar commands (`calendar_commands.rs`, camelCase serde). */

export interface SubTask {
  id: string;
  title: string;
  completed: boolean;
}

export interface TodoItem {
  id: string;
  title: string;
  description: string;
  /** See `dueDate.ts` for the stored shapes. Absent when the task has no due date. */
  dueDate?: string | null;
  priority: string;
  status: string;
  tags: string[];
  subtasks: SubTask[];
  project?: string | null;
  /** `user`, `agent` or `document`. */
  source: string;
  sourceRef?: string | null;
  createdAt: string;
  updatedAt: string;
  completedAt?: string | null;
  /** Local `YYYY-MM-DDTHH:MM` (see `reminders.ts`). */
  reminder?: string | null;
  /** When the reminder rang (RFC 3339); cleared when it changes or is snoozed. */
  reminderFiredAt?: string | null;
  /** A snoozed reminder rings again at this local time instead. */
  snoozedUntil?: string | null;
}

export interface CalendarEvent {
  id: string;
  title: string;
  description: string;
  startTime: string;
  endTime?: string | null;
  allDay: boolean;
  color?: string | null;
  location?: string | null;
  source: string;
  sourceRef?: string | null;
  createdAt: string;
}

/** Priorities the backend and agent use (`default_priority` is `medium`). */
export const PRIORITIES = ['high', 'medium', 'low'] as const;

/** Statuses the backend uses; only `completed` is special-cased there. */
export const STATUSES = ['pending', 'completed'] as const;

export const STATUS_LABELS: Record<string, string> = {
  pending: 'To do',
  completed: 'Done',
};

export const PRIORITY_LABELS: Record<string, string> = {
  high: 'High',
  medium: 'Medium',
  low: 'Low',
};

export function isDone(task: Pick<TodoItem, 'status'>): boolean {
  return task.status === 'completed';
}
