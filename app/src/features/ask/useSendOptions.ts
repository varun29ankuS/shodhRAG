import { useMemo } from 'react';
import { sendOptionsFor, sendOptionsKey } from './sendOptions';
import type { OptionSource } from './sendOptions';
import type { SendOptions } from './types';

/**
 * `sendOptionsFor(sources)`, the same object until the options change (the
 * sources array changes on every indexing progress event; answers memoise on
 * callbacks built from these options).
 */
export function useSendOptions(sources: readonly OptionSource[]): SendOptions {
  const options = sendOptionsFor(sources);
  const key = sendOptionsKey(options);
  // Keyed on the options themselves, not the array they came from.
  return useMemo(() => options, [key]);
}
