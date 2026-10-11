/**
 * Conversation history grouping for the sidebar and command palette.
 *
 * Pure module (no runtime imports) so it is unit-tested directly with Node
 * (`app/tests/conversationGroups.test.ts`).
 */

export type HistoryGroupId = 'pinned' | 'today' | 'yesterday' | 'previous7' | 'older';

export const HISTORY_GROUP_LABELS: Record<HistoryGroupId, string> = {
  pinned: 'Pinned',
  today: 'Today',
  yesterday: 'Yesterday',
  previous7: 'Previous 7 days',
  older: 'Older',
};

const GROUP_ORDER: readonly HistoryGroupId[] = ['pinned', 'today', 'yesterday', 'previous7', 'older'];

/** The fields grouping needs; `Conversation` satisfies it. */
export interface HistoryItem {
  id: string;
  title: string;
  updatedAt: string;
  pinned: boolean;
}

export interface HistoryGroup<T extends HistoryItem> {
  id: HistoryGroupId;
  label: string;
  items: T[];
}

/** Local midnight at the start of the day containing `date`. */
function startOfLocalDay(date: Date): number {
  return new Date(date.getFullYear(), date.getMonth(), date.getDate()).getTime();
}

/** Local midnight `days` calendar days before the day containing `date` (DST-safe). */
function localDayOffset(date: Date, days: number): number {
  return new Date(date.getFullYear(), date.getMonth(), date.getDate() - days).getTime();
}

/**
 * Which date bucket an ISO timestamp falls in, by local calendar day.
 * Unparseable timestamps are 'older'; timestamps in the future (clock skew)
 * are 'today'.
 */
export function historyBucket(updatedAt: string, now: Date): Exclude<HistoryGroupId, 'pinned'> {
  const time = new Date(updatedAt).getTime();
  if (Number.isNaN(time)) return 'older';
  const today = startOfLocalDay(now);
  if (time >= today) return 'today';
  if (time >= localDayOffset(now, 1)) return 'yesterday';
  if (time >= localDayOffset(now, 7)) return 'previous7';
  return 'older';
}

function timeOf(item: HistoryItem): number {
  const t = new Date(item.updatedAt).getTime();
  return Number.isNaN(t) ? Number.NEGATIVE_INFINITY : t;
}

/**
 * Group conversations for display: pinned first, then Today / Yesterday /
 * Previous 7 days / Older, newest first within each group. Empty groups are
 * omitted.
 */
export function groupConversations<T extends HistoryItem>(items: readonly T[], now: Date): HistoryGroup<T>[] {
  const buckets = new Map<HistoryGroupId, T[]>();
  for (const item of items) {
    const id: HistoryGroupId = item.pinned ? 'pinned' : historyBucket(item.updatedAt, now);
    const list = buckets.get(id);
    if (list) list.push(item);
    else buckets.set(id, [item]);
  }
  const groups: HistoryGroup<T>[] = [];
  for (const id of GROUP_ORDER) {
    const list = buckets.get(id);
    if (!list) continue;
    list.sort((a, b) => timeOf(b) - timeOf(a));
    groups.push({ id, label: HISTORY_GROUP_LABELS[id], items: list });
  }
  return groups;
}

/** A conversation that has never been used: untitled and without messages. */
export function isBlankConversation(item: { title: string; messages?: readonly unknown[] }): boolean {
  return item.title === 'New Chat' && (item.messages?.length ?? 0) === 0;
}
