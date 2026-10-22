import { useCallback, useEffect, useRef, useState } from 'react';
import { onTableChanged, tableError, tablesApi } from './api';
import { jobActive } from './format';
import type { FillStatus } from './types';

/** How often a running or queued fill is asked for its progress. */
export const ACTIVE_POLL_MS = 2_000;
/**
 * How often a paused fill is looked at: the scheduler takes it up again on its own (every
 * minute at most) without a change event at the start of the run.
 */
export const PAUSED_POLL_MS = 15_000;

function visible(): boolean {
  return typeof document === 'undefined' || document.visibilityState === 'visible';
}

/**
 * The table's fill status: read at once, again on every `table_changed` of the table, and
 * polled while a fill is active (fast while it runs or waits to start, slowly while it is
 * paused). Nothing is polled while no fill is active or while the window is hidden; the
 * status is read again as soon as it is shown.
 */
export function useFillStatus(tableId: string): {
  status: FillStatus | null;
  error: string | null;
  refresh: () => Promise<void>;
} {
  const [status, setStatus] = useState<FillStatus | null>(null);
  const [error, setError] = useState<string | null>(null);
  // Only the newest answer is shown: a slow older one never overwrites it.
  const asked = useRef(0);

  const refresh = useCallback(async () => {
    const mine = ++asked.current;
    try {
      const next = await tablesApi.status(tableId);
      if (mine !== asked.current) return;
      setStatus(next);
      setError(null);
    } catch (err) {
      if (mine !== asked.current) return;
      setError(tableError(err).message);
    }
  }, [tableId]);

  useEffect(() => {
    setStatus(null);
    void refresh();
  }, [refresh]);

  useEffect(
    () => onTableChanged(id => {
      if (id === null || id === tableId) void refresh();
    }),
    [refresh, tableId],
  );

  const job = status?.job ?? null;
  const delay = !jobActive(job) ? null : job.state === 'paused' ? PAUSED_POLL_MS : ACTIVE_POLL_MS;

  useEffect(() => {
    if (delay === null) return undefined;
    // The next look is scheduled once the last one answered, so slow answers never pile up.
    let timer: number | null = null;
    let stopped = false;
    const stop = () => {
      if (timer !== null) {
        window.clearTimeout(timer);
        timer = null;
      }
    };
    const schedule = () => {
      if (!stopped && timer === null && visible()) {
        timer = window.setTimeout(() => {
          timer = null;
          void refresh().then(schedule);
        }, delay);
      }
    };
    const onVisibility = () => {
      if (visible()) {
        stop();
        void refresh().then(schedule);
      } else {
        stop();
      }
    };
    schedule();
    document.addEventListener('visibilitychange', onVisibility);
    return () => {
      stopped = true;
      stop();
      document.removeEventListener('visibilitychange', onVisibility);
    };
  }, [delay, refresh]);

  return { status, error, refresh };
}
