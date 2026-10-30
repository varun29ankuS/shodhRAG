import { useCallback, useEffect, useState } from 'react';
import { useCachedState } from '../../hooks/useCachedState';
import { VIEW_CACHE_KEYS, writeViewCache } from '../../lib/viewCache';
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
  /** Why the status could not be read (shown while there is none). */
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
    setLoadError(null);
    slackApi
      .status()
      .then(next => { setStatus(next); setLoadError(null); })
      .catch(err => setLoadError(slackError(err)));
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
