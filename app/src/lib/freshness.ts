/**
 * How current the data a view shows is. A view that renders its last data
 * while it refreshes (lib/viewCache) must say so when that refresh fails:
 * "stale" data is shown with "Couldn't refresh", never as if it were current
 * (a connector still pilled "Connected", a task list that may have changed).
 *
 * No imports: unit-tested under Node (`app/tests/freshness.test.ts`).
 */

export type Freshness =
  /** Nothing to show yet; the first read is running. */
  | 'loading'
  /** Nothing to show: the read failed. */
  | 'failed'
  /** Data from an earlier read is shown, and the latest read failed. */
  | 'stale'
  /** The data shown is what the latest read returned (or a live event since). */
  | 'fresh';

export function freshness(hasData: boolean, error: string | null): Freshness {
  if (hasData) return error ? 'stale' : 'fresh';
  return error ? 'failed' : 'loading';
}

/** What a view says over data it could not refresh. */
export const STALE_TITLE = "Couldn't refresh";
export const STALE_DETAIL = 'Showing the last known state.';
