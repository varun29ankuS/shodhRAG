import { useCallback, useEffect, useState } from 'react';
import { useCachedState } from '../../hooks/useCachedState';
import { VIEW_CACHE_KEYS, refreshViewCache, writeViewCache } from '../../lib/viewCache';
import { slackError } from './api';
import { githubApi, onGitHubStatus } from './githubApi';
import type { GitHubStatus } from './githubModel';

/** How often times on the page are refreshed: each second while a sign-in code counts down. */
const CLOCK_TICK_MS = 30_000;
const COUNTDOWN_TICK_MS = 1_000;

/**
 * Keeps the cached status current while the Connectors view is closed. Started by the
 * first mount and kept for the app's life (one listener).
 */
let statusFeed: (() => void) | null = null;

export interface GitHubStatusState {
  /** `null` until the first status arrives. */
  status: GitHubStatus | null;
  loadError: string | null;
  setStatus: (status: GitHubStatus) => void;
  retry: () => void;
  now: number;
}

/** The GitHub connector's status, kept current with the backend's events. */
export function useGitHubStatus(): GitHubStatusState {
  const [status, setStatus] = useCachedState<GitHubStatus>(VIEW_CACHE_KEYS.githubStatus);
  const [loadError, setLoadError] = useState<string | null>(null);
  const [now, setNow] = useState(() => Date.now());

  const retry = useCallback(() => {
    void refreshViewCache(VIEW_CACHE_KEYS.githubStatus, () => githubApi.status()).then(result => {
      if (result.ok) {
        setStatus(result.value);
        setLoadError(null);
      } else {
        setLoadError(slackError(result.error));
      }
    });
  }, []);

  useEffect(() => {
    statusFeed ??= onGitHubStatus(next => writeViewCache(VIEW_CACHE_KEYS.githubStatus, next));
    retry();
    const unsubscribe = onGitHubStatus(next => { setStatus(next); setLoadError(null); });
    return unsubscribe;
  }, [retry]);

  const counting = status?.state === 'signing_in';
  useEffect(() => {
    setNow(Date.now());
    const tick = window.setInterval(() => setNow(Date.now()), counting ? COUNTDOWN_TICK_MS : CLOCK_TICK_MS);
    return () => window.clearInterval(tick);
  }, [counting]);

  return { status, loadError, setStatus, retry, now };
}
