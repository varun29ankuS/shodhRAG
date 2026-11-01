/**
 * The System status, kept current: read once, again whenever the backend
 * says what runs search changed or indexing was paused or resumed, and every
 * few seconds while something indexes. The latest progress of each running
 * folder (its speed and time left) comes from `indexing-progress`.
 */
import { useCallback, useEffect, useRef, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import { INDEXING_PAUSED_EVENT, readSystemStatus } from './systemStatus';
import type { SystemStatus } from './systemStatus';

/** Emitted by the backend when what runs search changed (`indexing_control.rs`). */
const SYSTEM_STATUS_EVENT = 'system-status-changed';
const PROGRESS_EVENT = 'indexing-progress';
/** How often the status is read again while something indexes. */
const BUSY_REFRESH_MS = 5_000;
/** ... and otherwise (the model may have been unloaded while idle). */
const IDLE_REFRESH_MS = 60_000;

/** The last progress event of a running folder. */
export interface FolderProgress {
  processed: number;
  total: number;
  /** Files a second, measured by the indexer; 0 when not known yet. */
  speed: number;
  /** Seconds left at that speed; 0 when not known yet. */
  etaSeconds: number;
}

export interface SystemStatusState {
  status: SystemStatus | null;
  /** The last progress of each folder by source id. */
  progress: Readonly<Record<string, FolderProgress>>;
  refresh: () => void;
  setPaused: (paused: boolean) => Promise<void>;
  cancel: (sourceId?: string) => Promise<number>;
  setUseGpu: (useGpu: boolean) => Promise<void>;
}

function progressOf(payload: unknown): { space: string; progress: FolderProgress; done: boolean } | null {
  if (typeof payload !== 'object' || payload === null) return null;
  const p = payload as Record<string, unknown>;
  const space = typeof p.space_id === 'string' ? p.space_id : '';
  if (!space) return null;
  const num = (v: unknown) => (typeof v === 'number' && Number.isFinite(v) && v >= 0 ? v : 0);
  return {
    space,
    progress: {
      processed: num(p.processed_files),
      total: num(p.total_files),
      speed: num(p.speed),
      etaSeconds: num(p.eta_seconds),
    },
    done: num(p.percentage) >= 100,
  };
}

export function useSystemStatus(): SystemStatusState {
  const [status, setStatus] = useState<SystemStatus | null>(null);
  const [progress, setProgress] = useState<Record<string, FolderProgress>>({});
  const mounted = useRef(true);

  const refresh = useCallback(() => {
    invoke('system_status')
      .then(value => {
        if (mounted.current) setStatus(readSystemStatus(value));
      })
      .catch(err => console.error('Failed to read the system status:', err));
  }, []);

  useEffect(() => {
    mounted.current = true;
    refresh();
    const unlisteners: Array<Promise<() => void>> = [
      listen(SYSTEM_STATUS_EVENT, () => refresh()),
      listen(INDEXING_PAUSED_EVENT, () => refresh()),
      listen(PROGRESS_EVENT, event => {
        const read = progressOf(event.payload);
        if (!read) return;
        setProgress(current => {
          if (read.done) {
            if (!(read.space in current)) return current;
            const { [read.space]: _finished, ...rest } = current;
            return rest;
          }
          return { ...current, [read.space]: read.progress };
        });
      }),
    ];
    return () => {
      mounted.current = false;
      for (const unlisten of unlisteners) {
        unlisten.then(fn => fn()).catch(() => undefined);
      }
    };
  }, [refresh]);

  const busy = (status?.running.length ?? 0) > 0;
  useEffect(() => {
    const timer = window.setInterval(refresh, busy ? BUSY_REFRESH_MS : IDLE_REFRESH_MS);
    return () => window.clearInterval(timer);
  }, [busy, refresh]);

  const setPaused = useCallback(
    async (paused: boolean) => {
      if (paused) await invoke('pause_indexing');
      else await invoke('resume_indexing');
      refresh();
    },
    [refresh],
  );

  const cancel = useCallback(
    async (sourceId?: string) => {
      const stopped = await invoke<number>('cancel_indexing', { sourceId: sourceId ?? null });
      refresh();
      return stopped;
    },
    [refresh],
  );

  const setUseGpu = useCallback(async (useGpu: boolean) => {
    const value = await invoke('set_search_gpu', { useGpu });
    if (mounted.current) setStatus(readSystemStatus(value));
  }, []);

  return { status, progress, refresh, setPaused, cancel, setUseGpu };
}
