import { useCallback, useEffect, useState } from 'react';
import { slackError } from './api';
import { onTelegramStatus, telegramApi } from './telegramApi';
import type { TelegramStatus } from './telegramModel';

/** How often times on the page are refreshed. */
const CLOCK_TICK_MS = 30_000;

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
  const [status, setStatus] = useState<TelegramStatus | null>(null);
  const [loadError, setLoadError] = useState<string | null>(null);
  const [now, setNow] = useState(() => Date.now());

  const retry = useCallback(() => {
    setLoadError(null);
    telegramApi
      .status()
      .then(next => { setStatus(next); setLoadError(null); })
      .catch(err => setLoadError(slackError(err)));
  }, []);

  useEffect(() => {
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
