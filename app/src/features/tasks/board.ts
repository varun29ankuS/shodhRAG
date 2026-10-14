/**
 * Pure logic of a workspace's task board: which tasks it shows, in which
 * column, in what order, and where the keyboard moves a card.
 *
 * Imports only pure modules with `.ts` extensions, so Node's strip-types test
 * runner loads it directly (`app/tests/taskBoard.test.ts`).
 */
import { storedTime } from './dueDate.ts';
import { PRIORITIES, STATUS_LABELS, STATUSES } from './types.ts';
import type { TodoItem } from './types.ts';

export interface BoardColumn {
  status: string;
  label: string;
  tasks: TodoItem[];
}

/** The column a task shows in: its status, or To do for a status the board does not know. */
export function columnOf(task: Pick<TodoItem, 'status'>): string {
  return (STATUSES as readonly string[]).includes(task.status) ? task.status : 'pending';
}

function priorityRank(priority: string): number {
  const index = (PRIORITIES as readonly string[]).indexOf(priority);
  return index < 0 ? PRIORITIES.length : index;
}

/** Due first (soonest first), then by priority, then newest. */
function compareCards(a: TodoItem, b: TodoItem): number {
  const at = storedTime(a.dueDate);
  const bt = storedTime(b.dueDate);
  if (at !== null && bt !== null && at !== bt) return at - bt;
  if (at !== null && bt === null) return -1;
  if (at === null && bt !== null) return 1;
  const rank = priorityRank(a.priority) - priorityRank(b.priority);
  if (rank !== 0) return rank;
  return (Date.parse(b.createdAt) || 0) - (Date.parse(a.createdAt) || 0);
}

/** The tasks of `workspaceId`, one column per status in the backend's order. */
export function boardColumns(tasks: readonly TodoItem[], workspaceId: string): BoardColumn[] {
  const mine = tasks.filter(t => t.workspaceId === workspaceId);
  return STATUSES.map(status => ({
    status,
    label: STATUS_LABELS[status] ?? status,
    tasks: mine.filter(t => columnOf(t) === status).sort(compareCards),
  }));
}

/** The status one column to the left (`-1`) or right (`1`) of `status`, or null at an edge. */
export function adjacentStatus(status: string, step: -1 | 1): string | null {
  const index = (STATUSES as readonly string[]).indexOf(status);
  if (index < 0) return null;
  return STATUSES[index + step] ?? null;
}

/**
 * How a task's workspace reads: its name, or null when the task has none or
 * names a workspace that is not listed (deleted, or the list is not loaded).
 */
export function workspaceName(
  workspaceId: string | null | undefined,
  workspaces: readonly { id: string; name: string }[],
): string | null {
  if (!workspaceId) return null;
  return workspaces.find(w => w.id === workspaceId)?.name ?? null;
}

/**
 * The workspaces a task can be moved to: those not archived, plus the task's
 * own workspace when it is archived (so it still reads by name and stays
 * checked).
 */
export function workspaceChoices<W extends { id: string; archived: boolean }>(
  workspaces: readonly W[],
  currentId: string | null | undefined,
): W[] {
  return workspaces.filter(w => !w.archived || w.id === currentId);
}
