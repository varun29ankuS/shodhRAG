import { useCallback, useEffect, useState } from 'react';
import { useCachedState } from '../../hooks/useCachedState';
import { VIEW_CACHE_KEYS, refreshViewCache, writeViewCache } from '../../lib/viewCache';
import { slackError } from './api';
import { onTelegramStatus, telegramApi } from './telegramApi';
import type { TelegramStatus } from './telegramModel';

/** How often times on the page are refreshed. */
const CLOCK_TICK_MS = 30_000;

/**
 * Keeps the cached status current while the Connectors view is closed, so
 * opening it again shows the status as it is, not as it was when it closed.
 * Started by the first mount and kept for the app's life (one listener).
 */
let statusFeed: (() => void) | null = null;

export interface TelegramStatusState {
  /** `null` until the first status arrives. */
  status: TelegramStatus | null;
  loadError: string | null;
  setStatus: (status: TelegramStatus) => void;
  retry: () => void;
  now: number;
}

/** The Telegram connector's status, kept current with the backend's events. */
export function useTelegramStatus(): TelegramStatusState {
  // The last status seen this session renders at once on a revisit; the read below refreshes it.
  const [status, setStatus] = useCachedState<TelegramStatus>(VIEW_CACHE_KEYS.telegramStatus);
  const [loadError, setLoadError] = useState<string | null>(null);
  const [now, setNow] = useState(() => Date.now());

  const retry = useCallback(() => {
    // The error stays until the read answers: clearing it first would show a
    // status kept from earlier as current while it is being read again.
    // A failed read drops the kept status, so the next visit reads afresh; the
    // status on screen stays, and the page says it could not refresh (lib/freshness).
    void refreshViewCache(VIEW_CACHE_KEYS.telegramStatus, () => telegramApi.status()).then(result => {
      if (result.ok) {
        setStatus(result.value);
        setLoadError(null);
      } else {
        setLoadError(slackError(result.error));
      }
    });
  }, []);

  useEffect(() => {
    statusFeed ??= onTelegramStatus(next => writeViewCache(VIEW_CACHE_KEYS.telegramStatus, next));
    retry();
    const unsubscribe = onTelegramStatus(next => { setStatus(next); setLoadError(null); });
    const tick = window.setInterval(() => setNow(Date.now()), CLOCK_TICK_MS);
    return () => {
      unsubscribe();
      window.clearInterval(tick);
    };
  }, [retry]);

  return { status, loadError, setStatus, retry, now };
}
