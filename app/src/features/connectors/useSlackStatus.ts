import { useCallback, useEffect, useState } from 'react';
import { onSlackStatus, slackApi, slackError } from './api';
import type { SlackStatus } from './slackModel';

/** How often times on the page ("tomorrow", "in 3 minutes") are refreshed. */
const CLOCK_TICK_MS = 30_000;

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
  const [status, setStatus] = useState<SlackStatus | null>(null);
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
