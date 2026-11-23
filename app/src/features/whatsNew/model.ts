/**
 * Library → What's new: the section's state, its rows (bucketed by day, a folder's
 * busy day folded into one row) and their words. Pure (type imports only), so Node's
 * strip-types runner tests it directly (`app/tests/whatsNew.test.ts`).
 */

import type { FeedItem } from './types';

/** Rows the section shows before "Show all"; the list always reserves their height. */
export const VISIBLE_ROWS = 5;

/** More files than this from one folder on one day become one row. */
export const GROUP_AFTER = 3;

export const EMPTY_COPY = 'Nothing new yet. New files in your Library folders show up here.';

export type SectionState = 'loading' | 'error' | 'empty' | 'list';

/** Loading until the first answer; an error only when there is nothing to show. */
export function sectionState(input: { loaded: boolean; error: string | null; items: number }): SectionState {
  if (!input.loaded && !input.error) return 'loading';
  if (input.items === 0) return input.error ? 'error' : 'empty';
  return 'list';
}

export type Bucket = 'today' | 'this_week' | 'earlier';

export const BUCKET_LABEL: Record<Bucket, string> = {
  today: 'Today',
  this_week: 'Earlier this week',
  earlier: 'Earlier',
};

function startOfLocalDay(at: Date): Date {
  return new Date(at.getFullYear(), at.getMonth(), at.getDate());
}

/** Local midnight of the Monday of `at`'s week. */
function startOfLocalWeek(at: Date): Date {
  const day = startOfLocalDay(at);
  const sinceMonday = (day.getDay() + 6) % 7;
  return new Date(day.getFullYear(), day.getMonth(), day.getDate() - sinceMonday);
}

/** Today, earlier this week (weeks start on Monday) or earlier, in local time. */
export function bucketOf(foundAt: string, now: Date): Bucket {
  const at = new Date(foundAt);
  if (Number.isNaN(at.getTime())) return 'earlier';
  if (at >= startOfLocalDay(now)) return 'today';
  if (at >= startOfLocalWeek(now)) return 'this_week';
  return 'earlier';
}

const WEEKDAYS = ['Sun', 'Mon', 'Tue', 'Wed', 'Thu', 'Fri', 'Sat'];
const MONTHS = ['Jan', 'Feb', 'Mar', 'Apr', 'May', 'Jun', 'Jul', 'Aug', 'Sep', 'Oct', 'Nov', 'Dec'];

function hhmm(at: Date): string {
  return `${String(at.getHours()).padStart(2, '0')}:${String(at.getMinutes()).padStart(2, '0')}`;
}

/** When a row was found: "10:41" today, "Mon 10:41" this week, "12 Nov" before. */
export function foundLabel(foundAt: string, now: Date): string {
  const at = new Date(foundAt);
  if (Number.isNaN(at.getTime())) return '';
  switch (bucketOf(foundAt, now)) {
    case 'today':
      return hhmm(at);
    case 'this_week':
      return `${WEEKDAYS[at.getDay()]} ${hhmm(at)}`;
    case 'earlier':
      return `${at.getDate()} ${MONTHS[at.getMonth()]}`;
  }
}

/** "1 new file in Papers", "3 new files in Papers" (as the Inbox item reads). */
export function digestTitle(count: number, folder: string): string {
  return count === 1 ? `1 new file in ${folder}` : `${count} new files in ${folder}`;
}

/** A row of the section: one file, or a folder's busy day. */
export interface Row {
  key: string;
  kind: 'file' | 'file_group';
  title: string;
  /** Folder and time, on one line. */
  meta: string;
  /** Not seen yet. */
  isNew: boolean;
  /** The rows it stands for, in order. */
  items: FeedItem[];
  /** Files beyond those listed (the sync's cap). */
  more: number;
}

function folderOf(item: FeedItem): string {
  return item.detail ?? (typeof item.data.folder === 'string' ? item.data.folder : '');
}

function sourceOf(item: FeedItem): string {
  return typeof item.data.sourceId === 'string' ? item.data.sourceId : item.jobId;
}

function moreOf(item: FeedItem): number {
  return typeof item.data.more === 'number' && item.data.more > 0 ? item.data.more : 0;
}

function localDayKey(foundAt: string): string {
  const at = new Date(foundAt);
  return `${at.getFullYear()}-${at.getMonth()}-${at.getDate()}`;
}

/**
 * The rows of `items` (newest first, as listed), bucketed. Within a bucket, the files
 * of one folder found on one local day stay one row each up to {@link GROUP_AFTER};
 * past that (or with files beyond the sync's cap) they are one row, placed where its
 * newest file was.
 */
export function rowsOf(items: readonly FeedItem[], now: Date): { bucket: Bucket; rows: Row[] }[] {
  const groups = new Map<string, FeedItem[]>();
  for (const item of items) {
    const key = `${sourceOf(item)}|${localDayKey(item.foundAt)}`;
    const list = groups.get(key);
    if (list) list.push(item);
    else groups.set(key, [item]);
  }
  const placed = new Set<string>();
  const out: { bucket: Bucket; rows: Row[] }[] = [];
  const push = (bucket: Bucket, row: Row) => {
    const last = out[out.length - 1];
    if (last && last.bucket === bucket) last.rows.push(row);
    else out.push({ bucket, rows: [row] });
  };
  for (const item of items) {
    const key = `${sourceOf(item)}|${localDayKey(item.foundAt)}`;
    const group = groups.get(key) ?? [item];
    const more = group.reduce((sum, i) => sum + moreOf(i), 0);
    const bucket = bucketOf(item.foundAt, now);
    if (group.length > GROUP_AFTER || more > 0) {
      if (placed.has(key)) continue;
      placed.add(key);
      const folder = folderOf(item);
      push(bucket, {
        key: `group:${key}`,
        kind: 'file_group',
        title: digestTitle(group.length + more, folder),
        meta: `${folder} · ${foundLabel(item.foundAt, now)}`,
        isNew: group.some(i => !i.seenAt),
        items: group,
        more,
      });
      continue;
    }
    push(bucket, {
      key: item.id,
      kind: 'file',
      title: item.title,
      meta: `${folderOf(item)} · ${foundLabel(item.foundAt, now)}`,
      isNew: !item.seenAt,
      items: [item],
      more: 0,
    });
  }
  return out;
}

/** How many rows there are in all. */
export function rowCount(buckets: readonly { rows: readonly Row[] }[]): number {
  return buckets.reduce((sum, b) => sum + b.rows.length, 0);
}

/** The first `limit` rows, buckets kept (a bucket with none left is dropped). */
export function firstRows<T extends { rows: readonly Row[] }>(buckets: readonly T[], limit: number): T[] {
  const out: T[] = [];
  let left = limit;
  for (const bucket of buckets) {
    if (left <= 0) break;
    const rows = bucket.rows.slice(0, left);
    left -= rows.length;
    out.push({ ...bucket, rows });
  }
  return out;
}

/** The ids of rows not seen yet. */
export function unseenIds(items: readonly FeedItem[]): string[] {
  return items.filter(i => !i.seenAt).map(i => i.id);
}

/** The file a row opens, as spelled on disk. */
export function pathOf(item: FeedItem): string | null {
  return typeof item.data.path === 'string' ? item.data.path : null;
}

/** The state word of a folder's switch. */
export function watchLine(folder: { on: boolean; held: boolean }): string {
  if (folder.held) return 'Stopped after failing 3 times';
  return folder.on ? 'Shows new files' : 'Off';
}
