/**
 * UI state kept on disk by the backend (`ui_state_get` / `ui_state_set`), so
 * it survives the app being killed: the Library's sources and the first-run
 * setup state. localStorage stays a copy for the first frame (it is read
 * synchronously); the backend's copy is the one that counts.
 *
 * The WebView writes localStorage to disk lazily, so a force-quit could lose
 * the source list while the index survived: the Library came back empty
 * over a full index, and setup started again. Entries saved by older builds
 * are moved to the backend once, the first time it has none.
 *
 * The reconcile functions are pure so they are unit-tested with Node
 * (`app/tests/durableState.test.ts`).
 */

import type { LibrarySource } from '../features/library/sources.ts';
import { parseStoredSources } from '../features/library/sources.ts';
import type { FirstRunState } from '../features/setup/firstRun.ts';
import { FIRST_RUN_STEPS } from '../features/setup/firstRun.ts';

export type DurableKey = 'library_sources' | 'first_run';

/** What reading the backend's copy gave: the value, nothing stored yet, or a failure. */
export type StoredRead = { kind: 'value'; value: unknown } | { kind: 'none' } | { kind: 'failed' };

/** The sources to show once the backend's copy is read, and whether to save them back. */
export function reconcileSources(
  stored: StoredRead,
  local: LibrarySource[],
  changedSinceStart: boolean,
): { sources: LibrarySource[]; save: boolean } {
  // A source added or indexed while the copy was read wins over it.
  if (changedSinceStart) return { sources: local, save: true };
  switch (stored.kind) {
    case 'value':
      return { sources: parseStoredSources(JSON.stringify(stored.value)), save: false };
    case 'none':
      // Nothing on disk yet: move the list older builds kept in localStorage.
      return { sources: local, save: local.length > 0 };
    case 'failed':
      // Unreadable on disk: the local copy is the best there is, and is saved over it.
      return { sources: local, save: local.length > 0 };
  }
}

function readFirstRunValue(value: unknown): FirstRunState | null {
  if (typeof value !== 'object' || value === null) return null;
  const { status, step } = value as Record<string, unknown>;
  if (status !== 'pending' && status !== 'skipped' && status !== 'completed') return null;
  const known = typeof step === 'string' && (FIRST_RUN_STEPS as readonly string[]).includes(step);
  return { status, step: known ? (step as FirstRunState['step']) : 'welcome' };
}

/**
 * The first-run state once the backend's copy is read. `localSaved` is
 * whether localStorage held one (else `local` is the default for a fresh
 * install, which must not overwrite a finished setup saved on disk).
 */
export function reconcileFirstRun(
  stored: StoredRead,
  local: FirstRunState,
  localSaved: boolean,
): { state: FirstRunState; save: boolean } {
  const fromDisk = stored.kind === 'value' ? readFirstRunValue(stored.value) : null;
  if (fromDisk) return { state: fromDisk, save: false };
  return { state: local, save: localSaved };
}
