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
/** A Code project's code map started, progressed or finished (`code_map_commands.rs`). */
const CODE_MAP_EVENT = 'code-map-status';
/** How soon the status is read after a code map's progress. */
const CODE_MAP_REFRESH_MS = 1_000;
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
  /** Starts (or continues) re-indexing for the faster search model. */
  startModelSwitch: () => Promise<void>;
  /** Discards the re-index and what it embedded (the panel asks first). */
  cancelModelSwitch: () => Promise<void>;
  /** Stops the re-index and keeps its work, for Continue. */
  stopModelSwitch: () => Promise<void>;
  /** Brings over the last changes, saves, and restarts Shodh, which completes a ready re-index. */
  restartForModelSwitch: () => Promise<void>;
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
    let codeMapTimer: number | null = null;
    refresh();
    const unlisteners: Array<Promise<() => void>> = [
      listen(SYSTEM_STATUS_EVENT, () => refresh()),
      listen(INDEXING_PAUSED_EVENT, () => refresh()),
      // A code map sends progress several times a second: read the status at
      // most once a second, and once more after the last event.
      listen(CODE_MAP_EVENT, () => {
        if (codeMapTimer !== null) return;
        codeMapTimer = window.setTimeout(() => {
          codeMapTimer = null;
          refresh();
        }, CODE_MAP_REFRESH_MS);
      }),
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
      if (codeMapTimer !== null) window.clearTimeout(codeMapTimer);
      for (const unlisten of unlisteners) {
        unlisten.then(fn => fn()).catch(() => undefined);
      }
    };
  }, [refresh]);

  const busy = (status?.running.length ?? 0) > 0 || (status?.codeMaps.length ?? 0) > 0;
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

  const startModelSwitch = useCallback(async () => {
    await invoke('start_search_model_switch');
    refresh();
  }, [refresh]);

  const cancelModelSwitch = useCallback(async () => {
    await invoke('cancel_search_model_switch');
    refresh();
  }, [refresh]);

  const stopModelSwitch = useCallback(async () => {
    await invoke('stop_search_model_switch');
    refresh();
  }, [refresh]);

  const restartForModelSwitch = useCallback(async () => {
    await invoke('restart_for_search_model_switch');
    refresh();
  }, [refresh]);

  return {
    status,
    progress,
    refresh,
    setPaused,
    cancel,
    setUseGpu,
    startModelSwitch,
    cancelModelSwitch,
    stopModelSwitch,
    restartForModelSwitch,
  };
}
