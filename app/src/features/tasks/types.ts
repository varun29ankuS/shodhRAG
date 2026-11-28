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
  /** The workspace the task belongs to. It may name a workspace that is no longer listed. */
  workspaceId?: string | null;
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
  /** Its id in its workspace (`THE-12`); absent until the task is stored. */
  key?: string | null;
  /** The project it is in (its name is `project`). */
  projectId?: string | null;
  /** The task it is a sub-issue of. */
  parentId?: string | null;
  /** Who it is for; absent means you. */
  owner?: { kind: 'me' | 'person' | 'shodh'; ref?: string | null } | null;
}

/** One recorded change of a task (`task_db.rs::TaskChange`). */
export interface TaskChange {
  at: string;
  /** `user`, `agent`, `phone`, `automation`, `import` or `system`. */
  actor: string;
  actorRef?: string | null;
  field: string;
  before: unknown;
  after: unknown;
  reason?: string | null;
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

/** Priorities, in the order they sort (`calendar_store.rs::PRIORITIES`); a new task has `none`. */
export const PRIORITIES = ['urgent', 'high', 'medium', 'low', 'none'] as const;

/**
 * Statuses the backend accepts (`calendar_store.rs::STATUSES`): Todo is stored as `pending`
 * and Done as `completed`, the words older clients read.
 */
export const STATUSES = ['backlog', 'pending', 'in_progress', 'completed', 'canceled'] as const;

export const STATUS_LABELS: Record<string, string> = {
  backlog: 'Backlog',
  pending: 'Todo',
  in_progress: 'In progress',
  completed: 'Done',
  canceled: 'Canceled',
};

export const PRIORITY_LABELS: Record<string, string> = {
  urgent: 'Urgent',
  high: 'High',
  medium: 'Medium',
  low: 'Low',
  none: 'No priority',
};

export function isDone(task: Pick<TodoItem, 'status'>): boolean {
  return task.status === 'completed';
}

/** Done or canceled: nothing more to do. */
export function isClosed(task: Pick<TodoItem, 'status'>): boolean {
  return task.status === 'completed' || task.status === 'canceled';
}
