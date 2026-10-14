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

// ── Workspace filter of the Tasks list ─────────────────────────────────────

/** Which tasks the Tasks list shows by workspace: all, those in none, or one workspace's. */
export type WorkspaceFilter =
  | { kind: 'all' }
  | { kind: 'none' }
  | { kind: 'workspace'; id: string };

export const ALL_WORKSPACES: WorkspaceFilter = { kind: 'all' };

/** Stored form of a workspace's filter; the prefix keeps an id from reading as `all` or `none`. */
const WORKSPACE_PREFIX = 'workspace:';

/** The filter as stored per viewer. */
export function serializeWorkspaceFilter(filter: WorkspaceFilter): string {
  switch (filter.kind) {
    case 'all':
      return 'all';
    case 'none':
      return 'none';
    case 'workspace':
      return `${WORKSPACE_PREFIX}${filter.id}`;
  }
}

/** A stored filter; anything missing or unreadable is All. */
export function parseWorkspaceFilter(raw: string | null | undefined): WorkspaceFilter {
  if (raw === 'none') return { kind: 'none' };
  if (raw?.startsWith(WORKSPACE_PREFIX)) {
    const id = raw.slice(WORKSPACE_PREFIX.length);
    if (id) return { kind: 'workspace', id };
  }
  return ALL_WORKSPACES;
}

export function sameWorkspaceFilter(a: WorkspaceFilter, b: WorkspaceFilter): boolean {
  return serializeWorkspaceFilter(a) === serializeWorkspaceFilter(b);
}

/** Whether `task` shows under `filter`. */
export function matchesWorkspaceFilter(task: Pick<TodoItem, 'workspaceId'>, filter: WorkspaceFilter): boolean {
  switch (filter.kind) {
    case 'all':
      return true;
    case 'none':
      return !task.workspaceId;
    case 'workspace':
      return task.workspaceId === filter.id;
  }
}

/**
 * The workspaces the filter offers: those not archived, plus archived ones
 * that still hold tasks (else those tasks would be reachable only under All)
 * and the one chosen (so the choice stays visible).
 */
export function workspaceFilterChoices<W extends { id: string; archived: boolean }>(
  workspaces: readonly W[],
  tasks: readonly Pick<TodoItem, 'workspaceId'>[],
  filter: WorkspaceFilter,
): W[] {
  const used = new Set(tasks.map(t => t.workspaceId).filter((id): id is string => !!id));
  if (filter.kind === 'workspace') used.add(filter.id);
  return workspaces.filter(w => !w.archived || used.has(w.id));
}

/**
 * The filter in effect: a remembered workspace that is no longer listed
 * (deleted) falls back to All. Only once the list has loaded (`listed`),
 * so a slow load never discards a valid choice.
 */
export function effectiveWorkspaceFilter(
  filter: WorkspaceFilter,
  workspaces: readonly { id: string }[],
  listed: boolean,
): WorkspaceFilter {
  if (filter.kind !== 'workspace' || !listed) return filter;
  return workspaces.some(w => w.id === filter.id) ? filter : ALL_WORKSPACES;
}
