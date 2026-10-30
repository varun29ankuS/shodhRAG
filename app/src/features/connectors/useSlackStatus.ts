import { useCallback, useEffect, useState } from 'react';
import { useCachedState } from '../../hooks/useCachedState';
import { VIEW_CACHE_KEYS, refreshViewCache, writeViewCache } from '../../lib/viewCache';
import { onSlackStatus, slackApi, slackError } from './api';
import type { SlackStatus } from './slackModel';

/** How often times on the page ("tomorrow", "in 3 minutes") are refreshed. */
const CLOCK_TICK_MS = 30_000;

/**
 * Keeps the cached status current while the Connectors view is closed, so
 * opening it again shows the status as it is, not as it was when it closed.
 * Started by the first mount and kept for the app's life (one listener).
 */
let statusFeed: (() => void) | null = null;

export interface SlackStatusState {
  /** `null` until the first status arrives. */
  status: SlackStatus | null;
  /** Why the latest read failed; with a status kept from earlier, that status may be out of date. */
  loadError: string | null;
  setStatus: (status: SlackStatus) => void;
  retry: () => void;
  /** The clock the page's times are said against. */
  now: number;
}

/** The Slack connector's status, kept current with the backend's events. */
export function useSlackStatus(): SlackStatusState {
  // The last status seen this session renders at once on a revisit; the read below refreshes it.
  const [status, setStatus] = useCachedState<SlackStatus>(VIEW_CACHE_KEYS.slackStatus);
  const [loadError, setLoadError] = useState<string | null>(null);
  const [now, setNow] = useState(() => Date.now());

  const retry = useCallback(() => {
    // The error stays until the read answers: clearing it first would show a
    // status kept from earlier as current while it is being read again.
    // A failed read drops the kept status, so the next visit reads afresh; the
    // status on screen stays, and the page says it could not refresh (lib/freshness).
    void refreshViewCache(VIEW_CACHE_KEYS.slackStatus, () => slackApi.status()).then(result => {
      if (result.ok) {
        setStatus(result.value);
        setLoadError(null);
      } else {
        setLoadError(slackError(result.error));
      }
    });
  }, []);

  useEffect(() => {
    statusFeed ??= onSlackStatus(next => writeViewCache(VIEW_CACHE_KEYS.slackStatus, next));
    retry();
    const unsubscribe = onSlackStatus(next => { setStatus(next); setLoadError(null); });
    const tick = window.setInterval(() => setNow(Date.now()), CLOCK_TICK_MS);
    return () => {
      unsubscribe();
      window.clearInterval(tick);
    };
  }, [retry]);

  return { status, loadError, setStatus, retry, now };
}
