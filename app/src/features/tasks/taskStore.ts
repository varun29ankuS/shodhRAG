/**
 * Optimistic store for tasks and events.
 *
 * `tasks`/`events` hold the last server state (a `load_*` snapshot, updated
 * with each command's returned record). Edits in flight live in `pending` and
 * are layered on top by the selectors, so:
 *   - a failed command is rolled back by dropping its op, nothing to restore;
 *   - a refetch triggered by `calendar-changed` (emitted by our own writes)
 *     that lands while another edit is in flight cannot undo that edit on
 *     screen, because the op is re-applied over the new snapshot;
 *   - a delete waiting out its undo window is a `remove` op: hidden, but the
 *     record is untouched until the op commits.
 *
 * Pure module, unit-tested with Node (`app/tests/taskStore.test.ts`).
 */
import type { CalendarEvent, TodoItem } from './types.ts';

export type PendingOp =
  | { opId: number; kind: 'task'; id: string; patch?: Partial<TodoItem>; remove?: boolean }
  | { opId: number; kind: 'event'; id: string; patch?: Partial<CalendarEvent>; remove?: boolean };

export interface StoreState {
  tasks: TodoItem[];
  events: CalendarEvent[];
  pending: PendingOp[];
}

export type StoreAction =
  | { type: 'snapshot'; tasks: TodoItem[]; events: CalendarEvent[] }
  | { type: 'begin'; op: PendingOp }
  /** The command succeeded: drop the op and adopt the server's record (or removal). */
  | { type: 'commit'; opId: number; task?: TodoItem; event?: CalendarEvent; removed?: boolean }
  /** The command failed or was undone: drop the op. */
  | { type: 'rollback'; opId: number }
  /** A record created by a command (before the refetch arrives). */
  | { type: 'insertTask'; task: TodoItem }
  | { type: 'insertEvent'; event: CalendarEvent };

export const initialStore: StoreState = { tasks: [], events: [], pending: [] };

function upsert<T extends { id: string }>(list: T[], item: T, prepend: boolean): T[] {
  const index = list.findIndex(x => x.id === item.id);
  if (index < 0) return prepend ? [item, ...list] : [...list, item];
  const next = list.slice();
  next[index] = item;
  return next;
}

export function storeReducer(state: StoreState, action: StoreAction): StoreState {
  switch (action.type) {
    case 'snapshot':
      return { ...state, tasks: action.tasks, events: action.events };
    case 'begin':
      return { ...state, pending: [...state.pending, action.op] };
    case 'commit': {
      const op = state.pending.find(p => p.opId === action.opId);
      const pending = state.pending.filter(p => p.opId !== action.opId);
      let { tasks, events } = state;
      if (op && action.removed) {
        if (op.kind === 'task') tasks = tasks.filter(t => t.id !== op.id);
        else events = events.filter(e => e.id !== op.id);
      }
      if (action.task) tasks = upsert(tasks, action.task, false);
      if (action.event) events = upsert(events, action.event, false);
      return { tasks, events, pending };
    }
    case 'rollback':
      return { ...state, pending: state.pending.filter(p => p.opId !== action.opId) };
    case 'insertTask':
      return { ...state, tasks: upsert(state.tasks, action.task, true) };
    case 'insertEvent':
      return { ...state, events: upsert(state.events, action.event, false) };
    default:
      return state;
  }
}

function applyOps<T extends { id: string }>(base: T[], ops: { id: string; patch?: Partial<T>; remove?: boolean }[]): T[] {
  if (ops.length === 0) return base;
  const removed = new Set<string>();
  const patches = new Map<string, Partial<T>>();
  for (const op of ops) {
    if (op.remove) removed.add(op.id);
    if (op.patch) patches.set(op.id, { ...patches.get(op.id), ...op.patch });
  }
  const out: T[] = [];
  for (const item of base) {
    if (removed.has(item.id)) continue;
    const patch = patches.get(item.id);
    out.push(patch ? { ...item, ...patch } : item);
  }
  return out;
}

/** Tasks as the user should see them: server state with pending edits applied. */
export function visibleTasks(state: StoreState): TodoItem[] {
  return applyOps(state.tasks, state.pending.filter(p => p.kind === 'task') as { id: string; patch?: Partial<TodoItem>; remove?: boolean }[]);
}

/** Events as the user should see them. */
export function visibleEvents(state: StoreState): CalendarEvent[] {
  return applyOps(state.events, state.pending.filter(p => p.kind === 'event') as { id: string; patch?: Partial<CalendarEvent>; remove?: boolean }[]);
}

/** Ids of tasks whose delete is waiting out its undo window. */
export function pendingTaskRemovals(state: StoreState): Set<string> {
  return new Set(state.pending.filter(p => p.kind === 'task' && p.remove).map(p => p.id));
}

/**
 * Only the fields of `patch` that differ from `current`, so an edit that
 * changes nothing sends nothing (each write re-indexes the task for search).
 */
export function changedFields<T extends object>(current: T, patch: Partial<T>): Partial<T> {
  const out: Partial<T> = {};
  for (const key of Object.keys(patch) as (keyof T)[]) {
    const a = current[key];
    const b = patch[key];
    const same = Array.isArray(a) && Array.isArray(b)
      ? a.length === b.length && a.every((v, i) => JSON.stringify(v) === JSON.stringify(b[i]))
      : (a ?? null) === (b ?? null);
    if (!same) out[key] = b;
  }
  return out;
}

/** Prefix of ids given to subtasks added optimistically, until the server assigns one. */
export const TEMP_SUBTASK_PREFIX = 'pending:';

export function isTempSubtaskId(id: string): boolean {
  return id.startsWith(TEMP_SUBTASK_PREFIX);
}

// ── Wire arguments ─────────────────────────────────────────────────

/** Task fields `update_task` can set; `null` clears an optional one. */
export interface TaskPatch {
  title?: string;
  description?: string;
  dueDate?: string | null;
  priority?: string;
  status?: string;
  tags?: string[];
  project?: string | null;
  reminder?: string | null;
}

/** Event fields `update_event` can set; `null` clears an optional one. */
export interface EventPatch {
  title?: string;
  description?: string;
  startTime?: string;
  endTime?: string | null;
  allDay?: boolean;
  location?: string | null;
}

/** Backend names of the fields a `null` clears (`calendar_store.rs::TASK_CLEARABLE`). */
const TASK_CLEAR: Record<'dueDate' | 'project' | 'reminder', string> = {
  dueDate: 'due_date',
  project: 'project',
  reminder: 'reminder',
};

/** `calendar_store.rs::EVENT_CLEARABLE`. */
const EVENT_CLEAR: Record<'endTime' | 'location', string> = {
  endTime: 'end_time',
  location: 'location',
};

function wireArgs(id: string, patch: object, clearable: Record<string, string>): Record<string, unknown> {
  const args: Record<string, unknown> = { id };
  const clear: string[] = [];
  for (const [key, value] of Object.entries(patch)) {
    if (value === undefined) continue;
    if (value === null || (key in clearable && typeof value === 'string' && value.trim() === '')) {
      // An absent value means "keep" to the backend; clearing must be named.
      // Clear names are values, which Tauri does not case-convert: snake_case.
      if (key in clearable) clear.push(clearable[key]);
      continue;
    }
    args[key] = value;
  }
  if (clear.length > 0) args.clear = clear;
  return args;
}

/** Arguments of `update_task` (camelCase keys; Tauri maps them to snake_case). */
export function taskArgs(id: string, patch: TaskPatch): Record<string, unknown> {
  return wireArgs(id, patch, TASK_CLEAR);
}

/** Arguments of `update_event`. */
export function eventArgs(id: string, patch: EventPatch): Record<string, unknown> {
  return wireArgs(id, patch, EVENT_CLEAR);
}
