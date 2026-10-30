/**
 * The last data each view showed, kept for this session in memory only (never
 * written to disk: model and connector state stay out of storage). A view
 * mounted again renders its last data at once and refreshes it in the
 * background, so switching back to a view never flashes a skeleton or a
 * "Loading" line over data it already had.
 *
 * Kept data is never allowed to pass for current when it is not:
 * - a failed refresh drops it, so the next visit loads afresh (the view on
 *   screen says it could not refresh, see lib/freshness);
 * - a refresh that started before the data was dropped (it changed, or
 *   stored data was cleared) does not write its older result back;
 * - data whose changes are announced by an event (tasks) is kept only while
 *   the listener that drops it is registered.
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
/** Bumped on every drop, so a refresh started before it can tell. */
const epochs = new Map<ViewCacheKey, number>();
/** Keys whose data may be kept only while their change listener is registered. */
const watches = new Map<ViewCacheKey, 'registering' | 'live' | 'failed'>();

function epochOf(key: ViewCacheKey): number {
  return epochs.get(key) ?? 0;
}

/** A watched key keeps data only once its listener is live. */
function mayKeep(key: ViewCacheKey): boolean {
  const watch = watches.get(key);
  return watch === undefined || watch === 'live';
}

/** The last value kept under `key`, or `null` when there is none to show. */
export function readViewCache<T>(key: ViewCacheKey): T | null {
  return mayKeep(key) && entries.has(key) ? (entries.get(key) as T) : null;
}

/** Keep `value` as the view's last data. `null` and `undefined` are never kept. */
export function writeViewCache<T>(key: ViewCacheKey, value: T | null | undefined): void {
  if (value === null || value === undefined || !mayKeep(key)) return;
  entries.set(key, value);
}

/** Forget one view's data (it changed while the view was closed, or could not be refreshed). */
export function dropViewCache(key: ViewCacheKey): void {
  entries.delete(key);
  epochs.set(key, epochOf(key) + 1);
}

/** Forget every view's data (stored data was cleared). */
export function clearViewCache(): void {
  for (const key of Object.values(VIEW_CACHE_KEYS)) dropViewCache(key);
}

/** Taken when a refresh starts; its result is kept only if nothing was dropped since. */
export interface CacheTicket {
  readonly key: ViewCacheKey;
  readonly epoch: number;
}

export function viewCacheTicket(key: ViewCacheKey): CacheTicket {
  return { key, epoch: epochOf(key) };
}

/** Keep `value` unless the data was dropped after `ticket` was taken. Returns whether it was kept. */
export function writeViewCacheIfCurrent<T>(ticket: CacheTicket, value: T | null | undefined): boolean {
  if (epochOf(ticket.key) !== ticket.epoch) return false;
  if (value === null || value === undefined || !mayKeep(ticket.key)) return false;
  entries.set(ticket.key, value);
  return true;
}

/** Both shapes carry every field, so callers read them without narrowing (the app is not compiled with strict null checks). */
export type RefreshResult<T> = { ok: true; value: T; error: null } | { ok: false; value: null; error: unknown };

/**
 * Load a view's data and keep it: on success unless it was dropped while
 * loading; on failure the kept data is dropped, so it is not shown as
 * current on the next visit.
 */
export async function refreshViewCache<T>(key: ViewCacheKey, load: () => Promise<T>): Promise<RefreshResult<T>> {
  const ticket = viewCacheTicket(key);
  try {
    const value = await load();
    writeViewCacheIfCurrent(ticket, value);
    return { ok: true, value, error: null };
  } catch (error) {
    dropViewCache(key);
    return { ok: false, value: null, error };
  }
}

/**
 * Drop `key`'s data whenever `subscribe`'s source says it changed (the
 * `calendar-changed` event for tasks). Registered once for the app's life;
 * until the registration has succeeded nothing is kept for `key`, and a
 * failed registration is tried again on the next call.
 */
export function dropViewCacheOn(key: ViewCacheKey, subscribe: (onChange: () => void) => Promise<unknown>): void {
  const watch = watches.get(key);
  if (watch === 'registering' || watch === 'live') return;
  watches.set(key, 'registering');
  dropViewCache(key);
  let registered: Promise<unknown>;
  try {
    registered = subscribe(() => dropViewCache(key));
  } catch (error) {
    registered = Promise.reject(error);
  }
  registered.then(
    () => {
      watches.set(key, 'live');
    },
    () => {
      watches.set(key, 'failed');
      dropViewCache(key);
    },
  );
}
