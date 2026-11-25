import { useEffect, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { callsTodayLine, readUsageToday } from './modelCalls';
import type { ModelUsageToday } from './modelCalls';

/** How often the line reads the count again while it is on screen. */
const REFRESH_MS = 60_000;

type Read = { state: 'loading' } | { state: 'ready'; usage: ModelUsageToday } | { state: 'failed' };

/**
 * One line under Settings → Model → Model: the model calls Shodh made today, by
 * spender, and when the provider's day starts again. The line keeps its height in
 * every state.
 */
export function CallsToday() {
  const [read, setRead] = useState<Read>({ state: 'loading' });
  useEffect(() => {
    let live = true;
    const load = async () => {
      try {
        const usage = readUsageToday(await invoke<unknown>('model_usage_today'));
        if (live) setRead(usage ? { state: 'ready', usage } : { state: 'failed' });
      } catch {
        if (live) setRead({ state: 'failed' });
      }
    };
    void load();
    const timer = window.setInterval(() => void load(), REFRESH_MS);
    return () => {
      live = false;
      window.clearInterval(timer);
    };
  }, []);
  const text =
    read.state === 'ready'
      ? callsTodayLine(read.usage, Date.now())
      : read.state === 'failed'
        ? "Today's model calls couldn't be read."
        : 'Counting today’s model calls…';
  return (
    <p className="m-0 min-h-[1.25rem] truncate text-caption text-shodh-text-muted" role="status" title={text}>
      {text}
    </p>
  );
}
