/**
 * Data read from the computer for a screen: read when the phone is
 * connected, read again when it reconnects or when the computer says it
 * changed, and kept (shown as last read) while the computer is away.
 */
import { useCallback, useEffect, useRef, useState } from 'react';
import { useLink } from './context';
import { errorText } from './link';
import type { Shim } from './invoke';

export interface Remote<T> {
  data: T | undefined;
  /** Reading now (first read, or again). */
  loading: boolean;
  error: string | null;
  reload: () => void;
}

/**
 * `load` runs with the computer's commands; `key` names what is read (a new key
 * starts over); `events` are the computer's events after which it is read again.
 */
export function useRemote<T>(key: string | null, load: (desktop: Shim) => Promise<T>, events: readonly string[] = []): Remote<T> {
  const { connected, desktop } = useLink();
  const [data, setData] = useState<{ key: string; value: T } | undefined>(undefined);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [version, setVersion] = useState(0);
  const loadRef = useRef(load);
  loadRef.current = load;
  const reload = useCallback(() => setVersion(v => v + 1), []);

  useEffect(() => {
    if (key === null || !connected) {
      setLoading(false);
      return;
    }
    let alive = true;
    setLoading(true);
    setError(null);
    loadRef
      .current(desktop)
      .then(value => {
        if (alive) setData({ key, value });
      })
      .catch(e => {
        if (alive) setError(errorText(e));
      })
      .finally(() => {
        if (alive) setLoading(false);
      });
    return () => {
      alive = false;
    };
  }, [key, connected, desktop, version]);

  const eventKey = events.join('\n');
  useEffect(() => {
    if (key === null || eventKey === '') return;
    let alive = true;
    const stops: Array<() => void> = [];
    for (const name of eventKey.split('\n')) {
      desktop
        .listen(name, () => reload())
        .then(stop => {
          if (alive) stops.push(stop);
          else stop();
        })
        .catch(() => {
          // The link cannot listen now; the next connection reads again anyway.
        });
    }
    return () => {
      alive = false;
      for (const stop of stops) stop();
    };
  }, [key, eventKey, desktop, reload]);

  return { data: data && data.key === key ? data.value : undefined, loading, error, reload };
}
