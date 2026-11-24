/**
 * Library → What's new: the section's state, its rows (bucketed by day, a folder's
 * busy day folded into one row) and their words. Pure (type imports only), so Node's
 * strip-types runner tests it directly (`app/tests/whatsNew.test.ts`).
 */

import type { AlertView, FeedItem } from './types';

/** Rows the section shows before "Show all"; the list always reserves their height. */
export const VISIBLE_ROWS = 5;

/** More files than this from one folder on one day become one row. */
export const GROUP_AFTER = 3;

export const EMPTY_COPY =
  'Nothing new yet. New files in your Library folders show up here. You can also turn on alerts for new papers on a topic.';

/** Shown in place of the alert controls while nothing may leave the computer. */
export const LOCAL_ONLY_COPY = 'Paper alerts are paused: Local-only mode is on, so nothing leaves this computer.';

/** The same, when it is the web-access switch that is off. */
export const WEB_OFF_COPY = 'Paper alerts are paused: web access is off, so nothing leaves this computer.';

/** Why alerts are paused, in one line, from the backend's reason. */
export function blockedCopy(reason: string | null): string | null {
  if (!reason) return null;
  return reason.startsWith('Local-only') ? LOCAL_ONLY_COPY : WEB_OFF_COPY;
}

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

/** A row of the section: one file, a folder's busy day, or one paper. */
export interface Row {
  key: string;
  kind: 'file' | 'file_group' | 'paper';
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

/** The local day of a time, `yyyy-mm-dd` (as the backend keys a folder's extra count). */
export function localDayKey(foundAt: string): string {
  const at = new Date(foundAt);
  const mm = String(at.getMonth() + 1).padStart(2, '0');
  const dd = String(at.getDate()).padStart(2, '0');
  return `${at.getFullYear()}-${mm}-${dd}`;
}

/** Files a folder found beyond its listed ones on a day (`FolderWatchView.moreByDay`). */
export type MoreOf = (sourceId: string, day: string) => number;

/**
 * The rows of `items` (newest first, as listed), bucketed. Within a bucket, the files
 * of one folder found on one local day stay one row each up to {@link GROUP_AFTER};
 * past that (or with files beyond the sync's cap) they are one row, placed where its
 * newest file was.
 */
export function rowsOf(items: readonly FeedItem[], now: Date, moreOf: MoreOf = () => 0): { bucket: Bucket; rows: Row[] }[] {
  const groups = new Map<string, FeedItem[]>();
  for (const item of items) {
    if (item.kind === 'paper') continue;
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
    if (item.kind === 'paper') {
      push(bucketOf(item.foundAt, now), {
        key: item.id,
        kind: 'paper',
        title: item.title,
        meta: paperMeta(item),
        isNew: !item.seenAt,
        items: [item],
        more: 0,
      });
      continue;
    }
    const key = `${sourceOf(item)}|${localDayKey(item.foundAt)}`;
    const group = groups.get(key) ?? [item];
    const more = Math.max(0, moreOf(sourceOf(item), localDayKey(item.foundAt)));
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

/** A paper's line: year, the APIs that found it, the alert. */
export function paperMeta(item: FeedItem): string {
  const parts: string[] = [];
  if (typeof item.data.year === 'number') parts.push(String(item.data.year));
  const sources = Array.isArray(item.data.sources) ? item.data.sources.filter((s): s is string => typeof s === 'string') : [];
  if (sources.length > 0) parts.push(sources.join(', '));
  if (typeof item.data.words === 'string') parts.push(`alert: ${item.data.words}`);
  return parts.join(' · ');
}

/** A paper's open-access PDF, when the API reported one. */
export function pdfOf(item: FeedItem): string | null {
  return typeof item.data.pdfUrl === 'string' && item.data.pdfUrl.startsWith('https://') ? item.data.pdfUrl : null;
}

/** Where a paper's title links: its landing page. */
export function landingOf(item: FeedItem): string | null {
  return typeof item.data.landingUrl === 'string' && /^https?:\/\//.test(item.data.landingUrl) ? item.data.landingUrl : null;
}

/** A paper saved to the library from What's new. */
export function savedPathOf(item: FeedItem): string | null {
  return typeof item.data.addedPath === 'string' ? item.data.addedPath : null;
}

function clockOf(at: string): string {
  const d = new Date(at);
  return Number.isNaN(d.getTime()) ? '' : `${String(d.getHours()).padStart(2, '0')}:${String(d.getMinutes()).padStart(2, '0')}`;
}

function dayOf(at: string): string {
  const d = new Date(at);
  return Number.isNaN(d.getTime()) ? '' : `${WEEKDAYS[d.getDay()]} ${d.getDate()} ${MONTHS[d.getMonth()]}, ${clockOf(at)}`;
}

/** An alert's state in a word, its fact in one line, and its one fix. */
export function alertLine(alert: AlertView): { word: string; fact: string; fix: 'resume' | 'privacy' | 'turn_on' | null } {
  switch (alert.state) {
    case 'checking':
      return { word: 'Checking', fact: 'Checking now…', fix: null };
    case 'waiting_retry': {
      const who = alert.failedApis.join(' and ') || 'An API';
      const when = alert.retryAt ? ` at ${clockOf(alert.retryAt)}` : ' later';
      return { word: 'Retrying', fact: `${who} didn't answer; trying again${when}`, fix: null };
    }
    case 'offline':
      return { word: 'Waiting', fact: "No internet connection; Shodh checks again when it's back", fix: null };
    case 'active':
      return {
        word: 'On',
        fact: alert.nextCheckAt ? `Next check ${dayOf(alert.nextCheckAt)}` : 'Checks every Monday at 09:00',
        fix: null,
      };
    case 'off':
      return { word: 'Off', fact: 'Nothing is sent for it', fix: 'turn_on' };
    case 'held':
      return { word: 'Stopped', fact: 'Stopped after failing 3 weeks in a row', fix: 'resume' };
    case 'blocked':
      return { word: 'Paused', fact: blockedCopy(alert.blockedReason) ?? LOCAL_ONLY_COPY, fix: 'privacy' };
  }
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

/** The rows on screen: every file row, and a folder's busy day only once expanded. */
export function renderedIds(buckets: readonly { rows: readonly Row[] }[], expanded: ReadonlySet<string>): string[] {
  const ids: string[] = [];
  for (const bucket of buckets) {
    for (const row of bucket.rows) {
      if (row.kind === 'file_group' && !expanded.has(row.key)) continue;
      for (const item of row.items) ids.push(item.id);
    }
  }
  return ids;
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
