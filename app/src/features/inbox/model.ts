/**
 * The Inbox: items as the backend sends them (`inbox_list`), what each one
 * offers, and the panel's keys. Pure (no runtime imports), so Node's
 * strip-types runner tests it directly (`app/tests/inbox.test.ts`).
 */

export type InboxStatus = 'working' | 'needs_you' | 'done' | 'failed';
export type InboxKind = 'approval' | 'memory' | 'reminder' | 'indexing' | 'tables' | 'citation_graph' | 'export' | 'install';

export interface InboxLink {
  /** A view id (`ask`, `library`, `tasks`, `settings`, ...). */
  view: string;
  /** A navigation target for that view (see `features/agent/navigation.ts`). */
  target: unknown;
}

export interface InboxItem {
  id: string;
  kind: InboxKind;
  status: InboxStatus;
  title: string;
  detail: string | null;
  link: InboxLink | null;
  data: Record<string, unknown>;
  createdAt: string;
  updatedAt: string;
}

const STATUSES: readonly InboxStatus[] = ['working', 'needs_you', 'done', 'failed'];
const KINDS: readonly InboxKind[] = ['approval', 'memory', 'reminder', 'indexing', 'tables', 'citation_graph', 'export', 'install'];

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === 'object' && value !== null && !Array.isArray(value);
}

function parseItem(value: unknown): InboxItem | null {
  if (!isRecord(value)) return null;
  const { id, kind, status, title } = value;
  if (typeof id !== 'string' || typeof title !== 'string') return null;
  if (!KINDS.includes(kind as InboxKind) || !STATUSES.includes(status as InboxStatus)) return null;
  const link = isRecord(value.link) && typeof value.link.view === 'string'
    ? { view: value.link.view, target: value.link.target ?? null }
    : null;
  return {
    id,
    kind: kind as InboxKind,
    status: status as InboxStatus,
    title,
    detail: typeof value.detail === 'string' ? value.detail : null,
    link,
    data: isRecord(value.data) ? value.data : {},
    createdAt: typeof value.createdAt === 'string' ? value.createdAt : '',
    updatedAt: typeof value.updatedAt === 'string' ? value.updatedAt : '',
  };
}

/** Items from the backend; malformed ones (a newer build's kinds) are skipped. */
export function parseItems(value: unknown): InboxItem[] {
  return Array.isArray(value) ? value.map(parseItem).filter((i): i is InboxItem => i !== null) : [];
}

export type PrimaryAction = 'approve' | 'accept';
export type SecondaryAction = 'deny' | 'dismiss';

export interface ItemActions {
  /** A: Approve (an approval) or Accept (a memory suggestion). */
  primary: PrimaryAction | null;
  /** D: Deny (an approval) or Dismiss (anything else that is not running). */
  secondary: SecondaryAction | null;
}

/** What an item offers. Work still running has nothing to decide or dismiss. */
export function actionsFor(item: Pick<InboxItem, 'kind' | 'status'>): ItemActions {
  if (item.kind === 'approval') return { primary: 'approve', secondary: 'deny' };
  if (item.kind === 'memory') return { primary: 'accept', secondary: 'dismiss' };
  if (item.status === 'working') return { primary: null, secondary: null };
  return { primary: null, secondary: 'dismiss' };
}

/** Whether the secondary action can be undone (U): Deny cannot, the assistant moves on. */
export function isUndoable(action: SecondaryAction): boolean {
  return action === 'dismiss';
}

export const STATUS_LABEL: Record<InboxStatus, string> = {
  working: 'Working',
  needs_you: 'Needs you',
  done: 'Done',
  failed: 'Failed',
};

/** Token classes of each status: blue working, amber needs you, green done, red failed only. */
export const STATUS_TONE: Record<InboxStatus, { dot: string; text: string }> = {
  working: { dot: 'bg-shodh-info', text: 'text-shodh-info' },
  needs_you: { dot: 'bg-shodh-warning', text: 'text-shodh-warning' },
  done: { dot: 'bg-shodh-success', text: 'text-shodh-success' },
  failed: { dot: 'bg-shodh-error', text: 'text-shodh-error' },
};

/** How many items wait on the user (the bell's count). */
export function waitingCount(items: readonly InboxItem[]): number {
  return items.filter(i => i.status === 'needs_you').length;
}

export type InboxCommand = 'next' | 'previous' | 'open' | 'primary' | 'secondary' | 'undo';

/**
 * The panel's keys: J/K (or the arrows) move, Enter opens, A approves or
 * accepts, D denies or dismisses, U undoes the last dismissal. Keys with
 * Ctrl, Alt or Meta, and keys typed into a text field, are not the panel's.
 */
export function inboxCommand(
  event: { key: string; ctrlKey?: boolean; altKey?: boolean; metaKey?: boolean },
  inTextField = false,
): InboxCommand | null {
  if (event.ctrlKey || event.altKey || event.metaKey || inTextField) return null;
  switch (event.key) {
    case 'j':
    case 'J':
    case 'ArrowDown':
      return 'next';
    case 'k':
    case 'K':
    case 'ArrowUp':
      return 'previous';
    case 'Enter':
      return 'open';
    case 'a':
    case 'A':
      return 'primary';
    case 'd':
    case 'D':
      return 'secondary';
    case 'u':
    case 'U':
      return 'undo';
    default:
      return null;
  }
}

/** The selection after moving by `step` in a list of `count` (clamped, not wrapped). */
export function moveSelection(index: number, count: number, step: 1 | -1): number {
  if (count <= 0) return 0;
  return Math.min(count - 1, Math.max(0, index + step));
}

/** Keep the selection on the same item after the list changed, else at the same place. */
export function keepSelection(previousId: string | null, previousIndex: number, items: readonly InboxItem[]): number {
  if (items.length === 0) return 0;
  const found = previousId === null ? -1 : items.findIndex(i => i.id === previousId);
  if (found >= 0) return found;
  return Math.min(previousIndex, items.length - 1);
}
