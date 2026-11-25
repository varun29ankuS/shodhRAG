/**
 * The Inbox: items as the backend sends them (`inbox_list`), what each one
 * offers, and the panel's keys. Pure (no runtime imports), so Node's
 * strip-types runner tests it directly (`app/tests/inbox.test.ts`).
 */

export type InboxStatus = 'working' | 'needs_you' | 'done' | 'failed';
export type InboxKind =
  | 'approval'
  | 'memory'
  | 'reminder'
  | 'indexing'
  | 'tables'
  | 'citation_graph'
  | 'export'
  | 'install'
  | 'connector'
  /** The user's chosen model keeps failing; Open goes to Settings → Model. */
  | 'model'
  /** Scheduled work that needs the user, e.g. a repeating task that stopped after failing. */
  | 'automation'
  /** What's new: files new in a watched folder, one item per folder and day. */
  | 'whats_new';

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
const KINDS: readonly InboxKind[] = ['approval', 'memory', 'reminder', 'indexing', 'tables', 'citation_graph', 'export', 'install', 'connector', 'model', 'automation', 'whats_new'];

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

/**
 * The Settings section an item's "Open" shows, when its link goes to Settings: a
 * memory suggestion is decided in Settings → Memory (the link names the view only); a
 * model that keeps failing is changed in Settings → Model.
 */
export function settingsSectionOf(item: Pick<InboxItem, 'kind' | 'link'>): 'memory' | 'models' | null {
  if (item.link?.view !== 'settings') return null;
  if (item.kind === 'memory') return 'memory';
  if (item.kind === 'model') return 'models';
  return null;
}

export type PrimaryAction = 'approve' | 'accept' | 'install';

/** `data.action` of the Install item that offers the scan reader (OCR model). */
export const INSTALL_SCAN_READER = 'install_scan_reader';
export type SecondaryAction = 'deny' | 'dismiss';

export interface ItemActions {
  /** A: Approve (an approval), Accept (a memory suggestion) or Install (an offered download). */
  primary: PrimaryAction | null;
  /** D: Deny (an approval) or Dismiss (anything else that is not running). */
  secondary: SecondaryAction | null;
  /** One-click answers: a repeating task's question, or Resume for one on hold. */
  choices?: InboxChoice[];
}

/** The answers of a repeating task's question (`task_repeat_decide`), and Resume. */
export type InboxChoice = 'make' | 'skip' | 'mark_done' | 'resume';

export const CHOICE_LABEL: Record<InboxChoice, string> = {
  make: 'Make the next one',
  skip: 'Skip this one',
  mark_done: 'Mark done',
  resume: 'Resume',
};

/** What an item offers. Work still running has nothing to decide or dismiss. */
export function actionsFor(item: Pick<InboxItem, 'kind' | 'status'> & { data?: Record<string, unknown> }): ItemActions {
  if (item.kind === 'approval') return { primary: 'approve', secondary: 'deny' };
  // A repeating task's question is answered, never dismissed: the series waits on it.
  if (item.kind === 'reminder' && item.status === 'needs_you' && Array.isArray(item.data?.choices)) {
    return { primary: null, secondary: null, choices: ['make', 'skip', 'mark_done'] };
  }
  if (item.kind === 'automation' && item.data?.fix === 'resume') {
    return { primary: null, secondary: null, choices: ['resume'] };
  }
  // An automation's changes wait for the user's OK in Automations (Open): never dismissed,
  // or the run would wait with nothing to show it.
  if (item.kind === 'automation' && item.status === 'needs_you' && typeof item.data?.approvalId === 'string') {
    return { primary: null, secondary: null };
  }
  if (item.kind === 'memory') return { primary: 'accept', secondary: 'dismiss' };
  if (item.kind === 'install' && item.status === 'needs_you' && item.data?.action === INSTALL_SCAN_READER) {
    return { primary: 'install', secondary: 'dismiss' };
  }
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

/**
 * The Inbox's two tabs (Signal and Noise in the design, never in the UI): Attention
 * holds what waits on the user and what stays stopped until they fix it; Info holds
 * everything else. The bell counts Attention only.
 */
export type InboxTab = 'attention' | 'info';
export const INBOX_TABS: readonly InboxTab[] = ['attention', 'info'];
export const TAB_LABEL: Record<InboxTab, string> = { attention: 'Attention', info: 'Info' };

/**
 * `data.oneOff` (Rust `inbox_commands::ONE_OFF_KEY`): a connector failure that ended
 * by itself (one answer failed, one push was refused, a rate limit that lifts alone).
 */
const ONE_OFF_KEY = 'oneOff';

/**
 * Whether a failed item of each kind stops something the user owns until they act.
 * Keyed by every kind, so a new kind does not compile until it is placed. A failure
 * that `recover` made from interrupted work keeps its working data, so it is never
 * one of these: a restart never adds to Attention.
 */
const FAILED_STOPS: Record<InboxKind, (data: Record<string, unknown>) => boolean> = {
  approval: () => false,
  memory: () => false,
  reminder: () => false,
  // A folder that could not be synced or indexed at all; a sync that only missed some
  // files is Info.
  indexing: data => data.failure === 'sync' || data.failure === 'index',
  tables: () => false,
  citation_graph: () => false,
  export: () => false,
  install: () => false,
  // A connection lost or stopped, missing permissions, today's cap: stopped until fixed.
  connector: data => data[ONE_OFF_KEY] !== true,
  // The chosen model has been failing for over an hour.
  model: () => true,
  // One failed run; three in a row become a needs-you hold.
  automation: () => false,
  // An update: it never stops anything.
  whats_new: () => false,
};

/** The tab of an item. */
export function tabOf(item: Pick<InboxItem, 'kind' | 'status'> & { data?: Record<string, unknown> }): InboxTab {
  if (item.status === 'needs_you') return 'attention';
  if (item.status === 'failed' && FAILED_STOPS[item.kind](item.data ?? {})) return 'attention';
  return 'info';
}

/** The items of one tab, in the order given. */
export function tabItems(items: readonly InboxItem[], tab: InboxTab): InboxItem[] {
  return items.filter(i => tabOf(i) === tab);
}

/** How many items are in Attention (the bell's count and the Attention tab's). */
export function attentionCount(items: readonly InboxItem[]): number {
  return tabItems(items, 'attention').length;
}

/**
 * `data.capped` (Rust `inbox_commands::MEMORY_CAPPED_KEY`): more memory suggestions
 * wait than the Inbox lists.
 */
const MEMORY_CAPPED_KEY = 'capped';

/** Whether more waits than the Inbox lists (memory suggestions past their cap). */
export function attentionCapped(items: readonly InboxItem[]): boolean {
  return items.some(i => i.kind === 'memory' && i.data[MEMORY_CAPPED_KEY] === true);
}

/** A count for a pill: `n`, or `${max}+` past `max`, or `${n}+` when more wait than listed. */
export function countLabel(n: number, max: number, more = false): string {
  if (n > max) return `${max}+`;
  return more ? `${n}+` : String(n);
}

/** The widest Attention count shown before it reads `99+`. */
export const ATTENTION_MAX = 99;

/** The Attention count as shown, the same on the bell and on the Attention tab. */
export function attentionLabel(items: readonly InboxItem[]): string {
  return countLabel(attentionCount(items), ATTENTION_MAX, attentionCapped(items));
}

/** The counts the bell and the Attention tab show, and the bell's name. */
export interface InboxCounts {
  /** The Attention tab's count; `null` before the first answer, or when it failed with nothing to count. */
  pill: string | null;
  /** The bell's badge: the same text as the pill, hidden when nothing needs the user. */
  bell: string | null;
  bellAria: string;
}

export function inboxCounts(input: { loaded: boolean; error: string | null; items: readonly InboxItem[] }): InboxCounts {
  const known = input.loaded && !(input.error !== null && input.items.length === 0);
  const count = attentionCount(input.items);
  const label = attentionLabel(input.items);
  const bell = known && count > 0 ? label : null;
  const bellAria =
    bell === null ? 'Inbox' : label === '1' ? 'Inbox, 1 needs your attention' : `Inbox, ${label} need your attention`;
  return { pill: known ? label : null, bell, bellAria };
}

/** Whether the panel is open, and which tab it shows. Every open starts on Attention, what the bell counted. */
export interface PanelNav {
  open: boolean;
  tab: InboxTab;
}

export type PanelNavAction = { type: 'toggle' } | { type: 'open' } | { type: 'close' } | { type: 'show'; tab: InboxTab };

export function panelNav(state: PanelNav, action: PanelNavAction): PanelNav {
  switch (action.type) {
    case 'open':
      return { open: true, tab: 'attention' };
    case 'toggle':
      return state.open ? { ...state, open: false } : { open: true, tab: 'attention' };
    case 'close':
      return state.open ? { ...state, open: false } : state;
    case 'show':
      return state.tab === action.tab ? state : { ...state, tab: action.tab };
  }
}

interface ClosestTarget {
  closest(selector: string): unknown;
}

interface Container<T> {
  contains(node: T | null): boolean;
}

/** Marks the Inbox's item list, and each item's row button inside it. */
export const LIST_SELECTOR = '[data-inbox-list]';
export const ROW_SELECTOR = '[data-inbox-row]';

/**
 * Where a key in the panel came from: an item's row, one of its buttons, the rest of
 * the panel (the tabs, "Why is this here?", Show Info, Try again), or outside it (the
 * pop-up, a portal whose keys bubble to the panel through React).
 */
export type FocusZone = 'row' | 'item' | 'panel' | 'outside';

export function focusZone<T extends ClosestTarget>(target: T | null, panel: Container<T> | null): FocusZone {
  if (!target || !panel || !panel.contains(target)) return 'outside';
  if (target.closest(ROW_SELECTOR) !== null) return 'row';
  if (target.closest(LIST_SELECTOR) !== null) return 'item';
  return 'panel';
}

/**
 * What a panel key does from `zone`: J/K move from anywhere in the panel; Enter opens
 * only from a row (on a button it presses the button); A, D and U act only from the
 * item list, so a key on a tab never acts on an item it does not show.
 */
export function keyAction(command: InboxCommand, zone: FocusZone): InboxCommand | null {
  if (zone === 'outside') return null;
  if (command === 'next' || command === 'previous') return command;
  if (command === 'open') return zone === 'row' ? command : null;
  return zone === 'row' || zone === 'item' ? command : null;
}

/** Whether a press at `target` closes the panel: outside it, and not in a pop-up (a portal) over it. */
export function pointerDownCloses<T extends ClosestTarget>(target: T | null, root: Container<T> | null): boolean {
  if (!root) return false;
  if (!target) return true;
  if (root.contains(target)) return false;
  return target.closest('[role="dialog"]') === null;
}

/** Only the latest of overlapping `inbox_list` reads is applied: an older answer never overwrites a newer one. */
export interface RequestGate {
  begin(): number;
  isLatest(ticket: number): boolean;
}

export function requestGate(): RequestGate {
  let issued = 0;
  return {
    begin: () => ++issued,
    isLatest: ticket => ticket === issued,
  };
}

/** The Try again button: busy while the read it started runs. */
export function retryButton(pending: boolean): { label: string; disabled: boolean } {
  return pending ? { label: 'Trying again…', disabled: true } : { label: 'Try again', disabled: false };
}

/** What the open tab shows. A first load that has not answered is never "empty". */
export type PanelState = 'loading' | 'error' | 'empty' | 'list';

export function panelState(input: { loaded: boolean; error: string | null; count: number }): PanelState {
  if (!input.loaded && input.error === null) return 'loading';
  if (input.error !== null && input.count === 0) return 'error';
  if (input.count === 0) return 'empty';
  return 'list';
}

export interface EmptyCopy {
  title: string;
  detail: string;
}

export const EMPTY_COPY: Record<InboxTab, EmptyCopy> = {
  attention: { title: 'Nothing needs your attention.', detail: 'Shodh puts approvals, reminders and suggestions here.' },
  info: { title: 'Nothing new.', detail: 'Finished work and updates show here.' },
};

/** The fixed rules, shown in "Why is this here?". */
export const TAB_RULES: Record<InboxTab, string> = {
  attention:
    'Attention has what waits for you (approvals, suggestions, reminders, a repeating task’s question, something on hold) and what stays stopped until you fix it: a model that keeps failing, a connector that stopped, a folder that could not be synced or indexed. The bell counts these.',
  info: 'Info has everything else: work in progress, finished work, updates, and one-off failures that need nothing more from you.',
};

/**
 * The item automations post while they wait so chat keeps working (PR 4), updated in
 * place, never repeated: kind `automation`, status `working`, `data: {paused: 'chat',
 * resumesAt}`. It is Info; after a restart `recover` marks it failed, still Info.
 */
export const AUTOMATION_YIELD_ID = 'automation:yield';

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
