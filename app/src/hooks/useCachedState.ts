import { useEffect, useState } from 'react';
import type { Dispatch, SetStateAction } from 'react';
import { readViewCache, writeViewCache } from '../lib/viewCache';
import type { ViewCacheKey } from '../lib/viewCache';

/**
 * `useState` whose value starts from the view's last data (lib/viewCache) and
 * is kept there whenever it changes. `null` means "not loaded yet" and is
 * never written, so a remount after an error still shows the last good data
 * while the view loads again.
 */
export function useCachedState<T>(key: ViewCacheKey): [T | null, Dispatch<SetStateAction<T | null>>] {
  const [value, setValue] = useState<T | null>(() => readViewCache<T>(key));
  useEffect(() => {
    writeViewCache(key, value);
  }, [key, value]);
  return [value, setValue];
}
