/**
 * The last data each view showed, kept for this session in memory only (never
 * written to disk: model and connector state stay out of storage). A view
 * mounted again renders its last data at once and refreshes it in the
 * background, so switching back to a view never flashes a skeleton or a
 * "Loading" line over data it already had.
 *
 * No imports: unit-tested under Node (`app/tests/viewCache.test.ts`).
 */

/** What each view keeps. One key per view's data, so the set stays small. */
export const VIEW_CACHE_KEYS = {
  slackStatus: 'connectors.slack.status',
  telegramStatus: 'connectors.telegram.status',
  tasksSnapshot: 'tasks.snapshot',
  auditStats: 'activity.audit.stats',
  auditFirstPage: 'activity.audit.firstPage',
  modelPicker: 'settings.modelPicker',
} as const;

export type ViewCacheKey = typeof VIEW_CACHE_KEYS[keyof typeof VIEW_CACHE_KEYS];

const entries = new Map<ViewCacheKey, unknown>();

/** The last value written under `key`, or `null` when the view has not loaded it yet. */
export function readViewCache<T>(key: ViewCacheKey): T | null {
  return entries.has(key) ? (entries.get(key) as T) : null;
}

/** Keep `value` as the view's last data. `null` and `undefined` are never kept. */
export function writeViewCache<T>(key: ViewCacheKey, value: T | null | undefined): void {
  if (value === null || value === undefined) return;
  entries.set(key, value);
}

/** Forget one view's data (it changed while the view was closed). */
export function dropViewCache(key: ViewCacheKey): void {
  entries.delete(key);
}

/** Forget every view's data (stored data was cleared). */
export function clearViewCache(): void {
  entries.clear();
}
